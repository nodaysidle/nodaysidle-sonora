# AGENT.md — Sonora

## Project
**Sonora** — High-performance native desktop music player uniting Spotify, YouTube Music, and Local files in one polished client. Built with **Tauri v2 + Rust + React 19 + TypeScript + Tailwind CSS**. Features gapless playback, EBU R128 audio normalization, synced lyrics with non-Latin romanization, dynamic themes, and native OS integrations (macOS, Linux, Windows).

## BUILD & RUN INSTRUCTIONS
```bash
# In project root (/Volumes/omarchyuser/projekti/sonora)
npm install
npm run tauri dev        # Run native desktop app in development
npm run tauri build      # Compile production release binary / bundle
```

---

## RULES — READ THESE FIRST

### ✅ Do
1. **True Native Performance**: Keep idle memory under 100MB and CPU under 2% during playback. Offload heavy audio decoding, metadata extraction, and database searches to Rust.
2. **Deterministic Gapless Audio**: Use a double-buffering pre-roll mechanism in the Rust audio thread (`cpal` + `symphonia`) so that transitions between consecutive tracks have zero audible delay or click.
3. **EBU R128 Loudness Normalization**: Normalize track loudness to -14.0 LUFS with smooth gain transitions. Never hard-clip or drastically compress dynamic range.
4. **First-Class Lyrics & Romanization**: Provide word/line synchronized lyrics with instant script detection and romanization (Romaji, Hangul RR, Pinyin, Cyrillic Latin).
5. **Unified Canvas**: Treat Local files, Spotify tracks, and YouTube Music streams as first-class citizens in playlists, queues, and search.
6. **Glassmorphic Native Design**: Use Tailwind tokens, native window vibrancy (`window-vibrancy` / macOS Liquid Glass / Windows Mica), and dynamic album art tinting.
7. **Compile with Zero Errors**: Every step must compile cleanly via `npm run build` and `cargo check`.

### ❌ Don’t
1. **NO Heavy Electron Wrappers**: Do NOT introduce Electron, Chromium subprocesses, or embedded browser overhead. Use Tauri v2.
2. **NO Web Audio Decoding for Local Audio**: Do NOT decode large local FLAC/MP3 files entirely in JavaScript memory. Decoders must run in Rust.
3. **NO Blocking the UI Thread**: Long operations like scanning 10,000 local audio tracks must run on Rust background threads with progress events emitted to the frontend.
4. **NO Mock Stubs or Placeholders**: Avoid dead `TODO` comments or fake buttons. Wire real controls to Zustand stores and Tauri commands.
5. **NO Hardcoded Plain Light Theme**: Sonora is dark-mode and glassmorphic by default.

---

## FILES TO CREATE & MAINTAIN

### Rust Backend (`src-tauri/src/`)
- `src-tauri/src/main.rs` — Binary entry point (invokes `sonora_lib::run()`)
- `src-tauri/src/lib.rs` — Tauri setup, window vibrancy, command handlers registration
- `src-tauri/src/audio/engine.rs` — Symphonia + CPAL audio pipeline, ring buffer, gapless transitions
- `src-tauri/src/audio/normalization.rs` — EBU R128 loudness scanning and gain calculations
- `src-tauri/src/library/scanner.rs` — Recursive music folder scanner using `lofty`
- `src-tauri/src/db/mod.rs` — SQLite connection pool, FTS5 migrations, track queries
- `src-tauri/src/lyrics/mod.rs` — LRCLIB client, LRC parser, romanization transliterator
- `src-tauri/src/providers/spotify.rs` — Spotify PKCE OAuth and Web API bridge
- `src-tauri/src/providers/ytmusic.rs` — YouTube Music InnerTube stream resolver

