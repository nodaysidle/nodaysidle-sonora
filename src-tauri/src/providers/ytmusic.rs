//! YouTube Music via the public InnerTube API — the same endpoint the web player itself uses.
//!
//! Search and stream resolution use the public `WEB_REMIX` client values published by YouTube
//! Music, with yt-dlp as a fallback when a response is signature-ciphered.
//!
//! ponytail: InnerTube is undocumented and YouTube rotates its client versions and keys. Read the
//! current public web-player values at request time so a client-version rotation does not require a
//! release.

use super::{
    MusicProvider, ProviderKind, ProviderPlaylist, ProviderTrack, SearchResults, TrackAudioSource,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// music.youtube.com answers non-desktop user agents (including the Android app's) with an
/// "outdated browser" page that carries no InnerTube bootstrap values.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                          (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// The bootstrap page is ~0.5 MB; its values rotate on the order of days, not requests.
const WEB_REMIX_CONFIG_TTL: Duration = Duration::from_secs(6 * 60 * 60);

static WEB_REMIX_CONFIG: Mutex<Option<(String, String, Instant)>> = Mutex::new(None);

fn endpoint(path: &str, api_key: &str) -> String {
    format!("https://music.youtube.com/youtubei/v1/{path}?key={api_key}&prettyPrint=false")
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .expect("HTTP client configuration is valid")
}

/// Recursively collects every object stored under `key`, wherever it appears in the response.
/// InnerTube nests its payload under several layout renderers that change between releases, so
/// matching on the renderer name rather than a fixed path keeps parsing working across them.
fn collect_renderers<'a>(value: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                out.push(found);
            }
            for child in map.values() {
                collect_renderers(child, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_renderers(item, key, out);
            }
        }
        _ => {}
    }
}

fn renderers<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    collect_renderers(value, key, &mut out);
    out
}

/// Joins every `text` run under a `runs` array. InnerTube splits strings into runs whenever a
/// segment carries its own link or style.
fn runs_text(value: &Value) -> Option<String> {
    let runs = value.get("runs")?.as_array()?;
    let joined = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(|t| t.as_str()))
        .collect::<String>();
    let trimmed = joined.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn flex_column(renderer: &Value, index: usize) -> Option<String> {
    renderer
        .get("flexColumns")?
        .as_array()?
        .get(index)?
        .get("musicResponsiveListItemFlexColumnRenderer")?
        .get("text")
        .and_then(runs_text)
}

/// InnerTube reports durations as a trailing `m:ss` / `h:mm:ss` run inside the metadata column.
fn parse_duration(text: &str) -> u64 {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return 0;
    }
    let mut seconds = 0u64;
    for part in &parts {
        let Ok(value) = part.trim().parse::<u64>() else {
            return 0;
        };
        seconds = seconds * 60 + value;
    }
    seconds * 1000
}

fn video_id(renderer: &Value) -> Option<String> {
    // Playlists and album rows expose it directly.
    if let Some(id) = renderer
        .pointer("/playlistItemData/videoId")
        .and_then(|v| v.as_str())
    {
        return Some(id.to_string());
    }
    // Standalone song rows put it on the play button's watch endpoint.
    renderer
        .pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn thumbnail(renderer: &Value) -> Option<String> {
    renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|list| list.last())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
        .map(str::to_string)
        // YouTube escapes the URL's own slashes in some responses.
        .map(|url| url.replace("\\u0026", "&"))
}

fn parse_song(renderer: &Value) -> Option<ProviderTrack> {
    let id = video_id(renderer)?;
    let title = flex_column(renderer, 0)?;
    // Column 1 is `Artist • Album • Duration`; the first entry is the artist.
    let meta = flex_column(renderer, 1).unwrap_or_default();
    let mut segments = meta.split(" • ").map(str::trim).filter(|s| !s.is_empty());
    let artist = segments.next().unwrap_or("Unknown Artist").to_string();
    let album = segments.next().unwrap_or("Unknown Album").to_string();
    let duration_ms = segments.next().map(parse_duration).unwrap_or(0);

    Some(ProviderTrack {
        id: format!("ytmusic://track/{id}"),
        provider: ProviderKind::YouTubeMusic,
        title,
        artist,
        album,
        duration_ms,
        artwork_url: thumbnail(renderer),
    })
}

