//! Native in-process Spotify audio player engine using `librespot`.
//!
//! Connects directly to Spotify AP servers using PKCE OAuth tokens, decodes 320 kbps Vorbis
//! streams in-process, and renders audio through the rodio/CPAL backend directly on macOS CoreAudio
//! without needing the official Spotify Desktop app or Safari open.

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
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
use tauri::{AppHandle, Emitter};

use super::SpotifyPlaybackState;
use crate::audio::engine::EVENT_TRACK_ENDED;
use crate::providers::{ProviderKind, ProviderTrack};

pub const EVENT_SPOTIFY_PLAYBACK_STATE: &str = "spotify://playback-state";

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
    current_play_request_id: Mutex<Option<u64>>,
    load_wait: Mutex<Option<std::sync::mpsc::SyncSender<Result<(), String>>>>,
    cache_dir: Option<PathBuf>,
    client_id: Mutex<Option<String>>,
    app: Mutex<Option<AppHandle>>,
}

fn canonical_spotify_track_id(uri: &SpotifyUri) -> Option<String> {
    match uri {
        SpotifyUri::Track { id } => id
            .to_base62()
            .ok()
            .map(|id| format!("spotify://track/{id}")),
        _ => None,
    }
}

fn is_current_play_request(current: Option<u64>, event_request: u64) -> bool {
    current == Some(event_request)
}

