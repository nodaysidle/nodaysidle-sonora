//! Spotify integration: OAuth 2.0 PKCE against a loopback redirect, token storage in the OS
//! keyring, and the Web API for library browsing.
//!
//! Playback drives the user's Spotify app over Spotify Connect; the Web API also handles library
//! and playlist work.

pub mod connect_player;
pub mod native_player;
pub use connect_player::SpotifyConnectPlayer;
pub use native_player::NativeSpotifyPlayer;

use super::{
    MusicProvider, ProviderKind, ProviderPlaylist, ProviderTrack, SearchResults, TrackAudioSource,
};
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REDIRECT_PORT: u16 = 8899;
/// Spotify's desktop (Keymaster) client id. Login5 audio metadata rejects tokens from a
/// third-party dashboard app, which is what the Web API PKCE flow uses.
const LIBRESPOT_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const LIBRESPOT_REDIRECT_URI: &str = "http://127.0.0.1:8898/login";
const LIBRESPOT_SCOPES: &[&str] = &[
    "streaming",
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-read-currently-playing",
];

pub fn redirect_uri_for(client_id: &str) -> String {
    if client_id == "d420a117a32841c2b3474932e49fb54b" {
        format!("http://127.0.0.1:{REDIRECT_PORT}/login")
    } else {
        format!("http://127.0.0.1:{REDIRECT_PORT}/callback")
    }
}

const SCOPES: &str = "streaming user-read-private user-read-email playlist-read-private playlist-read-collaborative \
                      playlist-modify-private playlist-modify-public user-library-read user-top-read \
                      user-read-playback-state user-modify-playback-state";
const TOKEN_ENDPOINT: &str = "https://accounts.spotify.com/api/token";
const API_BASE: &str = "https://api.spotify.com/v1";
const KEYRING_SERVICE: &str = "com.nodaysidle.sonora";
const KEYRING_USER: &str = "spotify-oauth";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpotifyTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds at which `access_token` stops being valid.
    pub expires_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyPlaybackState {
    pub track: Option<ProviderTrack>,
    pub is_playing: bool,
    pub progress_ms: u64,
    pub duration_ms: u64,
    pub volume_percent: Option<u8>,
    pub device_name: Option<String>,
}

impl SpotifyTokens {
    pub fn is_expired(&self) -> bool {
        now_seconds() + 60 >= self.expires_at
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fallback_tokens_path() -> std::path::PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        std::path::PathBuf::from(config_home)
            .join("sonora")
            .join("spotify_tokens.json")
    } else if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("sonora")
            .join("spotify_tokens.json")
    } else {
        std::path::PathBuf::from("spotify_tokens.json")
    }
}

fn store_tokens_fallback(tokens: &SpotifyTokens) -> Result<(), String> {
    let path = fallback_tokens_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string(tokens).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write fallback Spotify tokens: {e}"))
}

fn load_tokens_fallback() -> Result<Option<SpotifyTokens>, String> {
    let path = fallback_tokens_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read fallback Spotify tokens: {e}"))?;
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|e| format!("unreadable fallback Spotify tokens: {e}"))
}

fn clear_tokens_fallback() -> Result<(), String> {
    let path = fallback_tokens_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

fn keyring_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("OS keyring unavailable, cannot store Spotify credentials: {e}"))
}

fn store_tokens(tokens: &SpotifyTokens) -> Result<(), String> {
    let json = serde_json::to_string(tokens).map_err(|e| e.to_string())?;
    match keyring_entry() {
        Ok(entry) => {
            if let Err(e) = entry.set_password(&json) {
                eprintln!("Keyring write failed ({e}), using file fallback");
                store_tokens_fallback(tokens)?;
            } else {
                let _ = store_tokens_fallback(tokens);
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Keyring unavailable ({e}), using file fallback");
            store_tokens_fallback(tokens)
        }
    }
}

pub fn load_tokens() -> Result<Option<SpotifyTokens>, String> {
    // File fallback first: keyring get_password blocks on a macOS prompt for a newly signed
    // binary, which froze Spotify play until the user dismissed it.
    if let Ok(Some(tokens)) = load_tokens_fallback() {
        return Ok(Some(tokens));
    }
    match keyring_entry() {
        Ok(entry) => match entry.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| format!("unreadable Spotify tokens: {e}")),
            Err(_) => Ok(None),
        },
        Err(_) => Ok(None),
    }
}

