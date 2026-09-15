//! Synced lyrics resolution: embedded tags first, then LRCLIB and NetEase, with the result and its
//! romanization cached in SQLite so a track is only ever fetched once.

pub mod romanize;

use crate::db::DbState;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
    pub romanized_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedLyrics {
    pub track_id: String,
    pub lines: Vec<LyricLine>,
    pub has_romanization: bool,
    pub provider: LyricsProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsProvider {
    Embedded,
    Lrclib,
    Netease,
    None,
}

fn parse_timestamp_tag(tag: &str) -> Option<u64> {
    let (min_str, sec_part) = tag.split_once(':')?;
    let minutes: u64 = min_str.parse().ok()?;
    if min_str.len() > 3 || minutes > 999 {
        return None;
    }
    let (sec_str, frac_str) = if let Some((s, f)) = sec_part
        .split_once('.')
        .or_else(|| sec_part.split_once(':'))
    {
        (s, Some(f))
    } else {
        (sec_part, None)
    };
    if sec_str.len() != 2 {
        return None;
    }
    let seconds: u64 = sec_str.parse().ok()?;
    if seconds >= 60 {
        return None;
    }
    let millis: u64 = match frac_str {
        Some(f) if f.len() == 1 => f.parse::<u64>().ok()? * 100,
        Some(f) if f.len() == 2 => f.parse::<u64>().ok()? * 10,
        Some(f) if f.len() >= 3 => f[..3].parse::<u64>().ok()?,
        _ => 0,
    };
    Some(minutes * 60_000 + seconds * 1_000 + millis)
}

fn extract_lrc_line(raw_line: &str) -> (Vec<u64>, String) {
    let mut stamps = Vec::new();
    let mut remainder = raw_line.trim();
    while let Some(start) = remainder.find('[') {
        if let Some(end) = remainder[start..].find(']') {
            let tag = &remainder[start + 1..start + end];
            if let Some(ms) = parse_timestamp_tag(tag) {
                stamps.push(ms);
                remainder = &remainder[start + end + 1..];
                continue;
            }
        }
        break;
    }
    (stamps, remainder.trim().to_string())
}

/// Parses LRC text into timestamped lines, adding romanization for non-Latin scripts.
///
/// A single physical line may carry several timestamps (a repeated chorus), so every stamp
/// produces its own entry.
pub fn parse_lrc(track_id: &str, lrc_content: &str, provider: LyricsProvider) -> ParsedLyrics {
    let mut lines = Vec::new();
    let mut has_romanization = false;

    for raw_line in lrc_content.lines() {
        let (stamps, text) = extract_lrc_line(raw_line);
        if stamps.is_empty() || text.is_empty() {
            continue;
        }

        let script = romanize::detect_script(&text);
        let romanized = match script {
            romanize::Script::Latin | romanize::Script::Other => None,
            _ => {
                let converted = romanize::romanize(&text);
                // Only keep the conversion when it actually changed something.
                (converted != text).then_some(converted)
            }
        };
        has_romanization |= romanized.is_some();

        for time_ms in stamps {
            lines.push(LyricLine {
                time_ms,
                text: text.clone(),
                romanized_text: romanized.clone(),
            });
        }
    }

    lines.sort_by_key(|l| l.time_ms);
    lines.dedup_by_key(|l| l.time_ms);

    ParsedLyrics {
        track_id: track_id.to_string(),
        lines,
        has_romanization,
        provider,
    }
}

/// Plain (unsynced) lyrics are not useful to a scrolling view, but they are still better than
/// nothing, so they are laid out evenly across the track duration.
pub fn lay_out_plain_lyrics(
    track_id: &str,
    text: &str,
    duration_ms: u64,
    provider: LyricsProvider,
) -> ParsedLyrics {
    let raw: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if raw.is_empty() {
        return ParsedLyrics {
            track_id: track_id.to_string(),
            lines: Vec::new(),
            has_romanization: false,
            provider: LyricsProvider::None,
        };
    }

    let step = duration_ms.max(1) / raw.len() as u64;
    let mut lines = Vec::with_capacity(raw.len());
    let mut has_romanization = false;
    for (index, line) in raw.iter().enumerate() {
        let script = romanize::detect_script(line);
        let romanized = match script {
            romanize::Script::Latin | romanize::Script::Other => None,
            _ => {
                let converted = romanize::romanize(line);
                (converted != *line).then_some(converted)
            }
        };
        has_romanization |= romanized.is_some();
        lines.push(LyricLine {
            time_ms: index as u64 * step,
            text: (*line).to_string(),
            romanized_text: romanized,
        });
    }

    ParsedLyrics {
        track_id: track_id.to_string(),
        lines,
        has_romanization,
        provider,
    }
}

#[derive(Deserialize)]
struct LrclibResponse {
    #[serde(rename = "syncedLyrics")]
    synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    plain_lyrics: Option<String>,
}

pub struct LyricsRequest<'a> {
    pub track_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub duration_ms: u64,
    /// Lyrics embedded in the file's tags, checked before any network call.
    pub embedded: Option<String>,
}