fn parse_playlist(renderer: &Value) -> Option<ProviderPlaylist> {
    let browse_id = renderer
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .and_then(|v| v.as_str())?;
    Some(ProviderPlaylist {
        id: format!("ytmusic://playlist/{browse_id}"),
        // Song rows use flexColumns; two-row tiles carry a plain `title` object instead.
        title: flex_column(renderer, 0)
            .or_else(|| renderer.pointer("/title").and_then(runs_text))?,
        description: None,
        artwork_url: thumbnail(renderer),
        track_count: 0,
        provider: ProviderKind::YouTubeMusic,
    })
}

fn bootstrap_value(page: &str, key: &str) -> Option<String> {
    [format!("\"{key}\":\""), format!("'{key}':'")]
        .iter()
        .find_map(|marker| {
            let start = page.find(marker)? + marker.len();
            let end = page[start..]
                .find('"')
                .or_else(|| page[start..].find('\''))?;
            Some(page[start..start + end].to_string())
        })
}

fn web_remix_config() -> Result<(String, String), String> {
    if let Ok(cached) = WEB_REMIX_CONFIG.lock() {
        if let Some((key, version, fetched)) = cached.as_ref() {
            if fetched.elapsed() < WEB_REMIX_CONFIG_TTL {
                return Ok((key.clone(), version.clone()));
            }
        }
    }
    let (api_key, version) = fetch_web_remix_config()?;
    if let Ok(mut cached) = WEB_REMIX_CONFIG.lock() {
        *cached = Some((api_key.clone(), version.clone(), Instant::now()));
    }
    Ok((api_key, version))
}

fn fetch_web_remix_config() -> Result<(String, String), String> {
    let page = client()
        .get("https://music.youtube.com/")
        .send()
        .map_err(|e| format!("YouTube Music configuration request failed: {e}"))?
        .text()
        .map_err(|e| format!("YouTube Music configuration was unreadable: {e}"))?;
    let api_key = bootstrap_value(&page, "INNERTUBE_API_KEY")
        .ok_or_else(|| "YouTube Music did not publish an InnerTube API key".to_string())?;
    let version = bootstrap_value(&page, "INNERTUBE_CLIENT_VERSION")
        .ok_or_else(|| "YouTube Music did not publish an InnerTube client version".to_string())?;
    Ok((api_key, version))
}

fn web_remix_context(version: &str) -> Value {
    json!({
        "client": {
            "clientName": "WEB_REMIX",
            "clientVersion": version,
            "hl": "en",
            "gl": "US",
        }
    })
}

