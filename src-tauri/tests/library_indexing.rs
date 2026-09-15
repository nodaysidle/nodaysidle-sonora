//! End-to-end check of the library pipeline against real, tagged audio files: a tagged MP3, an
//! MP3 with embedded cover art, and a FLAC carrying a ReplayGain tag.
//!
//! The unit tests in `library::scanner` use synthetic WAVs, which have no tags and no artwork, so
//! this is the only place the `lofty` read path runs against files the way a real library looks.
//!
//! Fixtures are produced with `ffmpeg`. If it is not installed the suite reports a skip rather than
//! failing, so a machine without ffmpeg still gets a green run.

use sonora_lib::db::DbState;
use sonora_lib::library::scanner::scan_directory;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Renders one MP3 with the given tags, optionally embedding `cover`.
fn make_mp3(dir: &Path, name: &str, title: &str, track: u32, cover: Option<&Path>) {
    let bare = dir.join(format!("{name}.bare.mp3"));
    let out = dir.join(format!("{name}.mp3"));
    assert!(
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1.5",
            "-metadata",
            &format!("title={title}"),
            "-metadata",
            "artist=YOASOBI",
            "-metadata",
            "album=THE BOOK",
            "-metadata",
            &format!("track={track}"),
            "-metadata",
            "date=2019",
            "-metadata",
            "genre=J-Pop",
            "-id3v2_version",
            "3",
            "-codec:a",
            "libmp3lame",
            "-b:a",
            "128k",
            bare.to_str().unwrap(),
        ]),
        "failed to render the MP3 audio for {name}"
    );

    match cover {
        Some(cover) => assert!(
            ffmpeg(&[
                "-i",
                bare.to_str().unwrap(),
                "-i",
                cover.to_str().unwrap(),
                "-map",
                "0:a",
                "-map",
                "1:v",
                "-c:a",
                "copy",
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-metadata:s:v",
                "title=Album cover",
                "-metadata:s:v",
                "comment=Cover (front)",
                out.to_str().unwrap(),
            ]),
            "failed to embed cover art into {name}"
        ),
        None => std::fs::rename(&bare, &out).unwrap(),
    }
    std::fs::remove_file(&bare).ok();
}

/// Builds a small music folder. Returns false when ffmpeg is unavailable.
fn build_fixtures(dir: &Path) -> bool {
    if !ffmpeg_available() {
        eprintln!("SKIPPED: ffmpeg is not installed, cannot build audio fixtures");
        return false;
    }

    let cover = dir.join("cover.png");
    assert!(
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "color=c=#C8FF00:s=300x300:d=1",
            "-frames:v",
            "1",
            cover.to_str().unwrap(),
        ]),
        "failed to render the cover art fixture"
    );

    // Untagged-art track, then two album-mates sharing the exact same cover so the content-hash
    // deduplication is actually exercised.
    make_mp3(dir, "a-yoru", "Yoru ni Kakeru", 1, None);
    make_mp3(dir, "b-kaibutsu", "Kaibutsu", 2, Some(&cover));
    make_mp3(dir, "c-gunjo", "Gunjo", 3, Some(&cover));

    assert!(
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=1.0",
            "-metadata",
            "title=Blue Monday",
            "-metadata",
            "artist=New Order",
            "-metadata",
            "album=Power, Corruption & Lies",
            "-metadata",
            "REPLAYGAIN_TRACK_GAIN=-7.32 dB",
            dir.join("d-vorbis.flac").to_str().unwrap(),
        ]),
        "failed to render the FLAC fixture"
    );

    std::fs::remove_file(&cover).ok();
    true
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    artwork_dir: PathBuf,
}

fn setup() -> Option<Fixture> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Music");
    std::fs::create_dir_all(&root).unwrap();

    // Nest a folder so the scan is proven recursive, not just a flat listing.
    let nested = root.join("YOASOBI").join("THE BOOK");
    std::fs::create_dir_all(&nested).unwrap();
    if !build_fixtures(&nested) {
        return None;
    }
    // A non-audio file must be ignored.
    std::fs::write(nested.join("notes.txt"), "not audio").unwrap();

    let artwork_dir = dir.path().join("artwork");
    Some(Fixture {
        _dir: dir,
        root,
        artwork_dir,
    })
}

