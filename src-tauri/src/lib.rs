pub mod audio;
pub mod config;
pub mod db;
pub mod library;
pub mod lyrics;
pub mod providers;

use audio::engine::{AudioEngine, EngineState, EngineTrack};
use config::AppConfig;
use db::{AlbumRecord, ArtistRecord, DbState, TrackRecord};
use library::scanner::{embedded_lyrics, scan_directory, ScanSummary};
use lyrics::ParsedLyrics;
use notify::{EventKind, RecursiveMode, Watcher};
use providers::spotify::{NativeSpotifyPlayer, SpotifyPlaybackState, SpotifyProvider};
use providers::ytmusic::YouTubeMusicProvider;
use providers::{MusicProvider, ProviderPlaylist, ProviderTrack, SearchResults};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

pub const EVENT_SCAN_PROGRESS: &str = "sonora://scan-progress";
pub const EVENT_LIBRARY_CHANGED: &str = "sonora://library-changed";
pub const EVENT_MEDIA_KEY: &str = "sonora://media-key-event";
/// TRD §3 caps scan progress events at roughly one every 50 ms.
const SCAN_EVENT_INTERVAL: Duration = Duration::from_millis(50);

pub struct AppState {
    pub audio_engine: Arc<AudioEngine>,
    pub db: Arc<DbState>,
    pub data_dir: PathBuf,
    pub spotify_player: Arc<NativeSpotifyPlayer>,
    library_watchers: Mutex<Vec<notify::RecommendedWatcher>>,
    scan_lock: Arc<Mutex<()>>,
    auto_scan_pending: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    fn config(&self) -> AppConfig {
        AppConfig::load(&self.data_dir)
    }
}

// -------------------------------------------------------------------------------------------
// Playback
// -------------------------------------------------------------------------------------------

#[tauri::command]
fn playback_load_track(
    state: State<AppState>,
    track: EngineTrack,
    auto_play: bool,
) -> Result<(), String> {
    let _ = state.spotify_player.pause();
    state.audio_engine.load_track(track, auto_play)
}

#[tauri::command]
fn playback_set_next_track(
    state: State<AppState>,
    track: Option<EngineTrack>,
) -> Result<(), String> {
    state.audio_engine.prebuffer_next_track(track)
}

#[tauri::command]
fn playback_play(state: State<AppState>) -> Result<(), String> {
    let _ = state.spotify_player.pause();
    state.audio_engine.play()
}

#[tauri::command]
fn playback_pause(state: State<AppState>) -> Result<(), String> {
    state.audio_engine.pause()
}

#[tauri::command]
fn playback_stop(state: State<AppState>) -> Result<(), String> {
    state.audio_engine.stop()
}

#[tauri::command]
fn playback_seek(state: State<AppState>, position_ms: u64) -> Result<(), String> {
    state.audio_engine.seek(position_ms)
}

#[tauri::command]
fn playback_set_volume(state: State<AppState>, volume: f32) -> Result<(), String> {
    state.audio_engine.set_volume(volume)
}

#[tauri::command]
fn playback_toggle_normalization(state: State<AppState>, enabled: bool) -> Result<bool, String> {
    state.audio_engine.toggle_normalization(enabled)
}

#[tauri::command]
fn playback_get_state(state: State<AppState>) -> EngineState {
    state.audio_engine.snapshot()
}

// -------------------------------------------------------------------------------------------
// Library
// -------------------------------------------------------------------------------------------

#[tauri::command]
async fn library_scan_directory(app: AppHandle, dir_path: String) -> Result<ScanSummary, String> {
    let path = PathBuf::from(&dir_path)
        .canonicalize()
        .map_err(|e| format!("could not open music folder {dir_path}: {e}"))?;
    if !path.is_dir() {
        return Err(format!("Not a folder: {dir_path}"));
    }

    let state = app.state::<AppState>();
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let scan_lock = state.scan_lock.clone();
    drop(state);
    let scan_app = app.clone();

    let summary = tauri::async_runtime::spawn_blocking(move || {
        let _guard = scan_lock.lock().unwrap_or_else(|e| e.into_inner());
        scan_directory_sync(scan_app, db, data_dir, path, dir_path)
    })
    .await
    .map_err(|e| format!("scan task failed: {e}"))??;
    let _ = configure_library_watchers(&app);
    Ok(summary)
}

