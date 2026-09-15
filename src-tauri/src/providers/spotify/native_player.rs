//! Native in-process Spotify audio player engine using `librespot`.
//!
//! Connects directly to Spotify AP servers using PKCE OAuth tokens, decodes 320 kbps Vorbis
//! streams in-process, and renders audio through the rodio/CPAL backend directly on macOS CoreAudio
//! without needing the official Spotify Desktop app or Safari open.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use librespot::core::{
    authentication::Credentials,
    cache::Cache,
    config::SessionConfig,
    session::Session,
    spotify_id::SpotifyId,
    spotify_uri::SpotifyUri,
};
use librespot::metadata::{Metadata, Track};
use librespot::playback::{
    audio_backend,
    config::{AudioFormat, Bitrate, NormalisationMethod, NormalisationType, PlayerConfig},
    mixer::{Mixer, MixerConfig},
    player::{Player, PlayerEvent},
};

use super::SpotifyPlaybackState;
use crate::providers::{ProviderKind, ProviderTrack};

/// Normalizes various Spotify URI and URL representations down to a canonical 22-character
/// base62 Spotify track ID.
pub fn normalize_spotify_id(uri: &str) -> Result<String, String> {
    let raw = uri.trim();
    if raw.is_empty() {
        return Err("Empty Spotify URI or track ID".to_string());
    }

    let id = if let Some(stripped) = raw.strip_prefix("spotify://track/") {
        stripped
    } else if let Some(stripped) = raw.strip_prefix("spotify:track:") {
        stripped
    } else if let Some(idx) = raw.find("/track/") {
        &raw[idx + "/track/".len()..]
    } else {
        raw
    };

    let id = id
        .split('?')
        .next()
        .unwrap_or(id)
        .split('#')
        .next()
        .unwrap_or(id)
        .trim();

    if id.is_empty() {
        return Err("Missing Spotify track ID".to_string());
    }

    // Verify valid base62 ID using librespot's SpotifyId
    SpotifyId::from_base62(id)
        .map(|_| id.to_string())
        .map_err(|_| format!("Invalid Spotify track ID '{id}': must be 22-character base62"))
}

type ActiveSessionTuple = (Session, Arc<Player>, Arc<dyn Mixer>);

struct ActiveSession {
    session: Session,
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,
    _event_task: tokio::task::JoinHandle<()>,
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.player.stop();
        self._event_task.abort();
    }
}

struct NativePlayerInner {
    state: Mutex<SpotifyPlaybackState>,
    last_position_update: Mutex<Option<(Instant, u64)>>,
    active_session: Mutex<Option<ActiveSession>>,
    cache_dir: Option<PathBuf>,
    client_id: Mutex<Option<String>>,
}

impl NativePlayerInner {
    fn handle_player_event(&self, event: PlayerEvent) {
        let mut state = self.state.lock().unwrap();
        let mut last_pos = self.last_position_update.lock().unwrap();

        match event {
            PlayerEvent::Playing { position_ms, .. } => {
                state.is_playing = true;
                state.progress_ms = position_ms as u64;
                *last_pos = Some((Instant::now(), position_ms as u64));
            }
            PlayerEvent::Paused { position_ms, .. } => {
                state.is_playing = false;
                state.progress_ms = position_ms as u64;
                *last_pos = None;
            }
            PlayerEvent::Stopped { .. } | PlayerEvent::EndOfTrack { .. } => {
                state.is_playing = false;
                *last_pos = None;
            }
            PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. } => {
                state.progress_ms = position_ms as u64;
                if state.is_playing {
                    *last_pos = Some((Instant::now(), position_ms as u64));
                }
            }
            PlayerEvent::VolumeChanged { volume } => {
                let percent = (volume as f32 / 65535.0 * 100.0).round() as u8;
                state.volume_percent = Some(percent);
            }
            PlayerEvent::Unavailable { .. } => {
                state.is_playing = false;
                *last_pos = None;
            }
            _ => {}
        }
    }

    fn update_track_metadata(&self, meta: Track) {
        let mut state = self.state.lock().unwrap();
        state.duration_ms = meta.duration.max(0) as u64;
        if let Some(ref mut track) = state.track {
            track.title = meta.name;
            track.album = meta.album.name;
            track.artist = meta
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            track.duration_ms = meta.duration.max(0) as u64;
        }
    }
}