impl NativePlayerInner {
    fn handle_player_event(&self, event: PlayerEvent) {
        let mut state = self.state.lock().unwrap();
        let mut last_pos = self.last_position_update.lock().unwrap();
        let mut request_id = self.current_play_request_id.lock().unwrap();
        let mut ended_track_id = None;
        let mut emit = true;

        match event {
            PlayerEvent::Loading { play_request_id, .. } => {
                *request_id = Some(play_request_id);
                emit = false;
            }
            PlayerEvent::Playing {
                play_request_id,
                position_ms,
                ..
            } => {
                *request_id = Some(play_request_id);
                state.is_playing = true;
                state.progress_ms = position_ms as u64;
                *last_pos = Some((Instant::now(), position_ms as u64));
                if let Some(tx) = self.load_wait.lock().unwrap().take() {
                    let _ = tx.send(Ok(()));
                }
            }
            PlayerEvent::Paused {
                play_request_id,
                position_ms,
                ..
            } => {
                if !is_current_play_request(*request_id, play_request_id) {
                    emit = false;
                } else {
                    state.is_playing = false;
                    state.progress_ms = position_ms as u64;
                    *last_pos = None;
                }
            }
            PlayerEvent::Stopped { play_request_id, .. } => {
                if !is_current_play_request(*request_id, play_request_id) {
                    emit = false;
                } else {
                    state.is_playing = false;
                    *last_pos = None;
                }
            }
            PlayerEvent::EndOfTrack {
                play_request_id,
                track_id,
                ..
            } => {
                if !is_current_play_request(*request_id, play_request_id) {
                    emit = false;
                } else {
                    state.is_playing = false;
                    state.progress_ms = state.duration_ms;
                    ended_track_id = canonical_spotify_track_id(&track_id)
                        .or_else(|| state.track.as_ref().map(|track| track.id.clone()));
                    *last_pos = None;
                }
            }
            PlayerEvent::PositionCorrection {
                play_request_id,
                position_ms,
                ..
            }
            | PlayerEvent::PositionChanged {
                play_request_id,
                position_ms,
                ..
            }
            | PlayerEvent::Seeked {
                play_request_id,
                position_ms,
                ..
            } => {
                if !is_current_play_request(*request_id, play_request_id) {
                    emit = false;
                } else {
                    state.progress_ms = position_ms as u64;
                    if state.is_playing {
                        *last_pos = Some((Instant::now(), position_ms as u64));
                    }
                }
            }
            PlayerEvent::VolumeChanged { volume } => {
                let percent = (volume as f32 / 65535.0 * 100.0).round() as u8;
                state.volume_percent = Some(percent);
            }
            PlayerEvent::Unavailable {
                play_request_id,
                track_id,
                ..
            } => {
                if !is_current_play_request(*request_id, play_request_id) {
                    emit = false;
                } else {
                    state.is_playing = false;
                    ended_track_id = canonical_spotify_track_id(&track_id)
                        .or_else(|| state.track.as_ref().map(|track| track.id.clone()));
                    *last_pos = None;
                    if let Some(tx) = self.load_wait.lock().unwrap().take() {
                        let _ = tx.send(Err(
                            "Spotify refused the audio key for this track. Native playback cannot decrypt it."
                                .into(),
                        ));
                    }
                }
            }
            _ => {}
        }

        let snapshot = state.clone();
        drop(request_id);
        drop(last_pos);
        drop(state);
        if !emit {
            return;
        }
        if let Some(app) = self.app.lock().unwrap().clone() {
            let _ = app.emit(EVENT_SPOTIFY_PLAYBACK_STATE, snapshot);
            if let Some(track_id) = ended_track_id {
                let _ = app.emit(
                    EVENT_TRACK_ENDED,
                    serde_json::json!({ "trackId": track_id, "gapless": false }),
                );
            }
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
                current_play_request_id: Mutex::new(None),
                load_wait: Mutex::new(None),
                cache_dir,
                client_id: Mutex::new(client_id),
                app: Mutex::new(None),
            }),
        }
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.inner.app.lock().unwrap() = Some(app);
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
        super::librespot_access_token()
    }

    fn ensure_active_session(&self) -> Result<ActiveSessionTuple, String> {
        {
            let lock = self.inner.active_session.lock().map_err(|e| e.to_string())?;
            if let Some(active) = lock.as_ref() {
                if !active.session.is_invalid() && !active.player.is_invalid() {
                    return Ok((
                        active.session.clone(),
                        active.player.clone(),
                        active.mixer.clone(),
                    ));
                }
            }
        }

        // Librespot's play example uses SessionConfig::default() (Keymaster client id).
        // Overriding it with the Web API dashboard id made AP login hang until the UI timed out.
        let token = self.obtain_access_token()?;
        let token_credentials = Some(Credentials::with_access_token(token));
        let cached_credentials = None::<Credentials>;
        let session_config = SessionConfig::default();
        let cache = self.inner.cache_dir.as_ref().and_then(|dir| {
            let vol_dir = dir.join("volume");
            let audio_dir = dir.join("audio");
            Cache::new(None::<PathBuf>, Some(vol_dir), Some(audio_dir), Some(500 * 1024 * 1024)).ok()
        });

        let player_config = PlayerConfig {
            bitrate: Bitrate::Bitrate160,
            normalisation: true,
            normalisation_type: NormalisationType::Album,
            normalisation_method: NormalisationMethod::Dynamic,
            position_update_interval: Some(Duration::from_millis(250)),
            ..Default::default()
        };

        let current_vol_pct = self.inner.state.lock().unwrap().volume_percent.unwrap_or(100);
        let inner_weak = Arc::downgrade(&self.inner);

        let (session, player, mixer, event_task) = run_async_block(async move {
            let mut last_err = None;
            let mut session = None;
            for credentials in [token_credentials, cached_credentials].into_iter().flatten() {
                let candidate = Session::new(session_config.clone(), cache.clone());
                match tokio::time::timeout(
                    Duration::from_secs(15),
                    candidate.connect(credentials, true),
                )
                .await
                {
                    Ok(Ok(())) => {
                        session = Some(candidate);
                        last_err = None;
                        break;
                    }
                    Ok(Err(e)) => last_err = Some(format!("Failed to connect Spotify session: {e}")),
                    Err(_) => last_err = Some("Spotify connection timed out after 15s".to_string()),
                }
            }
            let session = session.ok_or_else(|| {
                last_err.unwrap_or_else(|| "Failed to connect Spotify session".to_string())
            })?;

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

        *self
            .inner
            .active_session
            .lock()
            .map_err(|e| e.to_string())? = Some(active);
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
        *self.inner.current_play_request_id.lock().unwrap() = None;

        {
            let mut state = self.inner.state.lock().unwrap();
            state.is_playing = false;
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
            *self.inner.last_position_update.lock().unwrap() = None;
        }

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        *self.inner.load_wait.lock().unwrap() = Some(tx);
        player.load(track_uri.clone(), true, 0);

        let session_meta = session.clone();
        let inner_weak = Arc::downgrade(&self.inner);
        let meta_uri = track_uri;
        librespot_runtime().spawn(async move {
            if let Ok(track_meta) = Track::get(&session_meta, &meta_uri).await {
                if let Some(inner) = inner_weak.upgrade() {
                    inner.update_track_metadata(track_meta);
                }
            }
        });

        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(err),
            Err(_) => {
                let _ = player.stop();
                *self.inner.load_wait.lock().unwrap() = None;
                Err("Spotify took too long to start this track".to_string())
            }
        }
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

    /// Stops native playback and clears its track so the generic engine can take ownership.
    pub fn stop(&self) -> Result<(), String> {
        if let Ok(guard) = self.inner.active_session.lock() {
            if let Some(active) = guard.as_ref() {
                active.player.stop();
            }
        }
        *self.inner.current_play_request_id.lock().unwrap() = None;
        let mut state = self.inner.state.lock().unwrap();
        state.track = None;
        state.is_playing = false;
        state.progress_ms = 0;
        state.duration_ms = 0;
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

fn librespot_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(4)
            .thread_name("sonora-spotify")
            .build()
            .expect("failed to start Spotify runtime")
    })
}

