use crate::db::TrackRecord;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use walkdir::WalkDir;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scanned: u32,
    pub indexed: u32,
    pub removed: u32,
    pub duration_ms: u64,
}

/// ReplayGain 2.0 defines its 0 dB reference as -18 LUFS, so an absolute integrated loudness can
/// be recovered from a track gain tag. Tracks without a tag fall back to the engine's rolling
/// K-weighted measurement at playback time.
const REPLAYGAIN_REFERENCE_LUFS: f64 = -18.0;

pub fn is_audio_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|s| s.to_ascii_lowercase())
            .as_deref(),
        Some("mp3" | "flac" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aiff" | "aif")
    )
}

/// SQLite stores TEXT as UTF-8 and rejects embedded NUL bytes, which malformed ID3 frames do
/// occasionally contain.
fn clean(value: String) -> String {
    let trimmed = value.replace('\0', "").trim().to_string();
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed
    }
}

/// Parses a ReplayGain tag such as `"-7.32 dB"` into a value in dB.
fn parse_replaygain_gain(raw: &str) -> Option<f64> {
    raw.split_whitespace()
        .next()
        .and_then(|n| n.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// Persists embedded cover art under `artwork_dir`, keyed by a content hash so every track on an
/// album shares one file instead of duplicating the same JPEG thousands of times.
/// Returns the absolute path of the written file.
fn store_artwork(artwork_dir: &Path, data: &[u8], extension: &str) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    let digest = Sha256::digest(data);
    let name = format!("{:x}.{}", digest, extension);
    let target = artwork_dir.join(&name);
    if !target.exists() {
        std::fs::create_dir_all(artwork_dir).ok()?;
        // Two scanner workers routinely extract the same album cover at once. The temp name is
        // unique per call so neither can rename the file out from under the other, and the final
        // rename is atomic so a reader never sees a partial image.
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let tmp = artwork_dir.join(format!(
            "{}.{}.{}.part",
            name,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        if std::fs::write(&tmp, data).is_err() {
            return None;
        }
        if std::fs::rename(&tmp, &target).is_err() {
            // Another worker published this exact content first; prefer its copy over failing.
            std::fs::remove_file(&tmp).ok();
            return target
                .exists()
                .then(|| target.to_string_lossy().to_string());
        }
    }
    Some(target.to_string_lossy().to_string())
}

fn artwork_extension(mime: Option<&str>) -> &'static str {
    match mime {
        Some(m) if m.contains("png") => "png",
        Some(m) if m.contains("gif") => "gif",
        Some(m) if m.contains("bmp") => "bmp",
        _ => "jpg",
    }
}

pub fn extract_metadata(path: &Path, artwork_dir: &Path) -> Option<TrackRecord> {
    let tagged_file = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    let properties = tagged_file.properties();

    let title = clean(
        tag.and_then(|t| t.title().map(|s| s.to_string()))
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown Title")
                    .to_string()
            }),
    );
    let artist = clean(
        tag.and_then(|t| t.artist().map(|s| s.to_string()))
            .unwrap_or_else(|| "Unknown Artist".to_string()),
    );
    let album = clean(
        tag.and_then(|t| t.album().map(|s| s.to_string()))
            .unwrap_or_else(|| "Unknown Album".to_string()),
    );
    let album_artist = tag
        .and_then(|t| {
            t.get_string(&ItemKey::AlbumArtist)
                .map(|s| clean(s.to_string()))
        })
        .filter(|a| a != "Unknown");

    let loudness_lufs = tag
        .and_then(|t| t.get_string(&ItemKey::ReplayGainTrackGain))
        .and_then(parse_replaygain_gain)
        .map(|gain_db| REPLAYGAIN_REFERENCE_LUFS - gain_db);

    let artwork_path = tag.and_then(|t| t.pictures().first()).and_then(|pic| {
        store_artwork(
            artwork_dir,
            pic.data(),
            artwork_extension(pic.mime_type().map(|m| m.as_str())),
        )
    });

    let file_path = path.to_string_lossy().to_string();
    Some(TrackRecord {
        id: format!("local://{}", file_path),
        provider: "local".to_string(),
        title,
        artist,
        album,
        album_artist,
        duration_ms: properties.duration().as_millis() as u64,
        year: tag.and_then(|t| t.year()),
        genre: tag
            .and_then(|t| t.genre().map(|s| s.to_string()))
            .map(clean)
            .filter(|g| g != "Unknown"),
        file_path: Some(file_path),
        artwork_url: artwork_path,
        bitrate: properties.audio_bitrate(),
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase()),
        loudness_lufs,
        track_number: tag.and_then(|t| t.track()),
        disc_number: tag.and_then(|t| t.disk()),
    })
}