/// In-process native Spotify audio player.
#[derive(Clone)]
pub struct NativeSpotifyPlayer {
    inner: Arc<NativePlayerInner>,
}

impl Default for NativeSpotifyPlayer {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl NativeSpotifyPlayer {
    pub fn new(cache_dir: Option<PathBuf>, client_id: Option<String>) -> Self {
        let initial_state = SpotifyPlaybackState {
            track: None,
            is_playing: false,
            progress_ms: 0,
            duration_ms: 0,
            volume_percent: Some(100),
            device_name: Some("Sonora (Native)".to_string()),
        };

        Self {
            inner: Arc::new(NativePlayerInner {
                state: Mutex::new(initial_state),
                last_position_update: Mutex::new(None),
                active_session: Mutex::new(None),
                cache_dir,
                client_id: Mutex::new(client_id),
            }),
        }
    }

    pub fn set_client_id(&self, client_id: String) {
        let mut guard = self.inner.client_id.lock().unwrap();
        *guard = (!client_id.trim().is_empty()).then_some(client_id);
    }

    pub fn client_id(&self) -> Option<String> {
        self.inner.client_id.lock().unwrap().clone()
    }

    pub fn effective_client_id(&self) -> Result<String, String> {
        self.inner
            .client_id
            .lock()
            .unwrap()
            .clone()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "Configure your Spotify Client ID in Settings first.".to_string())
    }

    fn obtain_access_token(&self) -> Result<String, String> {
        let client_id = self.effective_client_id()?;
        super::access_token(&client_id)
    }

    fn ensure_active_session(&self) -> Result<ActiveSessionTuple, String> {
        let mut lock = self.inner.active_session.lock().map_err(|e| e.to_string())?;
        if let Some(active) = lock.as_ref() {
            if !active.session.is_invalid() && !active.player.is_invalid() {
                return Ok((active.session.clone(), active.player.clone(), active.mixer.clone()));
            }
        }

        let token = self.obtain_access_token()?;
        let credentials = Credentials::with_access_token(token);
        let client_id = self.effective_client_id()?;
        let session_config = SessionConfig {
            client_id,
            ..Default::default()
        };

        let cache = self.inner.cache_dir.as_ref().and_then(|dir| {
            let creds_dir = dir.join("credentials");
            let vol_dir = dir.join("volume");
            let audio_dir = dir.join("audio");
            Cache::new(Some(creds_dir), Some(vol_dir), Some(audio_dir), Some(500 * 1024 * 1024)).ok()
        });

        let player_config = PlayerConfig {
            bitrate: Bitrate::Bitrate320,
            normalisation: true,
            normalisation_type: NormalisationType::Album,
            normalisation_method: NormalisationMethod::Dynamic,
            position_update_interval: Some(Duration::from_millis(250)),
            ..Default::default()
        };

        let current_vol_pct = self.inner.state.lock().unwrap().volume_percent.unwrap_or(100);
        let inner_weak = Arc::downgrade(&self.inner);

        let (session, player, mixer, event_task) = run_async_block(async move {
            let session = Session::new(session_config, cache);
            session
                .connect(credentials, true)
                .await
                .map_err(|e| format!("Failed to connect Spotify session: {e}"))?;

            let mixer_fn = librespot::playback::mixer::find(None)
                .ok_or_else(|| "Failed to find audio mixer".to_string())?;
            let mixer = mixer_fn(MixerConfig::default())
                .map_err(|e| format!("Failed to open soft mixer: {e}"))?;

            let vol_u16 = ((current_vol_pct as f32 / 100.0) * 65535.0).round() as u16;
            mixer.set_volume(vol_u16);

            let soft_vol = mixer.get_soft_volume();
            let backend_fn = audio_backend::find(None)
                .ok_or_else(|| "Failed to find audio backend".to_string())?;

            let player = Player::new(
                player_config,
                session.clone(),
                soft_vol,
                move || backend_fn(None, AudioFormat::F32),
            );

            let mut event_rx = player.get_player_event_channel();
            let event_task = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    let Some(inner) = inner_weak.upgrade() else {
                        break;
                    };
                    inner.handle_player_event(event);
                }
            });