pub fn clear_tokens() -> Result<(), String> {
    if let Ok(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
    clear_tokens_fallback()?;
    let path = librespot_tokens_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

pub fn has_credentials() -> bool {
    matches!(load_tokens(), Ok(Some(_)))
}

fn librespot_tokens_path() -> std::path::PathBuf {
    fallback_tokens_path().with_file_name("librespot_tokens.json")
}

fn store_librespot_tokens(tokens: &SpotifyTokens) -> Result<(), String> {
    let path = librespot_tokens_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string(tokens).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write native Spotify tokens: {e}"))
}

fn load_librespot_tokens() -> Result<Option<SpotifyTokens>, String> {
    let path = librespot_tokens_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read native Spotify tokens: {e}"))?;
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|e| format!("unreadable native Spotify tokens: {e}"))
}

fn librespot_oauth_client() -> Result<librespot::oauth::OAuthClient, String> {
    librespot::oauth::OAuthClientBuilder::new(
        LIBRESPOT_CLIENT_ID,
        LIBRESPOT_REDIRECT_URI,
        LIBRESPOT_SCOPES.to_vec(),
    )
    .open_in_browser()
    .with_custom_message("Return to Sonora — native Spotify playback is connected.")
    .build()
    .map_err(|e| e.to_string())
}

fn persist_librespot_oauth(token: librespot::oauth::OAuthToken) -> Result<SpotifyTokens, String> {
    let expires_in = token
        .expires_at
        .saturating_duration_since(std::time::Instant::now())
        .as_secs();
    let tokens = SpotifyTokens {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: now_seconds() + expires_in,
    };
    store_librespot_tokens(&tokens)?;
    Ok(tokens)
}

pub fn authenticate_librespot() -> Result<SpotifyTokens, String> {
    let client = librespot_oauth_client()?;
    let token = client.get_access_token().map_err(|e| e.to_string())?;
    persist_librespot_oauth(token)
}

/// Access token minted by Spotify's Keymaster client — the one Librespot can exchange for audio keys.
pub fn librespot_access_token() -> Result<String, String> {
    if let Some(tokens) = load_librespot_tokens()? {
        if !tokens.is_expired() {
            return Ok(tokens.access_token);
        }
        if !tokens.refresh_token.is_empty() {
            let client = librespot_oauth_client()?;
            match client.refresh_token(&tokens.refresh_token) {
                Ok(token) => return Ok(persist_librespot_oauth(token)?.access_token),
                Err(e) => eprintln!("[sonora] native Spotify token refresh failed: {e}"),
            }
        }
    }
    Err("Reconnect Spotify in Settings to enable native playback.".into())
}

fn random_urlsafe(bytes: usize) -> String {
    use rand::RngCore;
    let mut buffer = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buffer);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

/// S256 challenge: base64url(sha256(verifier)).
fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn urlencode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: u64,
}

fn exchange_token(form: &[(&str, String)]) -> Result<TokenResponse, String> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(TOKEN_ENDPOINT)
        .form(form)
        .send()
        .map_err(|e| format!("token request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!(
            "Spotify rejected the token request ({status}): {body}"
        ));
    }
    response
        .json::<TokenResponse>()
        .map_err(|e| format!("unreadable token response: {e}"))
}

pub fn authenticate(client_id_raw: &str) -> Result<SpotifyTokens, String> {
    authenticate_with_emitter(client_id_raw, |_| {})
}