#[test]
fn scans_a_real_folder_into_searchable_indexed_tracks() {
    let Some(fixture) = setup() else { return };

    let tracks = scan_directory(&fixture.root, &fixture.artwork_dir, 2, |_, _, _| {});

    assert_eq!(tracks.len(), 4, "expected four audio files, got {tracks:?}");

    let tagged = tracks
        .iter()
        .find(|t| t.title == "Yoru ni Kakeru")
        .expect("MP3 tags must reach the index");
    assert_eq!(tagged.artist, "YOASOBI");
    assert_eq!(tagged.album, "THE BOOK");
    assert_eq!(tagged.year, Some(2019));
    assert_eq!(tagged.genre.as_deref(), Some("J-Pop"));
    assert_eq!(tagged.track_number, Some(1));
    assert_eq!(tagged.format.as_deref(), Some("mp3"));
    assert_eq!(tagged.provider, "local");
    assert!(
        tagged.id.starts_with("local://"),
        "canonical URI scheme, got {}",
        tagged.id
    );
    assert!(
        (1_400..=1_600).contains(&tagged.duration_ms),
        "expected ~1.5 s, got {} ms",
        tagged.duration_ms
    );

    let flac = tracks
        .iter()
        .find(|t| t.title == "Blue Monday")
        .expect("FLAC vorbis comments must reach the index");
    assert_eq!(flac.artist, "New Order");
    assert_eq!(flac.format.as_deref(), Some("flac"));
    // ReplayGain -7.32 dB against the -18 LUFS reference.
    let lufs = flac.loudness_lufs.expect("ReplayGain tag must be parsed");
    assert!(
        (lufs - (-10.68)).abs() < 0.01,
        "expected -10.68 LUFS, got {lufs}"
    );

    // Index them the way the scan command does, then search and roll up.
    let db = DbState::new(fixture._dir.path().join("test.db")).unwrap();
    db.upsert_tracks(&tracks).unwrap();

    assert_eq!(db.count_tracks(Some("local")).unwrap(), 4);

    let hits = db.get_tracks(Some("yoru"), None, None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1, "FTS must find the track by title prefix");
    assert_eq!(hits[0].title, "Yoru ni Kakeru");

    let same_artist = db.get_tracks(Some("yoasobi"), None, None, 10, 0).unwrap();
    assert_eq!(
        same_artist.len(),
        3,
        "all three album tracks share an artist"
    );

    let by_artist = db.get_tracks(Some("new order"), None, None, 10, 0).unwrap();
    assert_eq!(by_artist.len(), 1);
    assert_eq!(by_artist[0].album, "Power, Corruption & Lies");

    // A rescan must update in place rather than duplicate.
    db.upsert_tracks(&tracks).unwrap();
    assert_eq!(db.count_tracks(None).unwrap(), 4);

    let albums = db.get_albums(None, None).unwrap();
    assert_eq!(albums.len(), 2, "two distinct albums across four files");
    let book = albums.iter().find(|a| a.album == "THE BOOK").unwrap();
    assert_eq!(book.track_count, 3);
    assert_eq!(book.album_artist, None, "no album artist was tagged");
    assert!(
        book.artwork_url.is_some(),
        "album rollup surfaces the cover"
    );
}

#[test]
fn embedded_cover_art_is_extracted_and_deduplicated() {
    let Some(fixture) = setup() else { return };

    let tracks = scan_directory(&fixture.root, &fixture.artwork_dir, 2, |_, _, _| {});

    // Two album-mates carry byte-identical embedded PNGs.
    let with_art: Vec<_> = tracks.iter().filter(|t| t.artwork_url.is_some()).collect();
    assert_eq!(
        with_art.len(),
        2,
        "both art-bearing MP3s should report artwork"
    );
    assert_eq!(
        with_art[0].artwork_url, with_art[1].artwork_url,
        "identical cover art must collapse to one cached file"
    );

    let artwork_path = PathBuf::from(with_art[0].artwork_url.as_ref().unwrap());
    assert!(artwork_path.is_file(), "artwork file must exist on disk");
    assert_eq!(
        artwork_path.extension().and_then(|e| e.to_str()),
        Some("png"),
        "PNG cover must keep its extension"
    );
    assert!(
        artwork_path.starts_with(&fixture.artwork_dir),
        "artwork must live in the app's cache directory"
    );

    let written = std::fs::read_dir(&fixture.artwork_dir).unwrap().count();
    assert_eq!(written, 1, "one deduplicated file, no .part leftovers");

    // The FLAC has no picture and must not claim one.
    let flac = tracks
        .iter()
        .find(|t| t.format.as_deref() == Some("flac"))
        .unwrap();
    assert!(flac.artwork_url.is_none());
}
