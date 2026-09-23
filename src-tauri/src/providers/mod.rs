//! Unified provider surface. Local files, Spotify and YouTube Music all hand back the same
//! `ProviderTrack`, so the queue and library views never branch on source.

pub mod spotify;
pub mod ytmusic;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Serialized names match the `provider` column stored on every `tracks` row, so a Spotify track
/// coming from the API and one coming back out of SQLite carry the identical tag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProviderKind {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "spotify")]
    Spotify,
    #[serde(rename = "ytmusic")]
    YouTubeMusic,
}

impl ProviderKind {
    /// Canonical URI scheme from ARD.md §3.2.
    pub fn scheme(&self) -> &'static str {
        match self {
            ProviderKind::Local => "local",
            ProviderKind::Spotify => "spotify",
            ProviderKind::YouTubeMusic => "ytmusic",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTrack {
    pub id: String,
    pub provider: ProviderKind,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPlaylist {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub track_count: u32,
    pub provider: ProviderKind,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub tracks: Vec<ProviderTrack>,
    pub playlists: Vec<ProviderPlaylist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackAudioSource {
    /// Direct, seekable URL that the Rust decoder can read over HTTP.
    pub url: String,
    pub mime_type: Option<String>,
    /// Set when a provider owns playback instead of exposing a stream to the generic decoder.
    pub remote_playback: bool,
}

#[async_trait]
pub trait MusicProvider: Send + Sync {
    fn provider_id(&self) -> ProviderKind;
    async fn search(&self, query: &str, limit: usize) -> Result<SearchResults, String>;
    async fn get_track_stream(&self, track_id: &str) -> Result<TrackAudioSource, String>;
    async fn get_user_playlists(&self) -> Result<Vec<ProviderPlaylist>, String>;
}

/// Spotify's Web API serves library metadata; native playback is owned by Librespot rather than
/// the generic Symphonia decoder.
pub fn spotify_cannot_decode_locally() -> &'static str {
    "Spotify audio is handled by Sonora's native Spotify player"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_schemes_match_the_canonical_uris() {
        assert_eq!(ProviderKind::Local.scheme(), "local");
        assert_eq!(ProviderKind::Spotify.scheme(), "spotify");
        assert_eq!(ProviderKind::YouTubeMusic.scheme(), "ytmusic");
    }

    #[test]
    fn provider_kind_matches_the_stored_provider_column() {
        for (kind, stored) in [
            (ProviderKind::Local, "local"),
            (ProviderKind::Spotify, "spotify"),
            (ProviderKind::YouTubeMusic, "ytmusic"),
        ] {
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{stored}\"")
            );
            assert_eq!(kind.scheme(), stored);
        }
    }
}
