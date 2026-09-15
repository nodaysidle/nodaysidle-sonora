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
use std::time::Duration;

fn endpoint(path: &str, api_key: &str) -> String {
    format!("https://music.youtube.com/youtubei/v1/{path}?key={api_key}&prettyPrint=false")
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 14)")
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
            // `EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D` is the songs-only filter the web UI uses.
            let inner_result = inner_tube_post(
                "search",
                json!({ "query": query, "params": "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D" }),
            );

            if let Ok(value) = &inner_result {
                let mut tracks: Vec<ProviderTrack> =
                    renderers(value, "musicResponsiveListItemRenderer")
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

            // The web payload is undocumented and can temporarily omit its config or return an
            // empty shape. yt-dlp is the existing optional fallback for the same public catalog.
            search_with_ytdlp(&query, limit).map_err(|fallback_error| {
                inner_result
                    .err()
                    .map(|inner_error| {
                        format!("{inner_error}; yt-dlp fallback failed: {fallback_error}")
                    })
                    .unwrap_or(fallback_error)
            })
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
}
