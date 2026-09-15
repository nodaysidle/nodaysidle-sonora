# Native In-App Spotify Streaming & UI Header Refinement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Embed `librespot` directly into Sonora's Rust backend to deliver 100% standalone, in-app 320 kbps Spotify audio streaming without needing Spotify Desktop or Safari, and center the "Sonora" title in the window titlebar while removing the green badge.

**Architecture:** Integrate `librespot` in `src-tauri` with `Spirc` and `rodio-backend`/CPAL output. Wire transport commands from Tauri IPC directly to the internal Spirc instance and emit playback progress events. In the React frontend, center "Sonora" in `Shell.tsx` and update the player badge to "Spotify (Native)".

**Tech Stack:** Tauri v2, Rust (librespot 0.8, cpal, rodio-backend, native-tls, tokio), React 19, TypeScript, Tailwind CSS.

**Spec:** [`docs/superpowers/specs/2026-09-15-librespot-native-streaming-design.md`](file:///Volumes/omarchyuser/projekti/sonora/docs/superpowers/specs/2026-09-15-librespot-native-streaming-design.md)

## Global Constraints
- Keep idle memory under 120MB and CPU under 2% during playback.
- No external browser windows or processes launched during playback.
- Every task must compile cleanly with `npm run build` and `cargo check`.
- Use macOS native keychain and app data directories.

---

### Task 1: Center Window Header & Remove Green Badge (`Shell.tsx`)

**Files:**
- Modify: `src/components/layout/Shell.tsx`
- Test: `npm run build`

**Interfaces:**
- Consumes: `<Shell>{children}</Shell>`
- Produces: Clean minimalist titlebar with centered "Sonora" and no green tag

- [ ] **Step 1: Update `Shell.tsx` markup**

```tsx
import React from 'react';

interface ShellProps {
  children: React.ReactNode;
}

export const Shell: React.FC<ShellProps> = ({ children }) => {
  return (
    <div className="flex flex-col h-screen w-screen overflow-hidden bg-zinc-950/45 text-zinc-100 selection:bg-lime-400 selection:text-black">
      {/* Window titlebar drag region with centered title */}
      <header
        data-tauri-drag-region
        className="relative h-11 w-full flex items-center justify-center select-none shrink-0 border-b border-white/5 bg-black/25 backdrop-blur-xl"
      >
        <div className="absolute inset-x-0 flex items-center justify-center pointer-events-none">
          <span className="text-xs font-semibold tracking-[0.22em] text-zinc-300 uppercase">
            Sonora
          </span>
        </div>
      </header>

      {/* Workspace */}
      <div className="relative isolate ambient-wash flex-1 flex flex-col min-h-0 overflow-hidden">
        {children}
      </div>
    </div>
  );
};
```

- [ ] **Step 2: Run frontend build to verify**

Run: `npm run build`
Expected: PASS with 0 errors

- [ ] **Step 3: Commit**

```bash
git add src/components/layout/Shell.tsx
git commit -m "style(ui): center sonora title in top bar and remove green badge"
```

---

### Task 2: Add `librespot` Dependencies & Streaming Scopes

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/providers/spotify.rs`
- Test: `cargo check`

**Interfaces:**
- Consumes: `librespot` 0.8.0 crate
- Produces: `streaming` OAuth scope and access to `librespot::core`, `librespot::playback`, and `librespot::connect`

- [ ] **Step 1: Add `librespot` to `src-tauri/Cargo.toml`**

Add under `[dependencies]`:
```toml
librespot = { version = "0.8", default-features = false, features = ["native-tls", "rodio-backend", "with-libmdns"] }
```

- [ ] **Step 2: Update `SCOPES` in `src-tauri/src/providers/spotify.rs`**

Update `SCOPES` constant to include `streaming`:
```rust
const SCOPES: &str = "streaming user-read-private user-read-email playlist-read-private playlist-read-collaborative \
                      user-library-read user-top-read user-read-playback-state user-modify-playback-state";
```

- [ ] **Step 3: Verify compilation**

Run: `cd src-tauri && cargo check`
Expected: PASS (downloads and compiles `librespot`)

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/providers/spotify.rs
git commit -m "build(deps): add librespot and streaming oauth scope"
```

---

### Task 3: Build In-Process Native Spotify Player Engine

**Files:**
- Create: `src-tauri/src/providers/spotify/native_player.rs`
- Modify: `src-tauri/src/providers/spotify.rs`
- Test: `cargo test --lib providers::spotify`

**Interfaces:**
- Consumes: Spotify track IDs (`spotify://track/<id>` or `spotify:track:<id>`), Spotify access tokens
- Produces: `NativeSpotifyPlayer` with methods:
  - `play_track(&self, uri: &str) -> Result<(), String>`
  - `pause(&self) -> Result<(), String>`
  - `resume(&self) -> Result<(), String>`
  - `seek(&self, position_ms: u64) -> Result<(), String>`
  - `set_volume(&self, volume: f32) -> Result<(), String>`
  - `playback_state(&self) -> SpotifyPlaybackState`

- [ ] **Step 1: Write failing unit tests for URI parsing and ID normalization**

Create `src-tauri/src/providers/spotify/native_player.rs` with:
```rust
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
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --manifest-path src-tauri/Cargo.toml parses_various_spotify_uris`
Expected: FAIL (function not found)

- [ ] **Step 3: Implement `NativeSpotifyPlayer` and URI normalization**

Implement in `src-tauri/src/providers/spotify/native_player.rs`:
- Helper `normalize_spotify_id`
- `NativeSpotifyPlayer` struct managing `librespot_core::Session`, `Spirc`, and `Player`
- Background audio thread with CPAL sink and volume controls

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --manifest-path src-tauri/Cargo.toml parses_various_spotify_uris`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/providers/spotify/native_player.rs src-tauri/src/providers/spotify.rs
git commit -m "feat(spotify): implement native librespot player engine"
```

---

### Task 4: Connect Native Player to Tauri Commands & Audio Coordination

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/providers/spotify.rs`
- Test: `cargo check`

**Interfaces:**
- Consumes: `NativeSpotifyPlayer`
- Produces: Updated `spotify_play`, `spotify_pause`, `spotify_resume`, `spotify_seek`, `spotify_set_volume` that route to `NativeSpotifyPlayer` while stopping `AudioEngine` to prevent dual playback.

- [ ] **Step 1: Wire Native Player in `AppState` and commands in `lib.rs`**
- [ ] **Step 2: Forward player progress to `sonora://playback-progress`**
- [ ] **Step 3: Verify with `cargo check`**
- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/providers/spotify.rs
git commit -m "feat(audio): coordinate native spotify player with audio engine in tauri commands"
```

---

### Task 5: Update Frontend Badge & End-to-End Build

**Files:**
- Modify: `src/components/player/PlayerBar.tsx`
- Test: `npm run build && cd src-tauri && cargo check`

**Interfaces:**
- Consumes: Player state
- Produces: `Spotify (Native)` badge in PlayerBar

- [ ] **Step 1: Update badge in `PlayerBar.tsx`**

Change line 160 from:
```tsx
? `Spotify${spotifyDeviceName ? ` · ${spotifyDeviceName}` : ''}`
```
to:
```tsx
? (spotifyDeviceName ? `Spotify · ${spotifyDeviceName}` : 'Spotify (Native)')
```

- [ ] **Step 2: Run complete frontend build**

Run: `npm run build`
Expected: PASS with 0 errors

- [ ] **Step 3: Build release binary & verify**

Run: `npm run tauri build`
Expected: Generates fresh `Sonora.app` bundle in `src-tauri/target/release/bundle/macos/`

- [ ] **Step 4: Update `/Applications/Sonora.app`**

```bash
cp -R src-tauri/target/release/bundle/macos/Sonora.app /Applications/
```

- [ ] **Step 5: Commit**

```bash
git add src/components/player/PlayerBar.tsx
git commit -m "feat(ui): update player bar badge to reflect native in-app spotify playback"
```