            Ok::<_, String>((session, player, mixer, event_task))
        })?;

        let active = ActiveSession {
            session: session.clone(),
            player: player.clone(),
            mixer: mixer.clone(),
            _event_task: event_task,
        };

        *lock = Some(active);
        Ok((session, player, mixer))
    }

    /// Plays a Spotify track given a Spotify URI (`spotify://track/<id>` or `spotify:track:<id>`)
    /// or raw base62 track ID.
    pub fn play_track(&self, uri: &str) -> Result<(), String> {
        let id = normalize_spotify_id(uri)?;
        let spotify_id = SpotifyId::from_base62(&id)
            .map_err(|e| format!("Invalid Spotify track ID '{id}': {e}"))?;
        let track_uri = SpotifyUri::Track { id: spotify_id };

        let (session, player, _mixer) = self.ensure_active_session()?;

        // Reset state and set initial track info
        {
            let mut state = self.inner.state.lock().unwrap();
            state.is_playing = true;
            state.progress_ms = 0;
            state.track = Some(ProviderTrack {
                id: format!("spotify://track/{id}"),
                provider: ProviderKind::Spotify,
                title: "Spotify Track".to_string(),
                artist: "Spotify".to_string(),
                album: "".to_string(),
                duration_ms: 0,
                artwork_url: None,
            });
            *self.inner.last_position_update.lock().unwrap() = Some((Instant::now(), 0));
        }

        // Start playback
        player.load(track_uri.clone(), true, 0);

        // Fetch full metadata asynchronously in the background
        let session_meta = session.clone();
        let inner_weak = Arc::downgrade(&self.inner);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Ok(track_meta) = Track::get(&session_meta, &track_uri).await {
                    if let Some(inner) = inner_weak.upgrade() {
                        inner.update_track_metadata(track_meta);
                    }
                }
            });
        }

        Ok(())
    }

    /// Pauses audio playback.
    pub fn pause(&self) -> Result<(), String> {
        if let Ok(guard) = self.inner.active_session.lock() {
            if let Some(active) = guard.as_ref() {
                active.player.pause();
            }
        }
        let mut state = self.inner.state.lock().unwrap();
        if state.is_playing {
            if let Some((instant, base_ms)) = *self.inner.last_position_update.lock().unwrap() {
                let elapsed = instant.elapsed().as_millis() as u64;
                state.progress_ms = (base_ms + elapsed).min(state.duration_ms.max(base_ms + elapsed));
            }
        }
        state.is_playing = false;
        *self.inner.last_position_update.lock().unwrap() = None;
        Ok(())
    }

    /// Resumes audio playback.
    pub fn resume(&self) -> Result<(), String> {
        if let Ok(guard) = self.inner.active_session.lock() {
            if let Some(active) = guard.as_ref() {
                active.player.play();
                let mut state = self.inner.state.lock().unwrap();
                state.is_playing = true;
                *self.inner.last_position_update.lock().unwrap() =
                    Some((Instant::now(), state.progress_ms));
                return Ok(());
            }
        }

        let state = self.inner.state.lock().unwrap();
        if let Some(track) = &state.track {
            let uri = track.id.clone();
            drop(state);
            self.play_track(&uri)
        } else {
            Ok(())
        }
    }

    /// Seeks playback to a specific position in milliseconds.
    pub fn seek(&self, position_ms: u64) -> Result<(), String> {
        if let Ok(guard) = self.inner.active_session.lock() {
            if let Some(active) = guard.as_ref() {
                active.player.seek(position_ms as u32);
            }
        }
        let mut state = self.inner.state.lock().unwrap();
        state.progress_ms = position_ms;
        if state.is_playing {
            *self.inner.last_position_update.lock().unwrap() = Some((Instant::now(), position_ms));
        } else {
            *self.inner.last_position_update.lock().unwrap() = None;
        }
        Ok(())
    }

    /// Sets playback volume (range: 0.0 to 1.0).
    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        let clamped = volume.clamp(0.0, 1.0);
        let vol_u16 = (clamped * 65535.0).round() as u16;
        let vol_percent = (clamped * 100.0).round() as u8;

        if let Ok(guard) = self.inner.active_session.lock() {
            if let Some(active) = guard.as_ref() {
                active.mixer.set_volume(vol_u16);
                active.player.emit_volume_changed_event(vol_u16);
            }
        }

        let mut state = self.inner.state.lock().unwrap();
        state.volume_percent = Some(vol_percent);
        Ok(())
    }

    /// Returns the current playback status, progress, volume, and track info.
    pub fn playback_state(&self) -> SpotifyPlaybackState {
        let mut state = self.inner.state.lock().unwrap().clone();
        if state.is_playing {
            if let Some((instant, base_ms)) = *self.inner.last_position_update.lock().unwrap() {
                let elapsed = instant.elapsed().as_millis() as u64;
                let current = base_ms + elapsed;
                state.progress_ms = if state.duration_ms > 0 {
                    current.min(state.duration_ms)
                } else {
                    current
                };
            }
        }
        state
    }
}

