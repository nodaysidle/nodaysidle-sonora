//! Spotify playback through Spotify Connect: Sonora drives the user's Spotify app over the Web API
//! and mirrors its state into the same events the rest of the app already listens to.
//!
//! Spotify refuses librespot the audio keys for many (especially newer) Premium accounts
//! (librespot-org/librespot#1649), so an official Spotify client has to render the audio.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use super::native_player::{normalize_spotify_id, EVENT_SPOTIFY_PLAYBACK_STATE};
use super::{SpotifyPlaybackState, SpotifyProvider};
use crate::audio::engine::EVENT_TRACK_ENDED;

const POLL_INTERVAL: Duration = Duration::from_millis(1000);
/// How long a freshly started track may take to show up as playing on the device.
const START_GRACE: Duration = Duration::from_secs(15);
/// Remaining time below which a stopped track counts as finished rather than paused.
const END_MARGIN_MS: u64 = 2_000;

struct ConnectInner {
    client_id: Mutex<Option<String>>,
    app: Mutex<Option<AppHandle>>,
    state: Mutex<SpotifyPlaybackState>,
    /// Bumped on every play and stop so an older poller exits instead of reporting a stale track.
    generation: AtomicU64,
    /// The track Sonora asked the device to play, while Sonora owns Spotify playback.
    active_track: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct SpotifyConnectPlayer {
    inner: Arc<ConnectInner>,
}

/// What one poll of the device means for the track Sonora started.
#[derive(Debug, PartialEq, Eq)]
enum PollVerdict {
    /// Still waiting for the device to start the track.
    Starting,
    Playing,
    Ended,
}

fn judge_poll(expected_track: &str, started: bool, state: &SpotifyPlaybackState) -> PollVerdict {
    let same_track = state.track.as_ref().map(|t| t.id.as_str()) == Some(expected_track);
    if !started {
        return if same_track && state.is_playing {
            PollVerdict::Playing
        } else {
            PollVerdict::Starting
        };
    }
    // Spotify either leaves the finished track loaded at position 0, parks it at its end, or moves
    // on to autoplay; a pause keeps the same track somewhere in the middle.
    let parked = !state.is_playing
        && (state.progress_ms == 0
            || state.duration_ms.saturating_sub(state.progress_ms) <= END_MARGIN_MS);
    if !same_track || parked {
        PollVerdict::Ended
    } else {
        PollVerdict::Playing
    }
}

impl SpotifyConnectPlayer {
    pub fn new(client_id: Option<String>) -> Self {
        Self {
            inner: Arc::new(ConnectInner {
                client_id: Mutex::new(client_id.filter(|id| !id.trim().is_empty())),
                app: Mutex::new(None),
                state: Mutex::new(SpotifyPlaybackState::default()),
                generation: AtomicU64::new(0),
                active_track: Mutex::new(None),
            }),
        }
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.inner.app.lock().unwrap() = Some(app);
    }

    pub fn set_client_id(&self, client_id: String) {
        *self.inner.client_id.lock().unwrap() = (!client_id.trim().is_empty()).then_some(client_id);
    }

    pub fn client_id(&self) -> Option<String> {
        self.inner.client_id.lock().unwrap().clone()
    }

    fn provider(&self) -> Result<SpotifyProvider, String> {
        self.client_id()
            .map(SpotifyProvider::new)
            .ok_or_else(|| "Configure your Spotify Client ID in Settings first.".to_string())
    }

    fn is_active(&self) -> bool {
        self.inner.active_track.lock().unwrap().is_some()
    }

    /// Starts the track on the user's Spotify device and follows it until it ends.
    pub fn play_track(&self, uri: &str) -> Result<(), String> {
        let track_id = format!("spotify://track/{}", normalize_spotify_id(uri)?);
        let provider = self.provider()?;
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Err(error) = provider.play_remote(&track_id) {
            *self.inner.active_track.lock().unwrap() = None;
            return Err(error);
        }
        *self.inner.active_track.lock().unwrap() = Some(track_id.clone());

        let inner = Arc::clone(&self.inner);
        std::thread::spawn(move || follow_track(inner, provider, generation, track_id));
        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        if !self.is_active() {
            return Ok(());
        }
        self.provider()?.pause_remote()
    }

    pub fn resume(&self) -> Result<(), String> {
        self.provider()?.resume_remote()
    }

    /// Hands playback back to Sonora's own engine: stops following the device and pauses it.
    pub fn stop(&self) -> Result<(), String> {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        if self.inner.active_track.lock().unwrap().take().is_none() {
            return Ok(());
        }
        self.inner.state.lock().unwrap().is_playing = false;
        self.provider()?.pause_remote()
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), String> {
        self.provider()?.seek_remote(position_ms)
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), String> {
        self.provider()?.set_volume_remote(volume)
    }