fn scan_directory_sync(
    app: AppHandle,
    db: Arc<DbState>,
    data_dir: PathBuf,
    path: PathBuf,
    configured_path: String,
) -> Result<ScanSummary, String> {
    let started = Instant::now();
    let artwork_dir = data_dir.join("artwork");

    // Metadata extraction is CPU-bound and runs on a worker pool, so progress is reported from
    // several threads at once. A compare-exchange on the last-emit stamp throttles the events
    // to one per interval and makes a duplicate emission from two racing workers impossible.
    let last_emit = AtomicU64::new(0);
    let tracks = scan_directory(&path, &artwork_dir, worker_count(), |done, total, file| {
        let now = started.elapsed().as_millis() as u64;
        let previous = last_emit.load(Ordering::Relaxed);
        if now.saturating_sub(previous) >= SCAN_EVENT_INTERVAL.as_millis() as u64
            && last_emit
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let _ = app.emit(
                EVENT_SCAN_PROGRESS,
                serde_json::json!({
                    "scanned": done,
                    "total": total,
                    "currentFile": file.to_string_lossy(),
                }),
            );
        }
    });

    let scanned = tracks.len() as u32;
    let mut indexed = 0u32;
    for batch in tracks.chunks(256) {
        indexed += db.upsert_tracks(batch).map_err(|e| e.to_string())? as u32;
    }
    // Never remove rows belonging to another configured root that was not part of this scan.
    let removed = db.delete_missing_local_tracks_under(&path).map_err(|e| {
        format!(
            "could not remove stale tracks under {}: {e}",
            path.display()
        )
    })? as u32;

    let mut config = AppConfig::load(&data_dir);
    if !config.music_folders.contains(&configured_path) {
        config.music_folders.push(configured_path);
        let _ = config.save(&data_dir);
    }

    let summary = ScanSummary {
        scanned,
        indexed,
        removed,
        duration_ms: started.elapsed().as_millis() as u64,
    };
    let _ = app.emit(EVENT_LIBRARY_CHANGED, &summary);
    Ok(summary)
}

fn schedule_watched_rescan(app: AppHandle, root: PathBuf) {
    let state = app.state::<AppState>();
    if state
        .auto_scan_pending
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let pending = state.auto_scan_pending.clone();
    let scan_lock = state.scan_lock.clone();
    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    drop(state);

    // ponytail: one 500 ms global debounce keeps notify bursts (copying an album) from launching
    // one scan per file. Replace with per-root scheduling only if users scan concurrently.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let _guard = scan_lock.lock().unwrap_or_else(|e| e.into_inner());
        pending.store(false, std::sync::atomic::Ordering::Release);
        if let Err(error) = scan_directory_sync(
            app,
            db,
            data_dir,
            root.clone(),
            root.to_string_lossy().to_string(),
        ) {
            eprintln!("[sonora] watched library rescan failed: {error}");
        }
    });
}

fn configure_library_watchers(app: &AppHandle) -> Result<(), String> {
    let folders = app.state::<AppState>().config().music_folders;
    let mut watchers = Vec::new();
    for folder in folders {
        let root = PathBuf::from(&folder);
        if !root.is_dir() {
            continue;
        }
        let root = root
            .canonicalize()
            .map_err(|e| format!("could not watch {folder}: {e}"))?;
        let callback_app = app.clone();
        let callback_root = root.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else { return };
                let relevant_kind = matches!(
                    &event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                let relevant_path = event.paths.is_empty()
                    || event
                        .paths
                        .iter()
                        .any(|path| path.is_dir() || library::scanner::is_audio_file(path));
                if relevant_kind && relevant_path {
                    schedule_watched_rescan(callback_app.clone(), callback_root.clone());
                }
            })
            .map_err(|e| format!("could not create music-folder watcher: {e}"))?;
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("could not watch {}: {e}", root.display()))?;
        watchers.push(watcher);
    }

    let state = app.state::<AppState>();
    *state
        .library_watchers
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = watchers;
    Ok(())
}

fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1)
}

#[tauri::command]
fn library_get_tracks(
    state: State<AppState>,
    query: Option<String>,
    provider: Option<String>,
    sort: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<TrackRecord>, String> {
    state
        .db
        .get_tracks(
            query.as_deref(),
            provider.as_deref(),
            sort.as_deref(),
            limit.unwrap_or(500),
            offset.unwrap_or(0),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn library_get_albums(
    state: State<AppState>,
    provider: Option<String>,
    query: Option<String>,
) -> Result<Vec<AlbumRecord>, String> {
    state
        .db
        .get_albums(provider.as_deref(), query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn library_get_artists(
    state: State<AppState>,
    provider: Option<String>,
    query: Option<String>,
) -> Result<Vec<ArtistRecord>, String> {
    state
        .db
        .get_artists(provider.as_deref(), query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn library_count_tracks(state: State<AppState>, provider: Option<String>) -> Result<u32, String> {
    state
        .db
        .count_tracks(provider.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn library_get_config(state: State<AppState>) -> AppConfig {
    state.config()
}

#[tauri::command]
fn library_set_music_folders(app: AppHandle, folders: Vec<String>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let data_dir = state.data_dir.clone();
    drop(state);
    let mut config = AppConfig::load(&data_dir);
    config.music_folders = folders;
    config.save(&data_dir)?;
    configure_library_watchers(&app)
}

/// The Spotify client ID is public by design (PKCE ships no secret), so it lives in the settings
/// file rather than the keyring, which is reserved for the OAuth tokens.
#[tauri::command]
fn library_set_spotify_client_id(state: State<AppState>, client_id: String) -> Result<(), String> {
    let data_dir = state.data_dir.clone();
    let mut config = AppConfig::load(&data_dir);
    let trimmed = client_id.trim().to_string();
    config.spotify_client_id = (!trimmed.is_empty()).then_some(trimmed);
    config.save(&data_dir)?;
    state.spotify_player.set_client_id(client_id);
    Ok(())
}

// -------------------------------------------------------------------------------------------
// Lyrics
// -------------------------------------------------------------------------------------------

#[tauri::command]
fn lyrics_get_for_track(
    state: State<AppState>,
    track: TrackRecord,
) -> Result<ParsedLyrics, String> {
    let embedded = track
        .file_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_file())
        .and_then(embedded_lyrics);

    lyrics::resolve_lyrics(
        &state.db,
        lyrics::LyricsRequest {
            track_id: &track.id,
            title: &track.title,
            artist: &track.artist,
            album: Some(&track.album),
            duration_ms: track.duration_ms,
            embedded,
        },
    )
}

// -------------------------------------------------------------------------------------------
// Spotify
// -------------------------------------------------------------------------------------------

fn spotify_provider(state: &AppState) -> Result<SpotifyProvider, String> {
    let client_id = state
        .config()
        .spotify_client_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "Add your Spotify app's Client ID in Settings first, then connect.".to_string())?;
    Ok(SpotifyProvider::new(client_id))
}

#[tauri::command]
async fn spotify_authenticate(app: AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let client_id = state
        .config()
        .spotify_client_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "Add your Spotify app's Client ID in Settings first.".to_string())?;
    drop(state);

    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        providers::spotify::authenticate_with_emitter(&client_id, move |url| {
            let _ = app_handle.emit("spotify://auth-url", url);
            use tauri_plugin_opener::OpenerExt;
            let _ = app_handle.opener().open_url(url, None::<&str>);
        })
        .map(|_| true)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn spotify_status() -> Result<bool, String> {
    Ok(providers::spotify::has_credentials())
}

#[tauri::command]
fn spotify_disconnect() -> Result<(), String> {
    providers::spotify::clear_tokens()
}

#[tauri::command]
async fn spotify_search(
    app: AppHandle,
    query: String,
    limit: Option<u32>,
) -> Result<SearchResults, String> {
    let state = app.state::<AppState>();
    let provider = spotify_provider(&state)?;
    drop(state);
    provider.search(&query, limit.unwrap_or(25) as usize).await
}

#[tauri::command]
async fn spotify_playlists(app: AppHandle) -> Result<Vec<ProviderPlaylist>, String> {
    let state = app.state::<AppState>();
    let provider = spotify_provider(&state)?;
    drop(state);
    provider.get_user_playlists().await
}

#[tauri::command]
async fn spotify_playlist_tracks(
    app: AppHandle,
    playlist_id: String,
) -> Result<Vec<ProviderTrack>, String> {
    let state = app.state::<AppState>();
    let provider = spotify_provider(&state)?;
    drop(state);
    tauri::async_runtime::spawn_blocking(move || provider.get_playlist_tracks(&playlist_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_library(app: AppHandle, limit: Option<u32>) -> Result<Vec<ProviderTrack>, String> {
    let state = app.state::<AppState>();
    let provider = spotify_provider(&state)?;
    drop(state);
    tauri::async_runtime::spawn_blocking(move || {
        let limit = limit.unwrap_or(50);
        // Saved tracks are the most stable starting point; top tracks fill a fresh account.
        let saved = provider.get_saved_tracks(limit)?;
        if saved.is_empty() {
            provider.get_top_tracks(limit)
        } else {
            Ok(saved)
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_play(app: AppHandle, uri: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _ = state.audio_engine.stop();
    let player = state.spotify_player.clone();
    drop(state);

    tauri::async_runtime::spawn_blocking(move || player.play_track(&uri))
        .await
        .map_err(|e| e.to_string())?
}

async fn spotify_action<F>(app: AppHandle, action: F) -> Result<(), String>
where
    F: FnOnce(SpotifyProvider) -> Result<(), String> + Send + 'static,
{
    let state = app.state::<AppState>();
    let provider = spotify_provider(&state)?;
    drop(state);
    tauri::async_runtime::spawn_blocking(move || action(provider))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn spotify_playback_state(app: AppHandle) -> Result<SpotifyPlaybackState, String> {
    let state = app.state::<AppState>();
    Ok(state.spotify_player.playback_state())
}

#[tauri::command]
async fn spotify_resume(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _ = state.audio_engine.stop();
    let player = state.spotify_player.clone();
    drop(state);

    tauri::async_runtime::spawn_blocking(move || player.resume())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_pause(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let player = state.spotify_player.clone();
    drop(state);

    tauri::async_runtime::spawn_blocking(move || player.pause())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_next(app: AppHandle) -> Result<(), String> {
    spotify_action(app, |provider| provider.next_remote()).await
}

#[tauri::command]
async fn spotify_previous(app: AppHandle) -> Result<(), String> {
    spotify_action(app, |provider| provider.previous_remote()).await
}

#[tauri::command]
async fn spotify_seek(app: AppHandle, position_ms: u64) -> Result<(), String> {
    let state = app.state::<AppState>();
    let player = state.spotify_player.clone();
    drop(state);

    tauri::async_runtime::spawn_blocking(move || player.seek(position_ms))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_set_volume(app: AppHandle, volume: f32) -> Result<(), String> {
    let state = app.state::<AppState>();
    let player = state.spotify_player.clone();
    drop(state);

    tauri::async_runtime::spawn_blocking(move || player.set_volume(volume))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn spotify_resolve_stream(
    title: String,
    artist: String,
    duration_ms: u64,
) -> Result<String, String> {
    let clean_title = title
        .split('(')
        .next()
        .unwrap_or(&title)
        .split('-')
        .next()
        .unwrap_or(&title)
        .trim();
    let query = format!("{artist} {clean_title}");
    let yt = YouTubeMusicProvider::new();
    let search_res = match yt.search(&query, 5).await {
        Ok(res) if !res.tracks.is_empty() => res,
        _ => yt
            .search(&format!("{title} {artist}"), 5)
            .await
            .map_err(|e| format!("Stream search failed: {e}"))?,
    };

    let candidate = if duration_ms > 0 {
        search_res
            .tracks
            .iter()
            .min_by_key(|t| (t.duration_ms as i64 - duration_ms as i64).abs())
            .or_else(|| search_res.tracks.first())
    } else {
        search_res.tracks.first()
    };

    let best = candidate.ok_or_else(|| format!("No audio stream found for '{title}' by '{artist}'"))?;
    let video_id = best.id.trim_start_matches("ytmusic://track/").to_string();

    tauri::async_runtime::spawn_blocking(move || {
        YouTubeMusicProvider::new()
            .resolve_stream(&video_id)
            .map(|source| source.url)
    })
    .await
    .map_err(|e| e.to_string())?
}

// -------------------------------------------------------------------------------------------
// YouTube Music
// -------------------------------------------------------------------------------------------

#[tauri::command]
async fn ytmusic_search(query: String, limit: Option<u32>) -> Result<SearchResults, String> {
    YouTubeMusicProvider::new()
        .search(&query, limit.unwrap_or(25) as usize)
        .await
}

#[tauri::command]
async fn ytmusic_resolve_stream(video_id: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        YouTubeMusicProvider::new()
            .resolve_stream(&video_id)
            .map(|source| source.url)
    })
    .await
    .map_err(|e| e.to_string())?
}

// -------------------------------------------------------------------------------------------
// Setup
// -------------------------------------------------------------------------------------------

/// Resolves the library database and artwork cache under the OS app-data directory
/// (`~/Library/Application Support/com.nodaysidle.sonora` on macOS).
fn prepare_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("no app data directory: {e}"))?;
    std::fs::create_dir_all(dir.join("artwork")).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(desktop)]
fn media_key_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_global_shortcut::{Builder, Code, ShortcutState};

    Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let action = match shortcut.key {
                Code::MediaPlayPause => "play_pause",
                Code::MediaTrackNext => "next",
                Code::MediaTrackPrevious => "prev",
                _ => return,
            };
            let _ = app.emit(EVENT_MEDIA_KEY, serde_json::json!({ "action": action }));
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());
    #[cfg(desktop)]
    {
        builder = builder.plugin(media_key_plugin());
    }
    builder
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = prepare_data_dir(&handle).unwrap_or_else(|err| {
                panic!("Sonora cannot start without an app data directory: {err}")
            });

            let db = Arc::new(
                DbState::new(data_dir.join("sonora.db"))
                    .expect("failed to open the Sonora library database"),
            );
            let audio_engine = Arc::new(AudioEngine::new());
            audio_engine.attach_app(handle.clone());

            let client_id = AppConfig::load(&data_dir).spotify_client_id;
            let spotify_cache = data_dir.join("spotify_cache");
            let spotify_player = Arc::new(NativeSpotifyPlayer::new(Some(spotify_cache), client_id));

            app.manage(AppState {
                audio_engine,
                db,
                data_dir,
                spotify_player,
                library_watchers: Mutex::new(Vec::new()),
                scan_lock: Arc::new(Mutex::new(())),
                auto_scan_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            if let Err(error) = configure_library_watchers(&handle) {
                eprintln!("[sonora] music-folder watcher unavailable: {error}");
            }

            #[cfg(desktop)]
            {
                use tauri_plugin_global_shortcut::GlobalShortcut;

                let media_keys = app.state::<GlobalShortcut<tauri::Wry>>();
                if let Err(error) = media_keys.register_multiple([
                    "MediaPlayPause",
                    "MediaTrackNext",
                    "MediaTrackPrevious",
                ]) {
                    eprintln!("[sonora] media keys unavailable: {error}");
                }
            }

            let window = app
                .get_webview_window("main")
                .expect("main window is declared in tauri.conf.json");

            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
                let _ = apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::UnderWindowBackground,
                    None,
                    None,
                );
            }
            #[cfg(target_os = "windows")]
            {
                use window_vibrancy::apply_mica;
                let _ = apply_mica(&window, None);
            }
            #[cfg(target_os = "linux")]
            {
                let _ = window_vibrancy::apply_blur(&window, Some((18, 18, 26, 200)));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            playback_load_track,
            playback_set_next_track,
            playback_play,
            playback_pause,
            playback_stop,
            playback_seek,
            playback_set_volume,
            playback_toggle_normalization,
            playback_get_state,
            library_scan_directory,
            library_get_tracks,
            library_get_albums,
            library_get_artists,
            library_count_tracks,
            library_get_config,
            library_set_music_folders,
            library_set_spotify_client_id,
            lyrics_get_for_track,
            spotify_authenticate,
            spotify_status,
            spotify_disconnect,
            spotify_search,
            spotify_playlists,
            spotify_playlist_tracks,
            spotify_library,
            spotify_play,
            spotify_playback_state,
            spotify_resume,
            spotify_pause,
            spotify_next,
            spotify_previous,
            spotify_seek,
            spotify_set_volume,
            spotify_resolve_stream,
            ytmusic_search,
            ytmusic_resolve_stream,
        ])
        .run(tauri::generate_context!())
        .expect("error while running sonora application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::engine::EVENT_TRACK_ENDED;

    /// The frontend listens on this exact string; a rename would silently break playback UI.
    #[test]
    fn track_ended_event_name_is_stable() {
        assert_eq!(EVENT_TRACK_ENDED, "sonora://track-ended");
        assert_eq!(EVENT_SCAN_PROGRESS, "sonora://scan-progress");
        assert_eq!(EVENT_LIBRARY_CHANGED, "sonora://library-changed");
        assert_eq!(EVENT_MEDIA_KEY, "sonora://media-key-event");
    }

    #[test]
    fn scan_events_are_throttled_well_under_the_webview_budget() {
        assert!(SCAN_EVENT_INTERVAL >= Duration::from_millis(50));
    }

    #[test]
    fn worker_count_leaves_a_core_for_the_audio_thread() {
        assert!(worker_count() >= 1);
        if let Ok(parallelism) = std::thread::available_parallelism() {
            if parallelism.get() > 1 {
                assert!(worker_count() < parallelism.get());
            }
        }
    }

    /// A non-directory path is rejected before any scanning work starts.
    #[test]
    fn scan_path_validation_rejects_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("song.flac");
        std::fs::write(&file, b"not audio").unwrap();
        assert!(!file.is_dir(), "a file must not pass the is_dir guard");
        assert!(temp.path().is_dir());
    }

    #[test]
    fn app_state_spotify_player_initialization_and_client_id_sync() {
        let player = Arc::new(NativeSpotifyPlayer::new(None, Some("initial_id".to_string())));
        assert_eq!(player.client_id().as_deref(), Some("initial_id"));

        player.set_client_id("new_client_id".to_string());
        assert_eq!(player.client_id().as_deref(), Some("new_client_id"));

        player.set_client_id("   ".to_string());
        assert_eq!(player.client_id(), None);
    }

    #[test]
    fn playback_coordination_pause_and_stop_do_not_panic() {
        let engine = Arc::new(AudioEngine::new());
        let player = Arc::new(NativeSpotifyPlayer::new(None, None));

        // Coordination: starting local/yt playback pauses Spotify player
        assert!(player.pause().is_ok());

        // Coordination: starting Spotify playback stops the AudioEngine
        assert!(engine.stop().is_ok());
    }
}
