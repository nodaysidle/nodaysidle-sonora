# Design Spec: Native In-App Spotify Streaming via `librespot` & UI Centering

## Executive Summary
Sonora is a high-performance native desktop music player. Currently, Spotify playback relies on the Spotify Connect Web API, which delegates audio streaming to an external Spotify client (e.g., Spotify Desktop or Spotify Web Player running in Safari). When no external device is actively playing, playback fails and requires opening an external app.

This design introduces native in-app Spotify audio streaming using **`librespot`** (Rust). Sonora will act as its own native Spotify player, directly decoding high-quality (320 kbps) Vorbis audio streams through macOS CoreAudio/CPAL without needing the official Spotify app or Safari open. Additionally, the window header UI is refined to place the "Sonora" title centered in the top bar and remove the green "NATIVE DESKTOP" badge.

---

## 1. Requirements

### Functional Requirements
1. **In-App Spotify Playback**: When a user clicks play on any Spotify track in Sonora, the audio must stream and decode directly within Sonora.
2. **No External App Dependencies**: Playback must succeed without Spotify.app installed or Safari open.
3. **Transport Controls**: Play, Pause, Seek, and Volume adjustments in Sonora's bottom bar must instantly control the internal `librespot` player.
4. **State & Progress Synchronization**: Elapsed time, total duration, and playback status must be emitted to the frontend (`sonora://playback-progress` and `sonora://playback-status`) so that the scrubber and synchronized scrolling lyrics (`SyncedLyricsView`) stay in lockstep.
5. **Session Persistence**: Spotify credentials/session keys must be securely cached in the application data directory (`~/Library/Application Support/com.nodaysidle.sonora/spotify-cache/`) alongside existing Keychain tokens.
6. **Centered Minimalist Header**: The top titlebar in `Shell.tsx` must center the "Sonora" label across the window and remove the green badge.

### Non-Functional Requirements
1. **Audio Quality**: Stream at 320 kbps (Vorbis).
2. **Low Latency**: Transport controls (play/pause/seek) must respond in < 150ms.
3. **Memory & CPU Efficiency**: Keep memory footprint under 120MB and idle CPU under 2% during streaming.
4. **Zero Compiler Warnings/Errors**: Both `cargo check` and `npm run build` must pass cleanly.

---

## 2. Architecture & Subsystems

### Subsystem Overview

```
React 19 Frontend (Tauri Webview)
   │
   ├── Top Header: Centered "Sonora" (drag-region, no green badge)
   ├── Player Bar: Transport controls (play, pause, seek, volume)
   └── Synced Lyrics: 100ms interval lock with playback progress
   │
   ▼ Tauri IPC Commands / Events
   │
Rust Backend (`src-tauri`)
   │
   ├── `AppState`
   │     ├── `AudioEngine`: Plays local files & YouTube Music via Symphonia/CPAL
   │     └── `SpotifyManager`: Manages `librespot` Session & Player
   │
   └── `librespot` In-Process Player (`src-tauri/src/providers/spotify/`)
         ├── `Session`: Connected via cached credentials or OAuth access token
         ├── `Spirc`: Internal Spotify Connect receiver named "Sonora"
         └── Output Sink: Direct CoreAudio / CPAL audio device output
```

### Module Responsibilities

1. **`src-tauri/Cargo.toml`**:
   Add `librespot` with `rodio-backend`, `native-tls`, and `with-libmdns` features.
2. **`src-tauri/src/providers/spotify/`**:
   - `mod.rs`: Coordinates browsing and playback.
   - `player.rs`: Encapsulates `librespot_connect::spirc::Spirc` and `librespot_playback::player::Player`. Exposes thread-safe commands:
     - `load_and_play(uri: &str)`
     - `pause()`
     - `resume()`
     - `seek(position_ms: u64)`
     - `set_volume(volume: f32)`
     - `get_state() -> SpotifyPlaybackState`
   - `session.rs`: Handles connection to Spotify AP servers, session configuration (320kbps, normalisation), and credential caching.
   - `api.rs`: Existing Spotify Web API client for searching and browsing user playlists.
3. **`src-tauri/src/lib.rs`**:
   - Wire `spotify_play`, `spotify_pause`, `spotify_resume`, `spotify_seek`, and `spotify_set_volume` to the internal `SpotifyManager`.
   - Forward player events to Tauri emitters (`sonora://playback-progress`, `sonora://playback-status`).
4. **`src/components/layout/Shell.tsx`**:
   - Update `<header>` layout to center the "Sonora" title using CSS flexbox / absolute positioning with `pointer-events-none` on the title and drag regions enabled.
   - Delete the green `<span className="... text-lime-400 font-mono ...">NATIVE DESKTOP</span>` badge.
5. **`src/components/player/PlayerBar.tsx`**:
   - Show `Spotify (Native)` when current track provider is `spotify`.

---

## 3. Data Flow

### Playback Flow
1. User clicks track `spotify://track/1jzIJcHCXneHw7ojC6LXiF` in Sonora UI.
2. `playerStore.ts` calls `tauriBridge.spotifyPlay(track.id)`.
3. Rust command `spotify_play` receives base62 ID, converts to Spotify URI `spotify:track:1jzIJcHCXneHw7ojC6LXiF`.
4. `AudioEngine` (local playback) is paused/stopped to release output conflicts.
5. `SpotifyManager` tells `Spirc` to load the track.
6. `librespot` fetches audio stream from Spotify AP servers, decrypts with AES-128 key, decodes Ogg Vorbis, and sends PCM samples to CPAL.
7. Background event loop polls/listens to player position and emits `sonora://playback-progress` to React.
8. Synced lyrics view updates active line based on current position.

---

## 4. UI Changes (`Shell.tsx`)

### Before:
```tsx
<header data-tauri-drag-region className="h-11 w-full flex items-center px-4 pl-[84px] select-none shrink-0 border-b border-white/5 bg-black/25 backdrop-blur-xl">
  <div className="flex items-center gap-3">
    <span className="text-xs font-semibold tracking-[0.18em] text-zinc-300 uppercase">
      Sonora
    </span>
    <span className="text-[10px] px-1.5 py-0.5 rounded bg-lime-400/10 text-lime-400 font-mono tracking-wide">
      NATIVE DESKTOP
    </span>
  </div>
</header>
```

### After:
```tsx
<header data-tauri-drag-region className="relative h-11 w-full flex items-center justify-center select-none shrink-0 border-b border-white/5 bg-black/25 backdrop-blur-xl">
  <div className="absolute inset-x-0 flex items-center justify-center pointer-events-none">
    <span className="text-xs font-semibold tracking-[0.22em] text-zinc-300 uppercase">
      Sonora
    </span>
  </div>
</header>
```
* The title is centered across the entire window header.
* The macOS traffic lights (drawn at the top left by AppKit) have plenty of clear space.
* The green "NATIVE DESKTOP" tag is completely removed.

---

## 5. Verification & Testing

1. **Compilation Check**:
   - `cargo check --manifest-path src-tauri/Cargo.toml` passes with 0 errors.
   - `npm run build` passes with 0 errors.
2. **Playback Verification**:
   - Click a Spotify track in Sonora.
   - Confirm audio plays immediately through macOS speakers.
   - Verify Safari does NOT open.
   - Verify Spotify desktop app is NOT required.
   - Test Play, Pause, Seek, and Volume sliders in Sonora's player bar.
3. **UI Verification**:
   - Inspect window titlebar: confirm "Sonora" is centered, traffic lights are clean, and green badge is gone.