/// Resolves lyrics for a track: SQLite cache, then embedded tags, then LRCLIB and NetEase.
/// Successful network lookups are written back to the cache.
pub fn resolve_lyrics(db: &DbState, request: LyricsRequest<'_>) -> Result<ParsedLyrics, String> {
    if let Some((raw_lrc, romanized_json, provider_name)) =
        db.get_lyrics(request.track_id).ok().flatten()
    {
        let provider = match provider_name.as_str() {
            "embedded" => LyricsProvider::Embedded,
            "netease" => LyricsProvider::Netease,
            "none" => LyricsProvider::None,
            _ => LyricsProvider::Lrclib,
        };
        if let Some(json) = romanized_json {
            if let Ok(cached) = serde_json::from_str::<Vec<LyricLine>>(&json) {
                let has_romanization = cached.iter().any(|l| l.romanized_text.is_some());
                return Ok(ParsedLyrics {
                    track_id: request.track_id.to_string(),
                    lines: cached,
                    has_romanization,
                    provider,
                });
            }
        }
        return Ok(parse_lrc(request.track_id, &raw_lrc, provider));
    }

    if let Some(embedded) = request.embedded.as_deref().filter(|t| !t.trim().is_empty()) {
        let parsed = if embedded.lines().any(|l| !extract_lrc_line(l).0.is_empty()) {
            parse_lrc(request.track_id, embedded, LyricsProvider::Embedded)
        } else {
            lay_out_plain_lyrics(
                request.track_id,
                embedded,
                request.duration_ms,
                LyricsProvider::Embedded,
            )
        };
        if !parsed.lines.is_empty() {
            cache(db, &parsed, embedded);
            return Ok(parsed);
        }
    }

    let fetched =
        fetch_from_lrclib(&request)?.or_else(|| fetch_from_netease(&request).ok().flatten());
    let Some((raw, parsed)) = fetched else {
        return Ok(ParsedLyrics {
            track_id: request.track_id.to_string(),
            lines: Vec::new(),
            has_romanization: false,
            provider: LyricsProvider::None,
        });
    };
    cache(db, &parsed, &raw);
    Ok(parsed)
}

fn cache(db: &DbState, parsed: &ParsedLyrics, raw: &str) {
    let json = serde_json::to_string(&parsed.lines).unwrap_or_default();
    let provider = match parsed.provider {
        LyricsProvider::Embedded => "embedded",
        LyricsProvider::Lrclib => "lrclib",
        LyricsProvider::Netease => "netease",
        LyricsProvider::None => "none",
    };
    if let Err(err) = db.put_lyrics(&parsed.track_id, raw, Some(&json), provider) {
        eprintln!("[sonora] failed to cache lyrics: {err}");
    }
}

fn fetch_from_lrclib(
    request: &LyricsRequest<'_>,
) -> Result<Option<(String, ParsedLyrics)>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Sonora/0.1.0 (https://github.com/nodaysidle/sonora)")
        .build()
        .map_err(|e| e.to_string())?;

    let seconds = request.duration_ms / 1000;
    let attempt = |include_duration: bool| -> Option<LrclibResponse> {
        let mut query = vec![
            ("track_name", request.title.to_string()),
            ("artist_name", request.artist.to_string()),
        ];
        if let Some(album) = request.album {
            query.push(("album_name", album.to_string()));
        }
        if include_duration {
            query.push(("duration", seconds.to_string()));
        }
        client
            .get("https://lrclib.net/api/get")
            .query(&query)
            .send()
            .ok()?
            .json()
            .ok()
    };

    // LRCLIB is strict about duration; retry without it when the exact match misses.
    let response = match attempt(true) {
        Some(response) => Some(response),
        None => attempt(false),
    };

    let Some(response) = response else {
        return Ok(None);
    };

    if let Some(synced) = response.synced_lyrics.filter(|s| !s.trim().is_empty()) {
        let parsed = parse_lrc(request.track_id, &synced, LyricsProvider::Lrclib);
        if !parsed.lines.is_empty() {
            return Ok(Some((synced, parsed)));
        }
    }
    if let Some(plain) = response.plain_lyrics.filter(|s| !s.trim().is_empty()) {
        let parsed = lay_out_plain_lyrics(
            request.track_id,
            &plain,
            request.duration_ms,
            LyricsProvider::Lrclib,
        );
        if !parsed.lines.is_empty() {
            return Ok(Some((plain, parsed)));
        }
    }

    Ok(None)
}