/// Runs the full PKCE authorization-code flow: opens the system browser, waits for the loopback
/// callback, then exchanges the code for tokens and stores them in the keyring or file fallback.
pub fn authenticate_with_emitter<F: Fn(&str) + Send + Sync + 'static>(
    client_id_raw: &str,
    on_url: F,
) -> Result<SpotifyTokens, String> {
    let client_id = client_id_raw.trim();
    if client_id.is_empty() {
        return Err(
            "Add your Spotify app's Client ID in Settings first, then connect.".to_string(),
        );
    }

    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).map_err(|e| {
        format!("port {REDIRECT_PORT} is busy ({e}); close whatever is using it and retry")
    })?;

    let redirect_uri = redirect_uri_for(client_id);
    let verifier = random_urlsafe(64);
    let state = random_urlsafe(16);
    let authorize_url = format!(
        "https://accounts.spotify.com/authorize?client_id={}&response_type=code&redirect_uri={}\
         &code_challenge_method=S256&code_challenge={}&state={}&scope={}",
        urlencode(client_id),
        urlencode(&redirect_uri),
        code_challenge(&verifier),
        urlencode(&state),
        urlencode(SCOPES),
    );

    on_url(&authorize_url);
    let _ = open_in_browser(&authorize_url);

    // Bounded wait: a user who abandons the browser tab should not leave this thread parked on
    // accept() forever.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("could not arm the OAuth callback listener: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err("Timed out waiting for the Spotify authorization callback.".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("did not receive the OAuth callback: {e}")),
        }
    };
    // The listener was non-blocking; the accepted socket inherits that on some platforms.
    let _ = stream.set_nonblocking(false);

    let request_line = {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("malformed OAuth callback: {e}"))?;
        line
    };

    let query = request_line
        .split_whitespace()
        .nth(1)
        .and_then(|target| target.split_once('?').map(|(_, q)| q.to_string()))
        .unwrap_or_default();
    let params: std::collections::HashMap<_, _> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| {
            (
                k.to_string(),
                urlencoding::decode(v).unwrap_or_default().into_owned(),
            )
        })
        .collect();

    let body = "<html><body style=\"font-family:-apple-system,sans-serif;background:#0A0A0F;color:#C8FF00;\
                display:flex;align-items:center;justify-content:center;height:100vh;margin:0\">\
                <h2>Sonora is connected. You can close this tab.</h2></body></html>";
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.flush();

    if let Some(error) = params.get("error") {
        return Err(format!("Spotify authorization was denied: {error}"));
    }
    if params.get("state") != Some(&state) {
        return Err(
            "OAuth state mismatch; the callback did not originate from this request".to_string(),
        );
    }
    let code = params
        .get("code")
        .ok_or_else(|| "Spotify did not return an authorization code".to_string())?;

    let response = exchange_token(&[
        ("grant_type", "authorization_code".to_string()),
        ("code", code.clone()),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id.to_string()),
        ("code_verifier", verifier),
    ])?;

    let tokens = SpotifyTokens {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .ok_or_else(|| "Spotify did not return a refresh token".to_string())?,
        expires_at: now_seconds() + response.expires_in,
    };
    store_tokens(&tokens)?;
    Ok(tokens)
}

fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(target_os = "windows")]
    let command = "explorer";

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Inside AppImage, LD_LIBRARY_PATH causes host binaries (xdg-open, gio, browsers)
        // to fail due to conflicting bundled libraries. Strip AppImage environment variables.
        let spawn_clean = |bin: &str, args: &[&str]| -> bool {
            let mut cmd = std::process::Command::new(bin);
            cmd.env_remove("LD_LIBRARY_PATH");
            cmd.env_remove("LD_PRELOAD");
            cmd.env_remove("PYTHONPATH");
            cmd.args(args);
            cmd.spawn().is_ok()
        };

        if spawn_clean("xdg-open", &[url]) {
            return Ok(());
        }
        if spawn_clean("gio", &["open", url]) {
            return Ok(());
        }
        for b in &[
            "firefox",
            "google-chrome-stable",
            "google-chrome",
            "chromium",
            "brave",
            "zen-browser",
        ] {
            if spawn_clean(b, &[url]) {
                return Ok(());
            }
        }
        return Err("could not open the browser automatically".to_string());
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    std::process::Command::new(command)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the browser: {e}"))
}

/// Returns a valid access token, refreshing it when the stored one has expired.
pub fn access_token(client_id_raw: &str) -> Result<String, String> {
    let client_id = client_id_raw.trim();
    if client_id.is_empty() {
        return Err("Configure your Spotify Client ID in Settings first.".to_string());
    }
    let tokens = load_tokens()?.ok_or_else(|| "Not connected to Spotify.".to_string())?;
    if !tokens.is_expired() {
        return Ok(tokens.access_token);
    }

    let response = exchange_token(&[
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", tokens.refresh_token.clone()),
        ("client_id", client_id.to_string()),
    ])?;

    let refreshed = SpotifyTokens {
        access_token: response.access_token,
        // Spotify only rotates the refresh token sometimes; keep the old one when it does not.
        refresh_token: response.refresh_token.unwrap_or(tokens.refresh_token),
        expires_at: now_seconds() + response.expires_in,
    };
    store_tokens(&refreshed)?;
    Ok(refreshed.access_token)
}

