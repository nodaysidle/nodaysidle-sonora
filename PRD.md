# PRD — Sonora (Native Universal Music Client)

## 1. Executive Summary & Vision
**Sonora** is a high-performance, native-feeling desktop music player for **macOS, Linux, and Windows**. It unifies music listening across three primary audio sources:
1. **Local Audio Files** (FLAC, MP3, AAC, ALAC, OGG, OPUS, WAV)
2. **Spotify** (Library playlists, saved tracks, albums, playback control)
3. **YouTube Music** (Search, streaming, recommendations, artist catalogs)

Unlike web wrappers (Electron/webviews running heavy web instances), Sonora leverages **Tauri v2 + Rust** to deliver a sub-100MB RAM footprint, instant responsiveness, native OS window vibrancy (macOS Glassmorphism/Liquid Glass, Windows Acrylic/Mica, Linux frosted blur), native media key integration, gapless audio playback with sample-accurate transitions, EBU R128 loudness normalization, synchronized lyrics with automatic romanization for non-Latin scripts (Japanese, Korean, Chinese, Cyrillic), and dynamic customizable themes.

---

## 2. Core User Personas
- **The Hybrid Audiophile**: Keeps high-resolution local FLACs on disk, but relies on Spotify/YT Music for discovering new releases and streaming casual playlists. Wants one unified library without app-switching.
- **The Global & Anime/K-Pop Listener**: Listens to Japanese, Korean, Mandarin, or Slavic artists and wants synchronized scrolling lyrics with immediate romanization/transliteration so they can sing along without manually searching web romanizations.
- **The Focused Desktop Worker**: Wants a clean, minimalist, distraction-free player with global hotkeys, native OS media integration, seamless transitions without volume jumps between songs (audio normalization), and gapless playback for live albums and concept records.

---

## 3. Product Principles
1. **Real Native Feel, Zero Bloat**: Startup time < 400ms, idle memory < 90MB, 60/120fps animations. Avoid heavy embedded browser overhead.
2. **Unified Canvas**: Local files, Spotify tracks, and YouTube Music streams live alongside each other in playlists, queues, and search results.
3. **Studio-Grade Audio Fidelity**:
   - True gapless playback with background track pre-decoding.
   - EBU R128 / ReplayGain loudness normalization (-14 LUFS standard) to eliminate jarring loudness differences across streaming vs local files.
4. **Delightful Lyric Experience**:
   - Word-by-word or line-by-line synchronized scrolling lyrics (via LRCLIB / embedded synced tags).
   - Instant 1-click romanization toggle (Kanji/Kana -> Romaji, Hangul -> Romaja, Pinyin, Cyrillic -> Latin).
5. **Aesthetic Customization**: Dynamic album art background tinting, dark-mode first, glassmorphism, accent palette customization.

---

## 4. Key Functional Features

### 4.1 Unified Multi-Provider Architecture
- **Local File Engine**:
  - Recursive folder watch (`notify` crate in Rust).
  - Fast metadata parsing (tags, artwork, sample rate, bit depth, ReplayGain) using `lofty` in Rust.
  - SQLite local index with instant full-text search across title, artist, album, and year.
- **Spotify Integration**:
  - OAuth 2.0 PKCE authentication (no secret key stored in client).
  - Spotify Web API client for playlists, favorites, recommendations, and search.
  - Web Playback SDK / Connect bridge for playback state synchronization.
- **YouTube Music Integration**:
  - InnerTube client for high-quality audio streaming, playlist discovery, and track lookup.
  - Seamless stream resolution directly into Sonora's Rust/Web audio pipeline.

### 4.2 Playback Engine
- **Queue Management**: Play next, append to queue, shuffle (Fisher-Yates with history preservation), repeat track / repeat queue.
- **Gapless Playback**: Dual audio stream buffering; track $N+1$ begins decoding 5 seconds before track $N$ finishes, scheduling seamless crossfade or sample-accurate boundary splicing.
- **Loudness Normalization**: EBU R128 integrated loudness scanning or ReplayGain tag consumption. Targets configurable LUFS (default -14 LUFS, 1.0 dB true peak ceiling).
- **Audio Output**: Device selection (ASIO/WASAPI on Windows, CoreAudio on macOS, ALSA/PulseAudio/PipeWire on Linux) via CPAL / Symphonia.

### 4.3 Synced Lyrics & Non-Latin Romanization
- **Synced Lyrics Display**: Full-screen immersive view and compact player sheet with active line glow, auto-scroll, and click-to-seek timestamp support.
- **Provider Resolution**: Automatic fallback: Local embedded SYLT/USLT tags -> LRCLIB API -> NetEase / Musixmatch fallback.
- **Romanization Engine**:
  - Japanese: Romaji (Hepburn) conversion for Kanji/Kana.
  - Korean: Revised Romanization of Korean (RR) for Hangul.
  - Chinese: Pinyin transliteration for Hanzi.
  - Cyrillic / Greek: ISO 9 / ALA-LC Latin transliteration.
  - Single-click toggle: `Original`, `Romanized`, or `Dual (Original + Romaji)`.

### 4.4 Library & UI/UX
- **Navigation**: Home (Quick Picks, Recently Played), Library (Tracks, Albums, Artists, Playlists, Local Folders), Search (Unified instant search with provider filters).
- **Theme Engine**:
  - Presets: Obsidian Glass, Midnight Slate, Cyber Volt (#C8FF00), Rosé Pine, Nord Frost, Solarized Dark, AMOLED Black.
  - Adaptive Dynamic Mode: Extracts vibrant and muted dominant colors from the current playing album cover to subtly illuminate the background and glow effects.
- **Native OS Integrations**:
  - MPRIS v2 (Linux media controller integration).
  - SMTC (System Media Transport Controls on Windows).
  - `MPNowPlayingInfoCenter` / macOS media keys & Control Center widget.
  - Global hotkeys (Play/Pause, Next, Prev, Mute, Volume Up/Down).

---

## 5. Non-Functional Requirements
- **Performance**: Track switch latency < 150ms for local files, < 800ms for network streams.
- **Storage**: Local metadata cache stored in standard OS user data directories (`~/Library/Application Support/sonora`, `%APPDATA%\sonora`, `~/.config/sonora`).
- **Security**: Spotify tokens stored safely in OS secure keyring (`keyring-rs` / Tauri secure store plugin). No telemetry or tracking.
- **Reliability**: Graceful offline mode (local library works 100% without internet connectivity).

---

## 6. Success Metrics
- CPU utilization < 2% during audio playback on Apple Silicon & modern x86.
- Zero audio popping/clipping during track transitions.
- Instant search response (< 20ms for 50,000 indexed local tracks).