fn inner_tube_post(path: &str, mut body: Value) -> Result<Value, String> {
    let (api_key, version) = web_remix_config()?;
    body["context"] = web_remix_context(&version);
    let response = client()
        .post(endpoint(path, &api_key))
        .header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/")
        .json(&body)
        .send()
        .map_err(|e| format!("YouTube Music request failed: {e}"))?;

    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("YouTube Music returned {status}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("unreadable YouTube Music response: {e}"))
}

pub struct YouTubeMusicProvider;

fn find_ytdlp_bin() -> Option<String> {
    for candidate in [
        "yt-dlp",
        "/opt/homebrew/bin/yt-dlp",
        "/usr/local/bin/yt-dlp",
        "/usr/bin/yt-dlp",
    ] {
        if let Ok(output) = std::process::Command::new(candidate)
            .arg("--version")
            .output()
        {
            if output.status.success() {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn parse_ytdlp_search_entry(value: &Value) -> Option<ProviderTrack> {
    let video_id = value.get("id").and_then(Value::as_str)?.trim();
    let title = value.get("title").and_then(Value::as_str)?.trim();
    if video_id.is_empty() || title.is_empty() {
        return None;
    }

    let artist = value
        .get("artist")
        .and_then(Value::as_str)
        .or_else(|| value.get("uploader").and_then(Value::as_str))
        .or_else(|| value.get("channel").and_then(Value::as_str))
        .unwrap_or("Unknown Artist")
        .trim()
        .to_string();
    let duration_ms = value
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1000.0) as u64)
        .unwrap_or(0);

    Some(ProviderTrack {
        id: format!("ytmusic://track/{video_id}"),
        provider: ProviderKind::YouTubeMusic,
        title: title.to_string(),
        artist,
        album: value
            .get("album")
            .and_then(Value::as_str)
            .unwrap_or("YouTube Music")
            .to_string(),
        duration_ms,
        artwork_url: Some(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")),
    })
}

fn search_with_ytdlp(query: &str, limit: usize) -> Result<SearchResults, String> {
    let bin = find_ytdlp_bin().ok_or_else(|| "yt-dlp binary not found".to_string())?;
    let search = format!("ytsearch{}:{query}", limit.clamp(1, 50));
    let output = std::process::Command::new(&bin)
        .args(["--flat-playlist", "--dump-json", "--no-warnings", &search])
        .output()
        .map_err(|e| format!("failed to execute {bin}: {e}"))?;

    if !output.status.success() {
        return Err(format!("yt-dlp exited with {}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tracks = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| parse_ytdlp_search_entry(&entry))
        .collect::<Vec<_>>();
    tracks.dedup_by(|a, b| a.id == b.id);
    tracks.truncate(limit);
    if tracks.is_empty() {
        return Err("yt-dlp returned no YouTube results".to_string());
    }

    Ok(SearchResults {
        tracks,
        playlists: Vec::new(),
    })
}

fn resolve_with_ytdlp(video_id: &str) -> Result<TrackAudioSource, String> {
    let bin = find_ytdlp_bin().ok_or_else(|| "yt-dlp binary not found".to_string())?;
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let output = std::process::Command::new(&bin)
        .args([
            "-g",
            "-f",
            "ba[ext=m4a]/ba[format_id=140]/ba[vcodec=none]/bestaudio",
            "--no-warnings",
            &url,
        ])
        .output()
        .map_err(|e| format!("failed to execute {bin}: {e}"))?;

    if !output.status.success() {
        return Err(format!("yt-dlp exited with {}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stream_url = stdout
        .lines()
        .find(|l| l.starts_with("http"))
        .ok_or_else(|| "no stream URL returned by yt-dlp".to_string())?
        .trim()
        .to_string();

    Ok(TrackAudioSource {
        url: stream_url,
        mime_type: Some("audio/mp4".to_string()),
        remote_playback: false,
    })
}

/// Words that mark an upload as another rendition of the song. A candidate may only carry the ones
/// the reference title carries too, so `Live Forever` still matches the studio `Live Forever`.
const VARIANT_WORDS: &[&str] = &[
    "live",
    "cover",
    "remix",
    "acoustic",
    "instrumental",
    "karaoke",
    "sped",
    "slowed",
    "nightcore",
    "reaction",
    "mashup",
    "parody",
    "tribute",
    "8d",
    "unplugged",
    "extended",
    "perform",
    "performs",
    "performance",
    "concert",
    "tour",
    "session",
    "rehearsal",
    "528hz",
    "432hz",
    "medley",
    "take",
];

/// Words that say nothing about which song a title names.
const NOISE_WORDS: &[&str] = &[
    "feat",
    "ft",
    "with",
    "the",
    "a",
    "remastered",
    "remaster",
    "version",
    "official",
    "audio",
    "video",
    "lyrics",
    "lyric",
    "hd",
    "hq",
    "mv",
    "music",
    "and",
];

/// How far a candidate may run from the reference length before it counts as a different edit.
/// Re-uploads pad a few seconds of silence or intro; a different edit is usually further off.
const DURATION_TOLERANCE_MS: u64 = 10_000;

/// Lowercases, drops punctuation, and pads both ends so a marker can be matched as a whole word:
/// `"Blinding Lights (Live)"` becomes `" blinding lights live "`.
fn padded_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else {
            out.push(' ');
        }
    }
    out.push(' ');
    out
}

fn words(text: &str) -> Vec<String> {
    padded_words(text)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The reference title without catalogue suffixes (`Song - Remastered 2011`) or bracketed credits
/// (`Song (feat. X)`), which uploads format inconsistently.
fn core_title(title: &str) -> String {
    let head = title.split(" - ").next().unwrap_or(title);
    let mut depth = 0usize;
    head.chars()
        .filter(|&ch| match ch {
            '(' | '[' => {
                depth += 1;
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

/// The catalogue track a stream is being found for.
pub struct TrackReference<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    /// 0 when unknown, which disables the length check.
    pub duration_ms: u64,
}

/// True when the candidate names the reference's song, credits one of its artists, and is the same
/// rendition (no live, remix, cover… marker the reference lacks, and every marker it has).
fn names_reference(candidate: &ProviderTrack, reference: &TrackReference) -> bool {
    let meaningful = |w: &String| !NOISE_WORDS.contains(&w.as_str());
    let mut title_words: Vec<String> = words(&core_title(reference.title))
        .into_iter()
        .filter(meaningful)
        .collect();
    if title_words.is_empty() {
        title_words = words(reference.title);
    }
    let artist_words: Vec<String> = words(reference.artist)
        .into_iter()
        .filter(meaningful)
        .collect();

    let candidate_title = words(&candidate.title);
    let mut candidate_words = candidate_title.clone();
    candidate_words.extend(words(&candidate.artist));

    let variants = |ws: &[String]| -> Vec<String> {
        ws.iter()
            .filter(|w| VARIANT_WORDS.contains(&w.as_str()))
            .cloned()
            .collect()
    };
    let reference_variants = variants(&words(reference.title));
    let candidate_variants = variants(&candidate_title);

    title_words.iter().all(|w| candidate_words.contains(w))
        && artist_words.iter().any(|w| candidate_words.contains(w))
        && candidate_variants
            .iter()
            .all(|w| reference_variants.contains(w))
        && reference_variants
            .iter()
            .all(|w| candidate_variants.contains(w))
}

fn within_tolerance(candidate: &ProviderTrack, reference: &TrackReference) -> bool {
    reference.duration_ms == 0
        || candidate.duration_ms == 0
        || candidate.duration_ms.abs_diff(reference.duration_ms) <= DURATION_TOLERANCE_MS
}

/// Picks a candidate from YouTube Music's song catalogue (`songs`) or, failing that, from general
/// video search (`videos`), in search-relevance order.
///
/// Returns `None` rather than the closest-sounding upload when nothing names the reference: a
/// wrong song, live take or cover played silently is worse than an explicit "no match".
/// As a last resort a catalogue song of a different length is accepted, since that is the same
/// release in another edit rather than a fan upload.
fn select_candidate<'a>(
    songs: &'a [ProviderTrack],
    videos: &'a [ProviderTrack],
    reference: &TrackReference,
) -> Option<&'a ProviderTrack> {
    let exact =
        |t: &&ProviderTrack| names_reference(t, reference) && within_tolerance(t, reference);
    songs
        .iter()
        .find(exact)
        .or_else(|| videos.iter().find(exact))
        .or_else(|| songs.iter().find(|t| names_reference(t, reference)))
}

impl YouTubeMusicProvider {
    pub fn new() -> Self {
        Self
    }

    /// Resolves a directly playable audio URL for a video.
    ///
    /// Uses the current public InnerTube client first. yt-dlp is an optional fallback for videos
    /// whose stream is signature-ciphered.
    pub fn resolve_stream(&self, video_id: &str) -> Result<TrackAudioSource, String> {
        if let Ok(source) = resolve_with_inner_tube(video_id) {
            return Ok(source);
        }
        if let Ok(source) = resolve_with_ytdlp(video_id) {
            return Ok(source);
        }
        Err(
            "Could not resolve audio stream. YouTube Music's public player did not expose a direct stream; install yt-dlp for signature-ciphered videos (`brew install yt-dlp`)."
                .to_string(),
        )
    }

    /// Resolves an audio stream URL for a given artist and title (e.g. for Spotify track fallback).
    ///
    /// `target_duration_ms` is the length of the track being matched; pass 0 when it is unknown,
    /// which disables the duration check.
    pub fn resolve_stream_for_track(
        &self,
        title: &str,
        artist: &str,
        target_duration_ms: u64,
    ) -> Result<String, String> {
        let query = format!("{artist} {title}");
        let reference = TrackReference {
            title,
            artist,
            duration_ms: target_duration_ms,
        };
        let songs = search_with_inner_tube(&query, 5)
            .map(|r| r.tracks)
            .unwrap_or_default();

        // yt-dlp search costs over a second, so it only runs when the catalogue has no exact match.
        let exact_song = songs
            .iter()
            .find(|t| names_reference(t, &reference) && within_tolerance(t, &reference));
        let videos = match exact_song {
            Some(_) => Vec::new(),
            None => search_with_ytdlp(&query, 5)
                .map(|r| r.tracks)
                .unwrap_or_default(),
        };
        if songs.is_empty() && videos.is_empty() {
            return Err(format!("YouTube Music search failed for '{query}'"));
        }

        let track = select_candidate(&songs, &videos, &reference)
            .ok_or_else(|| format!("No confident YouTube match for '{title}' by {artist}"))?;
        let video_id = track.id.trim_start_matches("ytmusic://track/");
        self.resolve_stream(video_id).map(|s| s.url)
    }
}

fn search_with_inner_tube(query: &str, limit: usize) -> Result<SearchResults, String> {
    // `EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D` is the songs-only filter the web UI uses.
    let inner_result = inner_tube_post(
        "search",
        json!({ "query": query, "params": "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D" }),
    );

    if let Ok(value) = &inner_result {
        let mut tracks: Vec<ProviderTrack> = renderers(value, "musicResponsiveListItemRenderer")
            .iter()
            .filter_map(|r| parse_song(r))
            .collect();
        tracks.dedup_by(|a, b| a.id == b.id);
        tracks.truncate(limit);

        if !tracks.is_empty() {
            let playlists = renderers(value, "musicTwoRowItemRenderer")
                .iter()
                .filter_map(|r| parse_playlist(r))
                .take(limit)
                .collect();
            return Ok(SearchResults { tracks, playlists });
        }
    }

    Err("InnerTube search returned no tracks".to_string())
}

fn resolve_with_inner_tube(video_id: &str) -> Result<TrackAudioSource, String> {
    let (api_key, version) = web_remix_config()?;
    let body = json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "context": {
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": version,
                "hl": "en",
                "gl": "US",
            }
        }
    });

    let response = client()
        .post(endpoint("player", &api_key))
        .json(&body)
        .send()
        .map_err(|e| format!("stream resolution failed: {e}"))?;

    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("YouTube Music player returned {status}"));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("unreadable YouTube Music player response: {e}"))?;
    let formats = value
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|f| f.as_array())
        .ok_or_else(|| "YouTube Music returned no adaptive audio formats".to_string())?;
    let best = formats
        .iter()
        .filter(|f| {
            f.get("mimeType")
                .and_then(|m| m.as_str())
                .is_some_and(|m| m.starts_with("audio/"))
        })
        .filter_map(|f| {
            let url = f.get("url").and_then(|u| u.as_str())?;
            let bitrate = f.get("bitrate").and_then(|b| b.as_u64()).unwrap_or(0);
            Some((bitrate, url.to_string(), f))
        })
        .max_by_key(|(bitrate, _, _)| *bitrate)
        .ok_or_else(|| "YouTube Music returned no direct audio URL".to_string())?;
    Ok(TrackAudioSource {
        url: best.1,
        mime_type: best
            .2
            .get("mimeType")
            .and_then(|m| m.as_str())
            .map(str::to_string),
        remote_playback: false,
    })
}

impl Default for YouTubeMusicProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MusicProvider for YouTubeMusicProvider {
    fn provider_id(&self) -> ProviderKind {
        ProviderKind::YouTubeMusic
    }

    async fn search(&self, query: &str, limit: usize) -> Result<SearchResults, String> {
        let query = query.to_string();
        tokio::task::spawn_blocking(move || {
            search_with_inner_tube(&query, limit).or_else(|_| search_with_ytdlp(&query, limit))
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_track_stream(&self, track_id: &str) -> Result<TrackAudioSource, String> {
        let video_id = track_id.trim_start_matches("ytmusic://track/").to_string();
        tokio::task::spawn_blocking(move || YouTubeMusicProvider::new().resolve_stream(&video_id))
            .await
            .map_err(|e| e.to_string())?
    }

    async fn get_user_playlists(&self) -> Result<Vec<ProviderPlaylist>, String> {
        // YouTube Music playlists require an authenticated Google session, which Sonora does not
        // hold; the caller falls back to search results and local playlists.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations_in_both_shapes() {
        assert_eq!(parse_duration("3:45"), 225_000);
        assert_eq!(parse_duration("1:02:03"), 3_723_000);
        assert_eq!(parse_duration("0:07"), 7_000);
        assert_eq!(parse_duration("not a time"), 0);
    }

    #[test]
    fn reads_rotating_web_player_configuration_values() {
        let page = r#"{"INNERTUBE_API_KEY":"key-123","INNERTUBE_CLIENT_VERSION":"1.2.3"}"#;
        assert_eq!(
            bootstrap_value(page, "INNERTUBE_API_KEY").as_deref(),
            Some("key-123")
        );
        assert_eq!(
            bootstrap_value(page, "INNERTUBE_CLIENT_VERSION").as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn joins_split_text_runs() {
        let value = json!({ "runs": [{ "text": "YOASO" }, { "text": "BI" }] });
        assert_eq!(runs_text(&value).as_deref(), Some("YOASOBI"));

        let blank = json!({ "runs": [{ "text": "   " }] });
        assert_eq!(runs_text(&blank), None);
    }

    #[test]
    fn finds_renderers_nested_anywhere_in_the_payload() {
        let payload = json!({
            "contents": {
                "a": { "musicResponsiveListItemRenderer": { "id": 1 } },
                "b": [ { "c": { "musicResponsiveListItemRenderer": { "id": 2 } } } ]
            }
        });
        let found = renderers(&payload, "musicResponsiveListItemRenderer");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn parses_a_song_row_including_its_video_id() {
        let renderer = json!({
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Yoru ni Kakeru" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "YOASOBI • THE BOOK • 4:21" }] } } }
            ],
            "overlay": {
                "musicItemThumbnailOverlayRenderer": {
                    "content": {
                        "musicPlayButtonRenderer": {
                            "playNavigationEndpoint": { "watchEndpoint": { "videoId": "x8VYWazR5mE" } }
                        }
                    }
                }
            },
            "thumbnail": {
                "musicThumbnailRenderer": {
                    "thumbnail": { "thumbnails": [{ "url": "https://lh3.googleusercontent.com/small" },
                                                   { "url": "https://lh3.googleusercontent.com/large" }] }
                }
            }
        });

        let track = parse_song(&renderer).unwrap();
        assert_eq!(track.id, "ytmusic://track/x8VYWazR5mE");
        assert_eq!(track.title, "Yoru ni Kakeru");
        assert_eq!(track.artist, "YOASOBI");
        assert_eq!(track.album, "THE BOOK");
        assert_eq!(track.duration_ms, 261_000);
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://lh3.googleusercontent.com/large")
        );
    }

    #[test]
    fn parses_a_playlist_row_from_its_browse_endpoint() {
        let renderer = json!({
            "navigationEndpoint": { "browseEndpoint": { "browseId": "VLPL123" } },
            "title": { "runs": [{ "text": "My Mix" }] }
        });
        let playlist = parse_playlist(&renderer).unwrap();
        assert_eq!(playlist.id, "ytmusic://playlist/VLPL123");
        assert_eq!(playlist.title, "My Mix");
    }

    #[test]
    fn a_row_without_a_video_id_is_skipped() {
        let renderer = json!({
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Header" }] } } }
            ]
        });
        assert!(parse_song(&renderer).is_none());
    }

    #[test]
    fn parses_a_ytdlp_search_entry() {
        let entry = json!({
            "id": "5NV6Rdv1a3I",
            "title": "Daft Punk - Get Lucky",
            "uploader": "Daft Punk",
            "duration": 249
        });
        let track = parse_ytdlp_search_entry(&entry).unwrap();
        assert_eq!(track.id, "ytmusic://track/5NV6Rdv1a3I");
        assert_eq!(track.artist, "Daft Punk");
        assert_eq!(track.duration_ms, 249_000);
        assert!(track.artwork_url.is_some());
    }

    fn candidate(title: &str, channel: &str, duration_ms: u64) -> ProviderTrack {
        ProviderTrack {
            id: format!("ytmusic://track/{title}"),
            provider: ProviderKind::YouTubeMusic,
            title: title.to_string(),
            artist: channel.to_string(),
            album: String::new(),
            duration_ms,
            artwork_url: None,
        }
    }

    fn reference<'a>(title: &'a str, artist: &'a str, duration_ms: u64) -> TrackReference<'a> {
        TrackReference {
            title,
            artist,
            duration_ms,
        }
    }

    #[test]
    fn prefers_the_catalogue_song_over_video_uploads() {
        let songs = vec![
            candidate("Kyoto", "Yung Lean", 270_000),
            candidate("Ginseng Strip 2002", "Yung Lean", 154_000),
        ];
        let videos = vec![candidate(
            "Yung Lean ♦ Ginseng Strip 2002 ♦",
            "Yung Lean",
            159_000,
        )];
        let picked = select_candidate(
            &songs,
            &videos,
            &reference("Ginseng Strip 2002", "Yung Lean", 153_000),
        );
        assert_eq!(picked.unwrap().title, "Ginseng Strip 2002");
    }

    #[test]
    fn never_substitutes_an_instrumental_or_live_take_for_the_studio_track() {
        let videos = vec![
            candidate(
                "Yung lean - Ginseng strip 2002 (instrumental)",
                "Radio Silence",
                151_000,
            ),
            candidate(
                "Coma Cose - Mancarsi LIVE @ Filagosto Festival 2019",
                "FilagostoTV",
                154_000,
            ),
        ];
        assert!(select_candidate(
            &[],
            &videos,
            &reference("Ginseng Strip 2002", "Yung Lean", 153_000)
        )
        .is_none());
        assert!(
            select_candidate(&[], &videos, &reference("MANCARSI", "Coma_Cose", 229_000)).is_none()
        );
    }

    #[test]
    fn a_variant_word_in_the_reference_title_is_required_not_rejected() {
        let videos = vec![
            candidate("Live Forever (Acoustic)", "Fan Uploads", 274_000),
            candidate("Oasis - Live Forever", "Oasis - Topic", 276_000),
        ];
        let picked = select_candidate(&[], &videos, &reference("Live Forever", "Oasis", 276_000));
        assert_eq!(picked.unwrap().title, "Oasis - Live Forever");

        let videos = vec![
            candidate(
                "Adele - Someone Like You (Official Music Video)",
                "Adele",
                285_000,
            ),
            candidate(
                "Adele - Someone Like You (Live at The Royal Albert Hall)",
                "Adele",
                300_000,
            ),
        ];
        let live = reference(
            "Someone Like You - Live at the Royal Albert Hall",
            "Adele",
            297_000,
        );
        assert!(select_candidate(&[], &videos, &live)
            .unwrap()
            .title
            .contains("Live"));
    }

    #[test]
    fn rejects_a_different_song_of_the_right_length() {
        let videos = vec![candidate(
            "Riblja Čorba - Dva dinara, druže",
            "Riblja Čorba",
            203_000,
        )];
        let fabricated = reference("Zvezda nad Dunavom", "Riblja Čorba", 200_000);
        assert!(select_candidate(&[], &videos, &fabricated).is_none());
    }

    #[test]
    fn a_catalogue_song_of_another_edit_beats_no_match_but_a_video_does_not() {
        let songs = vec![candidate("Applausi Per Fibra", "Fabri Fibra", 242_000)];
        let videos = vec![candidate(
            "Fabri Fibra - Applausi Per Fibra",
            "Fabri Fibra",
            250_000,
        )];
        let platinum = reference("Applausi Per Fibra", "Fabri Fibra", 294_000);
        assert_eq!(
            select_candidate(&songs, &videos, &platinum)
                .unwrap()
                .duration_ms,
            242_000
        );
        assert!(select_candidate(&[], &videos, &platinum).is_none());
    }

    #[test]
    fn matches_credits_and_suffixes_the_catalogue_formats_differently() {
        let songs = vec![candidate(
            "That's It [from GTAVI: The Album] (feat. Future & Metro Boomin)",
            "Yung Lean & Grand Theft Auto VI",
            162_000,
        )];
        let spotify = reference(
            "That's It (feat. Future & Metro Boomin) [from GTAVI: The Album]",
            "Yung Lean, Grand Theft Auto VI, Metro Boomin, Future",
            161_000,
        );
        assert!(select_candidate(&songs, &[], &spotify).is_some());
        assert_eq!(core_title("Money - Remastered 2011"), "Money");
    }

    #[test]
    #[ignore]
    fn live_resolve_stream_for_track() {
        let provider = YouTubeMusicProvider::new();
        for (title, artist, duration_ms) in [
            ("Буйно голова", "Gio Pika", 128_000),
            ("MANCARSI", "Coma_Cose", 229_000),
            ("rockstar (feat. 21 Savage)", "Post Malone, 21 Savage", 218_000),
        ] {
            let started = std::time::Instant::now();
            let stream_url = provider
                .resolve_stream_for_track(title, artist, duration_ms)
                .expect("stream resolved for track");
            assert!(
                stream_url.starts_with("http"),
                "valid http url: {stream_url}"
            );
            println!("{title}: resolved in {:?}", started.elapsed());
        }
    }
}