### Frontend UI (`src/`)
- `src/main.tsx` — React 19 application mount
- `src/App.tsx` — Root shell hosting Sidebar, Main View, Lyrics Overlay, Player Bar
- `src/index.css` — Tailwind CSS directives, glassmorphic utility classes, scrollbar styles
- `src/types/index.ts` — TypeScript definitions (Track, Album, Playlist, PlaybackState, Lyrics)
- `src/stores/playerStore.ts` — Zustand audio player state (track, queue, position, volume, normalization)
- `src/stores/libraryStore.ts` — Zustand store for indexed tracks, albums, playlists, active source filter
- `src/stores/themeStore.ts` — Theme selector (Obsidian, Cyber Volt, Nord, Dynamic Album Glow)
- `src/services/tauriBridge.ts` — Typed wrappers for invoking Tauri Rust commands & listening to events
- `src/services/lyricsService.ts` — LRCLIB fetcher and LRC timecode parser
- `src/services/romanization.ts` — Japanese (Romaji), Korean, and Chinese text transliteration
- `src/components/layout/Shell.tsx` — Window header with traffic light padding and main container
- `src/components/sidebar/Sidebar.tsx` — Left navigation (Library, Playlists, Providers switcher)
- `src/components/player/PlayerBar.tsx` — Bottom transport controls, scrubber, volume, lyrics toggle
- `src/components/lyrics/SyncedLyricsView.tsx` — Synchronized scrolling lyrics with active line glow & Romaji toggle
- `src/components/library/LibraryView.tsx` — Grid/Table browser for tracks, albums, and artists

---

## READY-TO-USE AGENT PROMPTS

When you hand this repository to an AI agent, use one of the following prompts depending on your goal:

### Recommended Master Prompt (Full Implementation & Application Install):
```markdown
Read all spec documents in order: AGENT.md, PRD.md, ARD.md, TRD.md, and TASKS.md. 

The Sonora project has already been scaffolded with Tauri v2, Rust backend, and React 19 + TypeScript + Tailwind frontend. All files compile with 0 errors.

Your task is to implement the core features following the roadmap in TASKS.md:
1. Design a custom Sonora logo (audio waveform + acoustic resonance motif in SVG) and generate high-resolution app icons (`icon.icns`, `icon.ico`, 32x32, 128x128, 256x256, 512x512) in `src-tauri/icons/`.
2. Complete the Rust local library scanner (Phase 2) using `lofty` to extract metadata and store tracks in the SQLite database with FTS5 search.
3. Build the Rust audio engine (Phase 3) with `cpal` and `symphonia` supporting sample-accurate gapless playback and EBU R128 loudness normalization.
4. Wire the React frontend (Phase 4 & Phase 5) with a modern glassmorphic UI, persistent bottom player bar, library browsing, and synchronized scrolling lyrics with the 3-way romanization toggle (Original / Romaji / Dual).
5. Integrate the Spotify and YouTube Music provider interfaces (Phase 6).
6. Compile the production release binary and macOS application bundle (`npm run tauri build`).
7. Install the compiled `Sonora.app` into the macOS `/Applications/` directory (`cp -R src-tauri/target/release/bundle/macos/Sonora.app /Applications/`) and verify it launches cleanly from Spotlight / Applications.

Ensure every file compiles cleanly (`npm run build` and `cd src-tauri && cargo check`) and there are no placeholder stubs. Start with Phase 2 and proceed systematically until Sonora.app is installed in /Applications.
```

### Focused Audio & Engine Prompt:
```markdown
Read AGENT.md and TRD.md. Focus specifically on Phase 2 & Phase 3 of TASKS.md:
Implement the Rust native audio engine in `src-tauri/src/audio/engine.rs` and `src-tauri/src/audio/normalization.rs`.
Set up Symphonia decoding for FLAC/MP3/AAC/OGG with CPAL output, dual-decoder pre-buffering for gapless transitions, and real-time -14 LUFS loudness normalization. Wire the Tauri IPC commands and verify with `cargo test`.
```

### Focused UI & Synced Lyrics Prompt:
```markdown
Read AGENT.md, PRD.md, and TRD.md. Focus on Phase 4 & Phase 5:
Build the complete interactive frontend experience in React 19:
1. Polished glassmorphic Layout, Sidebar, and persistent PlayerBar with smooth scrubber and volume controls.
2. The SyncedLyricsView component with automatic scrolling, active line pulse, and interactive romanization toggle (Kanji/Kana -> Romaji, Hangul, Pinyin).
3. Connect all components to the Zustand stores and Tauri bridge.
Verify that `npm run build` passes with zero TypeScript errors.
```