fn fetch_from_netease(
    request: &LyricsRequest<'_>,
) -> Result<Option<(String, ParsedLyrics)>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("Sonora/0.1.0 (https://github.com/nodaysidle/sonora)")
        .build()
        .map_err(|e| e.to_string())?;
    let query = format!("{} {}", request.title, request.artist);
    let search = client
        .get("https://music.163.com/api/search/get/web")
        .query(&[
            ("s", query),
            ("type", "1".to_string()),
            ("offset", "0".to_string()),
            ("limit", "5".to_string()),
        ])
        .send()
        .ok()
        .and_then(|response| response.json::<serde_json::Value>().ok());
    let Some(search) = search else {
        return Ok(None);
    };
    let title = request.title.trim().to_lowercase();
    let artist = request.artist.trim().to_lowercase();
    let songs = search.pointer("/result/songs").and_then(|s| s.as_array());
    let Some(song) = songs.and_then(|songs| {
        songs
            .iter()
            .find(|song| {
                let same_title = song
                    .get("name")
                    .and_then(|name| name.as_str())
                    .is_some_and(|name| name.trim().to_lowercase() == title);
                let same_artist = song
                    .get("ar")
                    .and_then(|artists| artists.as_array())
                    .map(|artists| {
                        artists.iter().any(|item| {
                            item.get("name")
                                .and_then(|name| name.as_str())
                                .is_some_and(|name| name.to_lowercase().contains(&artist))
                        })
                    })
                    .unwrap_or(false);
                same_title && same_artist
            })
            .or_else(|| songs.first())
    }) else {
        return Ok(None);
    };
    let Some(song_id) = song.get("id").and_then(|id| id.as_u64()) else {
        return Ok(None);
    };

    let value = client
        .get("https://music.163.com/api/song/lyric")
        .query(&[
            ("id", song_id.to_string()),
            ("lv", "1".to_string()),
            ("kv", "1".to_string()),
            ("tv", "-1".to_string()),
        ])
        .send()
        .ok()
        .and_then(|response| response.json::<serde_json::Value>().ok());
    let Some(raw) = value
        .as_ref()
        .and_then(|value| value.pointer("/lrc/lyric"))
        .and_then(|lyric| lyric.as_str())
        .filter(|lyric| !lyric.trim().is_empty())
    else {
        return Ok(None);
    };
    let parsed = parse_lrc(request.track_id, raw, LyricsProvider::Netease);
    if parsed.lines.is_empty() {
        Ok(None)
    } else {
        Ok(Some((raw.to_string(), parsed)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_and_three_digit_fractions() {
        let parsed = parse_lrc(
            "t",
            "[00:12.34]First\n[00:15.678]Second",
            LyricsProvider::Lrclib,
        );
        assert_eq!(parsed.lines.len(), 2);
        assert_eq!(parsed.lines[0].time_ms, 12_340);
        assert_eq!(parsed.lines[1].time_ms, 15_678);
        assert_eq!(parsed.lines[1].text, "Second");
    }

    #[test]
    fn expands_lines_carrying_several_timestamps() {
        let parsed = parse_lrc("t", "[00:10.00][00:40.00]Chorus", LyricsProvider::Lrclib);
        assert_eq!(parsed.lines.len(), 2);
        assert_eq!(parsed.lines[0].time_ms, 10_000);
        assert_eq!(parsed.lines[1].time_ms, 40_000);
        assert!(parsed.lines.iter().all(|l| l.text == "Chorus"));
    }

    #[test]
    fn romanizes_non_latin_lines_and_flags_them() {
        let parsed = parse_lrc(
            "t",
            "[00:01.00]夜に駆ける\n[00:05.00]Just English",
            LyricsProvider::Lrclib,
        );
        assert!(parsed.has_romanization);
        assert!(parsed.lines[0].romanized_text.is_some());
        assert_eq!(
            parsed.lines[1].romanized_text, None,
            "Latin needs no conversion"
        );
    }

    #[test]
    fn metadata_tags_are_not_treated_as_lyric_lines() {
        let parsed = parse_lrc(
            "t",
            "[ar:YOASOBI]\n[ti:Yoru ni Kakeru]\n[00:01.00]Real line",
            LyricsProvider::Lrclib,
        );
        assert_eq!(parsed.lines.len(), 1);
        assert_eq!(parsed.lines[0].text, "Real line");
    }

    #[test]
    fn plain_lyrics_are_spread_across_the_track() {
        let parsed = lay_out_plain_lyrics("t", "one\ntwo\nthree", 30_000, LyricsProvider::Netease);
        assert_eq!(parsed.lines.len(), 3);
        assert_eq!(parsed.lines[0].time_ms, 0);
        assert_eq!(parsed.lines[1].time_ms, 10_000);
        assert_eq!(parsed.lines[2].time_ms, 20_000);
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert!(parse_lrc("t", "", LyricsProvider::Lrclib).lines.is_empty());
        assert!(
            lay_out_plain_lyrics("t", "   \n\n", 1000, LyricsProvider::Netease)
                .lines
                .is_empty()
        );
    }
}