fn api_get_optional(
    client_id: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<Option<Value>, String> {
    let token = access_token(client_id)?;
    let response = reqwest::blocking::Client::new()
        .get(format!("{API_BASE}{path}"))
        .bearer_auth(token)
        .query(query)
        .send()
        .map_err(|e| format!("Spotify request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("Spotify returned {status}: {body}"));
    }
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(None);
    }
    let body = response.text().map_err(|e| e.to_string())?;
    if body.trim().is_empty() {
        Ok(None)
    } else {
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| e.to_string())
    }
}

fn api_get(client_id: &str, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    api_get_optional(client_id, path, query)?
        .ok_or_else(|| format!("Spotify returned no data for {path}"))
}

fn text_at<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(key)?;
    }
    current.as_str()
}

fn parse_track(item: &serde_json::Value) -> Option<ProviderTrack> {
    let id = item.get("id")?.as_str()?;
    // Local files and podcast episodes have no album object.
    let artists = item
        .get("artists")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let images = item
        .get("album")
        .and_then(|a| a.get("images"))
        .and_then(|i| i.as_array());
    // Spotify orders artwork largest-first; the last entry is the cheapest to load.
    let artwork = images
        .and_then(|list| list.last())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
        .map(str::to_string);

    Some(ProviderTrack {
        id: format!("spotify://track/{id}"),
        provider: ProviderKind::Spotify,
        title: text_at(item, &["name"])
            .unwrap_or("Unknown Title")
            .to_string(),
        artist: artists,
        album: text_at(item, &["album", "name"])
            .unwrap_or("Unknown Album")
            .to_string(),
        duration_ms: item
            .get("duration_ms")
            .and_then(|d| d.as_u64())
            .unwrap_or(0),
        artwork_url: artwork,
    })
}

