# Codemap — Sonora

## Overview
Sonora is a native cross-platform desktop music player (macOS, Linux, Windows) that unifies Spotify, YouTube Music, and Local Audio files with gapless playback, audio normalization, synced lyrics, and non-Latin romanization.

## Directory Structure
```
sonora/
├── PRD.md                  # Product Requirements Document
├── ARD.md                  # Architecture Reference Document
├── TRD.md                  # Technical Requirements & Contracts
├── TASKS.md                # Phased Implementation Roadmap
├── AGENT.md                # Hard constraints, files to create, agent prompts
├── CLAUDE.md               # Quick development rules & commands
├── codemap.md              # Codebase architectural map
├── README.md               # User & contributor documentation
├── package.json            # Node.js dependencies & scripts
├── tsconfig.json           # TypeScript configuration
├── vite.config.ts          # Vite build configuration
├── src-tauri/              # Rust Native Backend
│   ├── Cargo.toml          # Rust dependencies (tauri, symphonia, cpal, lofty, rusqlite)
│   ├── tauri.conf.json     # Tauri v2 window and security settings
│   ├── build.rs            # Tauri build script
│   ├── capabilities/       # Tauri v2 permission capabilities
│   └── src/
│       ├── main.rs         # Tauri binary entry
│       ├── lib.rs          # Command registration & setup
│       ├── audio/          # Native audio engine
│       │   ├── engine.rs   # Symphonia + CPAL gapless playback pipeline
│       │   └── normalization.rs # EBU R128 loudness scanning & gain filter
│       ├── library/        # Local media management
│       │   └── scanner.rs  # Multi-threaded folder scanner & Lofty tagger
│       ├── db/             # Persistence
│       │   └── mod.rs      # SQLite pool & FTS5 full-text search
│       ├── lyrics/         # Lyrics & Romanization
│       │   └── mod.rs      # LRCLIB API & transliteration engines
│       └── providers/      # Streaming service bridges
│           ├── spotify.rs  # Spotify Web API & PKCE Auth
│           └── ytmusic.rs  # YouTube Music InnerTube stream resolution
└── src/                    # React 19 Frontend
    ├── main.tsx            # React application entry point
    ├── App.tsx             # Root application shell
    ├── index.css           # Tailwind CSS & glassmorphic styles
    ├── types/              # Domain models (Track, Album, Playlist, Lyrics)
    ├── stores/             # Zustand state management
    │   ├── playerStore.ts  # Playback state, transport, queue
    │   ├── libraryStore.ts # Tracks catalog, filters, source switcher
    │   └── themeStore.ts   # Glassmorphic & dynamic album themes
    ├── services/           # Backend communication & logic
    │   ├── tauriBridge.ts  # Typed Tauri command & event bindings
    │   ├── lyricsService.ts# Synced lyrics fetcher & LRC time parser
    │   └── romanization.ts # CJK & Cyrillic romanization utility
    └── components/         # UI Components
        ├── layout/         # Shell, Draggable Titlebar
        ├── sidebar/        # Navigation & Provider Switcher
        ├── player/         # Bottom Player Bar, Scrubber, Volume
        ├── library/        # Tracks Table, Albums Grid, Search
        └── lyrics/         # Synced Lyrics view with Romaji toggle
```

## Data Flow
1. **Local Audio**: `library_scan_directory` (Rust) -> `lofty` tags -> SQLite FTS5 -> `libraryStore` (UI).
2. **Playback**: User click -> `playback_load_track` (Rust) -> `symphonia` decodes -> double-buffer ring -> `ebur128` gain filter -> `cpal` device stream.
3. **Lyrics**: Track play -> `lyrics_get_for_track` -> LRCLIB / ID3 tags -> Romanizer (Kana/Hangul/Pinyin) -> `SyncedLyricsView` with live progress sync.