fn run_async_block<F, T>(future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    librespot_runtime().spawn(async move {
        let _ = tx.send(future.await);
    });
    rx.recv_timeout(Duration::from_secs(40))
        .map_err(|_| "Spotify connection timed out".to_string())?
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

        assert!(player.stop().is_ok());
        let state = player.playback_state();
        assert!(state.track.is_none());
        assert!(!state.is_playing);
        assert_eq!(state.progress_ms, 0);
    }

    #[test]
    fn stale_play_requests_are_ignored() {
        assert!(!is_current_play_request(None, 1));
        assert!(!is_current_play_request(Some(2), 1));
        assert!(is_current_play_request(Some(7), 7));
    }

    #[test]
    fn canonical_track_id_matches_frontend_uris() {
        let id = SpotifyId::from_base62("1jzIJcHCXneHw7ojC6LXiF").unwrap();
        let uri = SpotifyUri::Track { id };
        assert_eq!(
            canonical_spotify_track_id(&uri).as_deref(),
            Some("spotify://track/1jzIJcHCXneHw7ojC6LXiF")
        );
    }

    #[test]
    fn native_player_without_credentials_reports_error() {
        let player = NativeSpotifyPlayer::new(None, None);
        let result = player.play_track("1jzIJcHCXneHw7ojC6LXiF");
        assert!(result.is_err());
    }

    #[test]
    fn run_async_block_polls_the_dedicated_runtime() {
        let value = run_async_block(async { Ok::<_, String>(7) }).unwrap();
        assert_eq!(value, 7);
    }

    #[test]
    #[ignore]
    fn live_native_spotify_starts_playing() {
        struct EprintLogger;
        impl log::Log for EprintLogger {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                eprintln!("[{}] {}", record.target(), record.args());
            }
            fn flush(&self) {}
        }
        static LOGGER: EprintLogger = EprintLogger;
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Debug);
        let home = std::env::var("HOME").expect("HOME");
        let data = PathBuf::from(home).join("Library/Application Support/com.nodaysidle.sonora");
        if crate::providers::spotify::librespot_access_token().is_err() {
            eprintln!("Opening Keymaster OAuth for native playback…");
            crate::providers::spotify::authenticate_librespot().expect("keymaster oauth");
        }
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(data.join("settings.json")).expect("settings.json"),
        )
        .unwrap();
        let client_id = settings
            .get("spotifyClientId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let player = NativeSpotifyPlayer::new(Some(data.join("spotify_cache")), client_id);
        player
            .play_track("7v3rmoy5jcn4h5UqwQyCM3")
            .expect("native Spotify play");
        assert!(
            player.playback_state().is_playing,
            "Librespot must report Playing before play_track returns"
        );
        std::thread::sleep(Duration::from_secs(2));
        player.stop().unwrap();
    }
}