fn parse_playlist(item: &serde_json::Value) -> Option<ProviderPlaylist> {
    let id = item.get("id")?.as_str()?;
    Some(ProviderPlaylist {
        id: format!("spotify://playlist/{id}"),
        title: text_at(item, &["name"]).unwrap_or("Untitled").to_string(),
        description: item
            .get("description")
            .and_then(|d| d.as_str())
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        artwork_url: item
            .get("images")
            .and_then(|i| i.as_array())
            .and_then(|list| list.first())
            .and_then(|img| img.get("url"))
            .and_then(|u| u.as_str())
            .map(str::to_string),
        track_count: item
            .get("items")
            .or_else(|| item.get("tracks"))
            .and_then(|t| t.get("total"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as u32,
        provider: ProviderKind::Spotify,
    })
}

fn parse_playlist_track(item: &serde_json::Value) -> Option<ProviderTrack> {
    item.get("item")
        .or_else(|| item.get("track"))
        .and_then(parse_track)
}

fn parse_playback_state(value: &Value) -> SpotifyPlaybackState {
    let track = value
        .get("item")
        .filter(|item| !item.is_null())
        .and_then(parse_track);
    SpotifyPlaybackState {
        duration_ms: track
            .as_ref()
            .map(|track| track.duration_ms)
            .unwrap_or_else(|| {
                value
                    .pointer("/item/duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            }),
        progress_ms: value
            .get("progress_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        is_playing: value
            .get("is_playing")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        volume_percent: value
            .pointer("/device/volume_percent")
            .and_then(|v| v.as_i64())
            .and_then(|v| u8::try_from(v.clamp(0, 100)).ok()),
        device_name: value
            .pointer("/device/name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        track,
    }
}

/// The device Sonora should play on: the active one, else a desktop Spotify app, else a web
/// player, else any device that accepts remote control.
fn choose_device(devices: &[Value]) -> Option<String> {
    let usable: Vec<&Value> = devices
        .iter()
        .filter(|device| device.get("is_restricted").and_then(Value::as_bool) != Some(true))
        .collect();
    let text = |device: &Value, key: &str| {
        device
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    usable
        .iter()
        .find(|device| device.get("is_active").and_then(Value::as_bool) == Some(true))
        .or_else(|| {
            usable
                .iter()
                .find(|device| text(device, "type") == "computer")
        })
        .or_else(|| {
            usable.iter().find(|device| {
                let name = text(device, "name");
                name.contains("web player") || name.contains("safari")
            })
        })
        .or_else(|| usable.first())
        .and_then(|device| device.get("id").and_then(Value::as_str))
        .map(str::to_owned)
}

pub struct SpotifyProvider {
    pub client_id: String,
}

impl SpotifyProvider {
    pub fn new(client_id: String) -> Self {
        Self { client_id }
    }

    pub fn get_playlist_tracks(&self, playlist_id: &str) -> Result<Vec<ProviderTrack>, String> {
        let id = playlist_id.trim_start_matches("spotify://playlist/");
        let mut tracks = Vec::new();
        let mut offset = 0usize;
        loop {
            let value = api_get(
                &self.client_id,
                &format!("/playlists/{id}/items"),
                &[("limit", "50".to_string()), ("offset", offset.to_string())],
            )?;
            let items = value
                .get("items")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();
            let page_len = items.len();
            tracks.extend(items.iter().filter_map(parse_playlist_track));
            offset += page_len;
            let total = value
                .get("total")
                .and_then(|v| v.as_u64())
                .unwrap_or(offset as u64);
            if page_len == 0 || offset as u64 >= total || page_len < 50 {
                break;
            }
        }
        Ok(tracks)
    }

    pub fn add_track_to_playlist(&self, playlist_id: &str, track_id: &str) -> Result<(), String> {
        let playlist_id = playlist_id.trim_start_matches("spotify://playlist/");
        let track_id = native_player::normalize_spotify_id(track_id)?;
        let token = access_token(&self.client_id)?;
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?
            .post(format!("{API_BASE}/playlists/{playlist_id}/items"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "uris": [format!("spotify:track:{track_id}")] }))
            .send()
            .map_err(|e| format!("Spotify playlist request failed: {e}"))?;
        match response.status().as_u16() {
            200..=299 => Ok(()),
            403 => Err(
                "Spotify refused the playlist change. Reconnect Spotify to grant playlist access."
                    .to_string(),
            ),
            status => Err(format!("Spotify playlist request returned {status}")),
        }
    }

    pub fn get_saved_tracks(&self, limit: u32) -> Result<Vec<ProviderTrack>, String> {
        let limit = limit.clamp(1, 50);
        let value = api_get(
            &self.client_id,
            "/me/tracks",
            &[("limit", limit.to_string())],
        )?;
        Ok(value
            .get("items")
            .and_then(|i| i.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("track").and_then(parse_track))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn get_top_tracks(&self, limit: u32) -> Result<Vec<ProviderTrack>, String> {
        let limit = limit.clamp(1, 50);
        let value = api_get(
            &self.client_id,
            "/me/top/tracks",
            &[("limit", limit.to_string())],
        )?;
        Ok(value
            .get("items")
            .and_then(|i| i.as_array())
            .map(|items| items.iter().filter_map(parse_track).collect())
            .unwrap_or_default())
    }

    pub fn get_playback_state(&self) -> Result<SpotifyPlaybackState, String> {
        let value = api_get_optional(&self.client_id, "/me/player", &[])?;
        let Some(value) = value else {
            return Ok(SpotifyPlaybackState::default());
        };
        Ok(parse_playback_state(&value))
    }

    fn preferred_device_id(&self) -> Result<Option<String>, String> {
        let value = api_get(&self.client_id, "/me/player/devices", &[])?;
        let devices = value
            .get("devices")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(choose_device(&devices))
    }

    fn player_request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<(), String> {
        let token = access_token(&self.client_id)?;
        let retry_body = body.clone();
        let mut request = reqwest::blocking::Client::new()
            .request(method.clone(), format!("{API_BASE}{path}"))
            .bearer_auth(token)
            .query(query);
        if let Some(body) = body {
            request = request.json(&body);
        } else {
            // Spotify's PUT/POST player endpoints require an explicit zero-length body.
            request = request.header(reqwest::header::CONTENT_LENGTH, "0");
        }
        let response = request
            .send()
            .map_err(|e| format!("Spotify playback request failed: {e}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND
            && !query.iter().any(|(key, _)| *key == "device_id")
        {
            if let Some(device_id) = self.preferred_device_id()? {
                let mut device_query = query.to_vec();
                device_query.push(("device_id", device_id));
                return self.player_request(method, path, &device_query, retry_body);
            }
        }
        match response.status().as_u16() {
            200..=299 => Ok(()),
            403 => Err(
                "Spotify refused playback. Remote control requires a Premium account with an active Spotify device."
                    .to_string(),
            ),
            404 => Err(
                "No active Spotify device found. Open Spotify on a device and start it once, then retry."
                    .to_string(),
            ),
            status => Err(format!("Spotify playback returned {status}")),
        }
    }

    pub fn resume_remote(&self) -> Result<(), String> {
        self.player_request(reqwest::Method::PUT, "/me/player/play", &[], None)
    }

    pub fn pause_remote(&self) -> Result<(), String> {
        self.player_request(reqwest::Method::PUT, "/me/player/pause", &[], None)
    }

    pub fn next_remote(&self) -> Result<(), String> {
        self.player_request(reqwest::Method::POST, "/me/player/next", &[], None)
    }

    pub fn previous_remote(&self) -> Result<(), String> {
        self.player_request(reqwest::Method::POST, "/me/player/previous", &[], None)
    }

    pub fn seek_remote(&self, position_ms: u64) -> Result<(), String> {
        self.player_request(
            reqwest::Method::PUT,
            "/me/player/seek",
            &[("position_ms", position_ms.to_string())],
            None,
        )
    }

    pub fn set_volume_remote(&self, volume: f32) -> Result<(), String> {
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u8;
        self.player_request(
            reqwest::Method::PUT,
            "/me/player/volume",
            &[("volume_percent", percent.to_string())],
            None,
        )
    }

    /// Drives the user's active Spotify device over Connect.
    pub fn play_remote(&self, uri: &str) -> Result<(), String> {
        let spotify_uri = if let Some(id) = uri.strip_prefix("spotify://track/") {
            format!("spotify:track:{id}")
        } else if let Some(id) = uri.strip_prefix("spotify:track:") {
            format!("spotify:track:{id}")
        } else {
            uri.to_string()
        };

        self.player_request(
            reqwest::Method::PUT,
            "/me/player/play",
            &[],
            Some(serde_json::json!({ "uris": [spotify_uri] })),
        )
    }
}

#[async_trait]
impl MusicProvider for SpotifyProvider {
    fn provider_id(&self) -> ProviderKind {
        ProviderKind::Spotify
    }

    async fn search(&self, query: &str, limit: usize) -> Result<SearchResults, String> {
        let client_id = self.client_id.clone();
        let query = query.to_string();
        let limit = limit.clamp(1, 10);
        tokio::task::spawn_blocking(move || {
            let provider = SpotifyProvider::new(client_id);
            let value = api_get(
                &provider.client_id,
                "/search",
                &[
                    ("q", query),
                    ("type", "track,playlist".to_string()),
                    ("limit", limit.to_string()),
                ],
            )?;
            Ok(SearchResults {
                tracks: value
                    .pointer("/tracks/items")
                    .and_then(|i| i.as_array())
                    .map(|items| items.iter().filter_map(parse_track).collect())
                    .unwrap_or_default(),
                playlists: value
                    .pointer("/playlists/items")
                    .and_then(|i| i.as_array())
                    .map(|items| items.iter().filter_map(parse_playlist).collect())
                    .unwrap_or_default(),
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn get_track_stream(&self, _track_id: &str) -> Result<TrackAudioSource, String> {
        Err(super::spotify_cannot_decode_locally().to_string())
    }

    async fn get_user_playlists(&self) -> Result<Vec<ProviderPlaylist>, String> {
        let client_id = self.client_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut playlists = Vec::new();
            let mut offset = 0usize;
            loop {
                let value = api_get(
                    &client_id,
                    "/me/playlists",
                    &[("limit", "50".to_string()), ("offset", offset.to_string())],
                )?;
                let items = value
                    .get("items")
                    .and_then(|i| i.as_array())
                    .cloned()
                    .unwrap_or_default();
                let page_len = items.len();
                playlists.extend(items.iter().filter_map(parse_playlist));
                offset += page_len;
                let total = value
                    .get("total")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(offset as u64);
                if page_len == 0 || offset as u64 >= total || page_len < 50 {
                    break;
                }
            }
            Ok(playlists)
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_the_rfc7636_worked_example() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifiers_are_unique_and_url_safe() {
        let a = random_urlsafe(64);
        let b = random_urlsafe(64);
        assert_ne!(a, b);
        assert!(!a.contains('+') && !a.contains('/') && !a.contains('='));
        assert!(a.len() >= 43, "RFC 7636 requires at least 43 characters");
    }

    #[test]
    fn parses_a_spotify_track_payload() {
        let value = serde_json::json!({
            "id": "4cOdK2wGLETKBW3PvgPWqT",
            "name": "Starboy",
            "duration_ms": 230453,
            "artists": [{"name": "The Weeknd"}, {"name": "Daft Punk"}],
            "album": {
                "name": "Starboy",
                "images": [
                    {"url": "https://i.scdn.co/image/large"},
                    {"url": "https://i.scdn.co/image/small"}
                ]
            }
        });
        let track = parse_track(&value).unwrap();
        assert_eq!(track.id, "spotify://track/4cOdK2wGLETKBW3PvgPWqT");
        assert_eq!(track.artist, "The Weeknd, Daft Punk");
        assert_eq!(track.album, "Starboy");
        assert_eq!(track.duration_ms, 230_453);
        // The smallest artwork in the list is chosen.
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://i.scdn.co/image/small")
        );
    }

    #[test]
    fn a_track_without_an_album_still_parses() {
        let value = serde_json::json!({
            "id": "abc",
            "name": "Local File",
            "duration_ms": 1000,
            "artists": []
        });
        let track = parse_track(&value).unwrap();
        assert_eq!(track.album, "Unknown Album");
        assert_eq!(track.artist, "Unknown Artist");
    }

    #[test]
    fn parses_current_playlist_items_shape() {
        let playlist = parse_playlist(&serde_json::json!({
            "id": "playlist-id",
            "name": "Mix",
            "items": { "total": 3 }
        }))
        .unwrap();
        assert_eq!(playlist.track_count, 3);

        let track = parse_playlist_track(&serde_json::json!({
            "item": {
                "id": "track-id",
                "name": "Song",
                "duration_ms": 1234,
                "artists": [{ "name": "Artist" }],
                "album": { "name": "Album", "images": [] }
            }
        }))
        .unwrap();
        assert_eq!(track.title, "Song");
    }

    #[test]
    fn chooses_a_playable_device_even_when_none_is_active() {
        let devices = vec![
            serde_json::json!({ "id": "tv", "name": "Living Room TV", "type": "TV", "is_active": false }),
            serde_json::json!({ "id": "locked", "name": "Car", "type": "Automobile", "is_restricted": true }),
            serde_json::json!({ "id": "mac", "name": "MacBook", "type": "Computer", "is_active": false }),
        ];
        assert_eq!(choose_device(&devices).as_deref(), Some("mac"));

        let mut with_active = devices.clone();
        with_active[0]["is_active"] = serde_json::json!(true);
        assert_eq!(choose_device(&with_active).as_deref(), Some("tv"));

        assert_eq!(choose_device(&devices[1..2]), None);
        assert_eq!(choose_device(&[]), None);
    }

    #[test]
    fn parses_remote_playback_state_and_clamps_volume() {
        let state = parse_playback_state(&serde_json::json!({
            "is_playing": true,
            "progress_ms": 1200,
            "device": { "name": "MacBook", "volume_percent": 140 },
            "item": {
                "id": "track-id",
                "name": "Song",
                "duration_ms": 5000,
                "artists": [{ "name": "Artist" }],
                "album": { "name": "Album", "images": [] }
            }
        }));
        assert!(state.is_playing);
        assert_eq!(state.progress_ms, 1200);
        assert_eq!(state.duration_ms, 5000);
        assert_eq!(state.volume_percent, Some(100));
        assert_eq!(state.device_name.as_deref(), Some("MacBook"));
        assert_eq!(
            state.track.as_ref().map(|track| track.title.as_str()),
            Some("Song")
        );
    }

    #[test]
    fn expiry_leaves_a_safety_margin() {
        let fresh = SpotifyTokens {
            access_token: "a".into(),
            refresh_token: "b".into(),
            expires_at: now_seconds() + 3600,
        };
        assert!(!fresh.is_expired());

        // Inside the 60 s margin the token is treated as already stale.
        let nearly = SpotifyTokens {
            expires_at: now_seconds() + 30,
            ..fresh.clone()
        };
        assert!(nearly.is_expired());

        let stale = SpotifyTokens {
            expires_at: 0,
            ..fresh
        };
        assert!(stale.is_expired());
    }
}