    pub fn playback_state(&self) -> SpotifyPlaybackState {
        self.inner.state.lock().unwrap().clone()
    }
}

fn follow_track(
    inner: Arc<ConnectInner>,
    provider: SpotifyProvider,
    generation: u64,
    track_id: String,
) {
    let started_at = Instant::now();
    let mut started = false;
    loop {
        std::thread::sleep(POLL_INTERVAL);
        if inner.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let Ok(state) = provider.get_playback_state() else {
            continue;
        };
        if inner.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let verdict = judge_poll(&track_id, started, &state);
        *inner.state.lock().unwrap() = state.clone();
        let app = inner.app.lock().unwrap().clone();
        match verdict {
            PollVerdict::Starting => {
                if started_at.elapsed() > START_GRACE {
                    started = true;
                }
            }
            PollVerdict::Playing => {
                started = true;
                if let Some(app) = &app {
                    let _ = app.emit(EVENT_SPOTIFY_PLAYBACK_STATE, &state);
                }
            }
            PollVerdict::Ended => {
                let mut active = inner.active_track.lock().unwrap();
                if active.as_deref() == Some(track_id.as_str()) {
                    *active = None;
                }
                drop(active);
                if let Some(app) = &app {
                    let _ = app.emit(
                        EVENT_TRACK_ENDED,
                        serde_json::json!({ "trackId": track_id, "gapless": false }),
                    );
                }
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderKind, ProviderTrack};

    const TRACK: &str = "spotify://track/4cOdK2wGLETKBW3PvgPWqT";

    fn state(track: Option<&str>, is_playing: bool, progress_ms: u64) -> SpotifyPlaybackState {
        SpotifyPlaybackState {
            track: track.map(|id| ProviderTrack {
                id: id.to_string(),
                provider: ProviderKind::Spotify,
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                duration_ms: 200_000,
                artwork_url: None,
            }),
            is_playing,
            progress_ms,
            duration_ms: 200_000,
            volume_percent: None,
            device_name: None,
        }
    }

    #[test]
    fn waits_for_the_device_to_start_the_requested_track() {
        let previous = "spotify://track/0000000000000000000000";
        assert_eq!(
            judge_poll(TRACK, false, &state(Some(previous), true, 50_000)),
            PollVerdict::Starting
        );
        assert_eq!(
            judge_poll(TRACK, false, &state(Some(TRACK), false, 0)),
            PollVerdict::Starting
        );
        assert_eq!(
            judge_poll(TRACK, false, &state(Some(TRACK), true, 300)),
            PollVerdict::Playing
        );
    }

    #[test]
    fn a_pause_mid_track_is_not_the_end() {
        assert_eq!(
            judge_poll(TRACK, true, &state(Some(TRACK), false, 90_000)),
            PollVerdict::Playing
        );
    }

    #[test]
    fn detects_every_way_spotify_reports_a_finished_track() {
        assert_eq!(
            judge_poll(TRACK, true, &state(Some(TRACK), false, 0)),
            PollVerdict::Ended
        );
        assert_eq!(
            judge_poll(TRACK, true, &state(Some(TRACK), false, 199_500)),
            PollVerdict::Ended
        );
        let autoplay = "spotify://track/1111111111111111111111";
        assert_eq!(
            judge_poll(TRACK, true, &state(Some(autoplay), true, 1_000)),
            PollVerdict::Ended
        );
        assert_eq!(
            judge_poll(TRACK, true, &state(None, false, 0)),
            PollVerdict::Ended
        );
    }

    #[test]
    #[ignore]
    fn live_connect_plays_on_the_users_spotify_device() {
        let home = std::env::var("HOME").expect("HOME");
        let settings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(&home)
                    .join("Library/Application Support/com.nodaysidle.sonora/settings.json"),
            )
            .expect("settings.json"),
        )
        .unwrap();
        let client_id = settings["spotifyClientId"].as_str().map(str::to_string);
        let player = SpotifyConnectPlayer::new(client_id);
        player
            .play_track("spotify:track:4cOdK2wGLETKBW3PvgPWqT")
            .expect("Connect play");
        std::thread::sleep(Duration::from_secs(6));
        let state = player.playback_state();
        eprintln!(
            "device={:?} playing={} progress={}ms track={:?}",
            state.device_name,
            state.is_playing,
            state.progress_ms,
            state.track.as_ref().map(|t| &t.title)
        );
        assert!(state.is_playing, "the device reports the track playing");
        assert_eq!(state.track.map(|t| t.id).as_deref(), Some(TRACK));

        player
            .seek(state.duration_ms.saturating_sub(3_000))
            .unwrap();
        std::thread::sleep(Duration::from_secs(9));
        assert!(!player.is_active(), "the end of the track was detected");
        player.stop().unwrap();
    }

    #[test]
    fn stop_without_active_playback_makes_no_request() {
        let player = SpotifyConnectPlayer::new(None);
        assert!(player.stop().is_ok());
        assert!(player.pause().is_ok());
        assert!(player.play_track("4cOdK2wGLETKBW3PvgPWqT").is_err());
    }
}