fn run_async_block<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            return tokio::task::block_in_place(|| handle.block_on(future));
        }
    }

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to initialize tokio runtime");
        rt.block_on(future)
    })
    .join()
    .expect("Audio worker thread panicked")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_various_spotify_uris() {
        let uri1 = "spotify://track/1jzIJcHCXneHw7ojC6LXiF";
        let uri2 = "spotify:track:1jzIJcHCXneHw7ojC6LXiF";
        let uri3 = "1jzIJcHCXneHw7ojC6LXiF";
        assert_eq!(normalize_spotify_id(uri1).unwrap(), "1jzIJcHCXneHw7ojC6LXiF");
        assert_eq!(normalize_spotify_id(uri2).unwrap(), "1jzIJcHCXneHw7ojC6LXiF");
        assert_eq!(normalize_spotify_id(uri3).unwrap(), "1jzIJcHCXneHw7ojC6LXiF");
    }

    #[test]
    fn parses_web_urls_and_strips_queries() {
        let url1 = "https://open.spotify.com/track/1jzIJcHCXneHw7ojC6LXiF?si=abc123xyz";
        let url2 = "http://spotify.com/track/1jzIJcHCXneHw7ojC6LXiF";
        assert_eq!(normalize_spotify_id(url1).unwrap(), "1jzIJcHCXneHw7ojC6LXiF");
        assert_eq!(normalize_spotify_id(url2).unwrap(), "1jzIJcHCXneHw7ojC6LXiF");
    }

    #[test]
    fn rejects_invalid_spotify_uris() {
        assert!(normalize_spotify_id("").is_err());
        assert!(normalize_spotify_id("   ").is_err());
        assert!(normalize_spotify_id("not-a-valid-track-id").is_err());
        assert!(normalize_spotify_id("spotify:track:invalid!chars").is_err());
    }

    #[test]
    fn native_player_lifecycle_and_state() {
        let player = NativeSpotifyPlayer::default();
        let state = player.playback_state();
        assert!(!state.is_playing);
        assert_eq!(state.progress_ms, 0);
        assert_eq!(state.device_name.as_deref(), Some("Sonora (Native)"));

        assert!(player.pause().is_ok());
        assert!(player.seek(1500).is_ok());
        let state = player.playback_state();
        assert_eq!(state.progress_ms, 1500);

        assert!(player.set_volume(0.65).is_ok());
        assert_eq!(player.playback_state().volume_percent, Some(65));

        assert!(player.set_volume(1.5).is_ok());
        assert_eq!(player.playback_state().volume_percent, Some(100));

        assert!(player.set_volume(-0.5).is_ok());
        assert_eq!(player.playback_state().volume_percent, Some(0));
    }

    #[test]
    fn native_player_without_credentials_reports_error() {
        let player = NativeSpotifyPlayer::new(None, Some("test_client_id".to_string()));
        let result = player.play_track("1jzIJcHCXneHw7ojC6LXiF");
        // Should report error since not authenticated, rather than crashing or hanging
        assert!(result.is_err());
    }
}