/// Reads lyrics embedded in a file's tags (`USLT`, Vorbis `LYRICS`, MP4 `©lyr`). Checked before any
/// network lookup so offline listening still shows words.
pub fn embedded_lyrics(path: &Path) -> Option<String> {
    let tagged_file = Probe::open(path).ok()?.read().ok()?;
    tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())
        .and_then(|tag| tag.get_string(&ItemKey::Lyrics).map(str::to_string))
        .filter(|text| !text.trim().is_empty())
}

pub fn collect_audio_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| is_audio_file(p))
        .collect()
}

/// Walks `dir` and extracts metadata for every audio file, using a worker pool sized to the
/// machine. `progress` is called from the workers; it must be cheap and is expected to throttle
/// its own event emission.
pub fn scan_directory<F>(
    dir: &Path,
    artwork_dir: &Path,
    workers: usize,
    progress: F,
) -> Vec<TrackRecord>
where
    F: Fn(u32, u32, &Path) + Send + Sync,
{
    let files = collect_audio_files(dir);
    let total = files.len() as u32;
    let cursor = AtomicUsize::new(0);
    let results: Mutex<Vec<TrackRecord>> = Mutex::new(Vec::new());
    let done = AtomicUsize::new(0);

    let workers = workers.max(1).min(files.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = cursor.fetch_add(1, Ordering::Relaxed);
                let Some(path) = files.get(index) else { break };
                if let Some(record) = extract_metadata(path, artwork_dir) {
                    results
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(record);
                }
                let processed = done.fetch_add(1, Ordering::Relaxed) as u32 + 1;
                progress(processed, total, path);
            });
        }
    });

    let mut out = results.into_inner().unwrap_or_else(|e| e.into_inner());
    // Worker completion order is nondeterministic; sort so repeated scans produce a stable order.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_tags_map_to_absolute_lufs() {
        // A tag of -7.32 dB means the track sits 7.32 dB above the -18 LUFS reference.
        let lufs = REPLAYGAIN_REFERENCE_LUFS - parse_replaygain_gain("-7.32 dB").unwrap();
        assert!((lufs - (-10.68)).abs() < 1e-9);

        assert_eq!(parse_replaygain_gain("+2.00 dB"), Some(2.0));
        assert_eq!(parse_replaygain_gain("not a number"), None);
        assert_eq!(parse_replaygain_gain("inf"), None);
    }

    #[test]
    fn artwork_is_deduplicated_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"\xFF\xD8\xFFnot-really-a-jpeg-but-content-identical";

        let first = store_artwork(dir.path(), bytes, "jpg").unwrap();
        let second = store_artwork(dir.path(), bytes, "jpg").unwrap();
        assert_eq!(first, second, "identical art must collapse to one file");

        let other = store_artwork(dir.path(), b"\xFF\xD8\xFFdifferent", "jpg").unwrap();
        assert_ne!(first, other);

        let entries = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(entries, 2, "no .part files may be left behind");
    }

    #[test]
    fn extension_filter_accepts_supported_containers() {
        assert!(is_audio_file(Path::new("/m/song.FLAC")));
        assert!(is_audio_file(Path::new("/m/song.m4a")));
        assert!(!is_audio_file(Path::new("/m/cover.jpg")));
        assert!(!is_audio_file(Path::new("/m/no-extension")));
    }
}
