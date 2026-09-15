# TASKS — Implementation Roadmap: Sonora

## Overview
This document tracks the phased execution plan for Sonora. Each phase contains clear deliverables, acceptance criteria, and verification commands.

---

### Phase 1: Project Scaffolding & Native Window Setup
- [x] **Task 1.1**: Initialize Tauri v2 project structure with Rust backend and React 19 + TypeScript + Vite frontend.
- [x] **Task 1.2**: Configure Tailwind CSS with glassmorphic tokens, CSS backdrop blur, and custom scrollbars.
- [x] **Task 1.3**: Configure native window vibrancy (macOS Liquid Glass, Windows Mica/Acrylic, Linux transparent canvas) in `tauri.conf.json`.
- [ ] **Task 1.4**: Establish core app layout shell: Header with traffic light draggable region, Sidebar navigation, Main View canvas, and persistent Bottom Player Bar.

*Acceptance Criteria*: App compiles and launches a transparent glassmorphic desktop window with draggable titlebar and responsive layout shell.
*Validation*: `npm run tauri dev`

---

### Phase 2: Core Database & Local File Scanner
- [ ] **Task 2.1**: Implement SQLite connection pool and schema migrations (`tracks`, `tracks_fts`, `playlists`, `lyrics_cache`).
- [ ] **Task 2.2**: Integrate `lofty` crate in Rust for high-speed audio metadata extraction (ID3v2, Vorbis, FLAC, MP4/AAC, embedded artwork).
- [ ] **Task 2.3**: Build recursive multi-threaded folder scanner (`library_scan_directory`) with real-time scan progress events to UI.
- [ ] **Task 2.4**: Implement fast full-text search Tauri command (`library_get_tracks`) leveraging SQLite FTS5.

*Acceptance Criteria*: Pointing Sonora at a music folder indexes 1,000+ songs in under 3 seconds with metadata and embedded album art cached and searchable.
*Validation*: `cargo test --lib library::scanner`

---

### Phase 3: Rust Native Audio Engine (Gapless & Normalization)
- [ ] **Task 3.1**: Implement audio playback stream using `cpal` and `symphonia` for bit-perfect decoding of FLAC, MP3, AAC, OGG, and WAV.
- [ ] **Task 3.2**: Build the dual-decoder ring buffer for sample-accurate gapless playback with pre-roll buffering of the next queue track.
- [ ] **Task 3.3**: Integrate EBU R128 loudness normalization filter (`ebur128` crate) targeting -14.0 LUFS with smooth gain ramping to prevent clipping.
- [ ] **Task 3.4**: Expose Tauri commands for transport control: `playback_load_track`, `playback_play`, `playback_pause`, `playback_seek`, `playback_set_volume`, and `playback_toggle_normalization`.

*Acceptance Criteria*: Continuous playback without stutter; gapless transition between consecutive tracks with zero audible click/pause; loudness normalization balances quiet and loud tracks smoothly.
*Validation*: `cargo test --lib audio::engine`

---

### Phase 4: Modern Player UI & Library Browsing
- [ ] **Task 4.1**: Build persistent Bottom Player Bar: track artwork thumbnail, scrolling title/artist, play/pause/prev/next, shuffle/repeat, progress bar with hover timestamp tooltip, and volume slider.
- [ ] **Task 4.2**: Implement Library views:
  - Tracks Table: sortable by Title, Artist, Album, Duration, Format.
  - Albums Grid: responsive album cover art grid with hover play action.
  - Artists View: grouped artist catalog with discography.
- [ ] **Task 4.3**: Build Interactive Queue Drawer with drag-and-drop reordering, "Up Next", and "History".
- [ ] **Task 4.4**: Implement Unified Search bar with instant autocomplete and provider badges (Local, Spotify, YouTube).

*Acceptance Criteria*: Smooth 60fps UI; clicking any song plays instantly; queue responds immediately to reordering; search filters tracks in real time.
*Validation*: `npm run build && npm run tauri dev`

---

### Phase 5: Synced Lyrics Engine & Non-Latin Romanization
- [ ] **Task 5.1**: Build lyrics resolution pipeline: local embedded tags -> LRCLIB API client -> local SQLite cache.
- [ ] **Task 5.2**: Implement timestamp synchronization parser with millisecond precision and line-highlight tracking synchronized to playback progress events.
- [ ] **Task 5.3**: Build the Romanization engine:
  - Japanese (Kanji/Kana -> Romaji).
  - Korean (Hangul -> Romanized Korean).
  - Chinese (Hanzi -> Pinyin).
  - Cyrillic -> Latin transliteration.
- [ ] **Task 5.4**: Build Synced Lyrics UI: Fullscreen/Sheet view with active line glow, auto-scroll, manual click-to-seek, and 3-way toggle button (`Original` / `Romanized` / `Dual`).

*Acceptance Criteria*: Playing a Japanese/Korean/Chinese track automatically displays synchronized scrolling lyrics with accurate romaji above or replacing the original text.
*Validation*: `npm test -- lyricsService`

---

### Phase 6: Streaming Providers Integration (Spotify & YouTube Music)
- [ ] **Task 6.1**: Implement Spotify OAuth 2.0 PKCE authentication flow opening system browser with local loopback callback.
- [ ] **Task 6.2**: Implement Spotify Web API client for fetching user playlists, saved albums, top tracks, and search.
- [ ] **Task 6.3**: Implement YouTube Music InnerTube client for searching tracks, albums, videos, and extracting high-bitrate Opus audio streams.
- [ ] **Task 6.4**: Build Unified Library Switcher allowing users to view "All Sources", "Local Only", "Spotify", or "YouTube Music".

*Acceptance Criteria*: User can browse their Spotify playlists and stream YouTube Music tracks within the same unified Sonora interface.
*Validation*: `cargo test --lib providers`

---

### Phase 7: Themes & Native Polish
- [ ] **Task 7.1**: Implement Theme System with presets: Obsidian Glass, Midnight Slate, Cyber Volt (#C8FF00), Rosé Pine, Nord Frost, AMOLED Pitch Black.
- [ ] **Task 7.2**: Implement Adaptive Dynamic Theme: Color extraction from active track's album cover art to dynamically illuminate background gradients and player glow.
- [ ] **Task 7.3**: Integrate OS Native Media Key Controls and Now Playing indicators:
  - macOS: `MPNowPlayingInfoCenter` bridge.
  - Linux: MPRIS v2 D-Bus service.
  - Windows: SMTC (System Media Transport Controls).
- [ ] **Task 7.4**: Implement system tray / menubar mini-controller with play/pause and track skipping.

*Acceptance Criteria*: System media keys control Sonora playback even when app is minimized; MPRIS/Control Center shows song title, artist, and album artwork.
*Validation*: `npm run tauri dev`

---

### Phase 8: Final Packaging & Production Release
- [ ] **Task 8.1**: Design custom Sonora app logo (SVG waveform + acoustic ring) and generate icon set (`icon.icns`, `icon.ico`, 32x32, 128x128, 256x256, 512x512).
- [ ] **Task 8.2**: Configure release build profiles with LTO and strip for minimal binary size.
- [ ] **Task 8.3**: Compile production release bundle (`npm run tauri build`).
- [ ] **Task 8.4**: Install compiled `Sonora.app` into the macOS `/Applications/` folder (`cp -R src-tauri/target/release/bundle/macos/Sonora.app /Applications/`) and verify native launch.

*Acceptance Criteria*: Standalone desktop application builds cleanly, installs into `/Applications/Sonora.app`, and launches with full audio, lyrics, and provider functionality.
*Validation*: `npm run tauri build && test -d /Applications/Sonora.app`
