# Sonora

Sonora is a native desktop music player for local files, YouTube Music, and Spotify.
The app uses Tauri 2, Rust 2021, React 19, TypeScript, Vite, Tailwind CSS, Zustand, and SQLite.
Local and YouTube audio use the Rust Symphonia/CPAL engine; Spotify plays natively through Librespot.
This file is the repository source of truth. When older planning documents disagree, follow this file and the working source.

# Start Here

Work from `/Volumes/omarchyuser/projekti/sonora`.
Read `AGENTS.md`, then `docs/codemap.md`; consult `docs/ARD.md` and `docs/TRD.md` for subsystem contracts.
Treat `docs/TASKS.md` as historical planning, not reliable completion status.
Inspect the current implementation and tests before changing behavior.
Preserve unrelated working-tree changes; never reset, discard, or overwrite them.

# Commands

Run `npm install` to install frontend and Tauri CLI dependencies.
Run `npm run tauri dev` for the desktop app; Vite uses strict port 1420.
Run `npm run dev` only when frontend-only development is sufficient.
Run `npm run build` for strict TypeScript checking and the Vite production build.
Run `cargo check --manifest-path src-tauri/Cargo.toml` for a fast Rust verification.
Run `cargo test --manifest-path src-tauri/Cargo.toml` for Rust unit and integration tests.
Run `npm run tauri build` only when a production bundle is required.
The library integration tests need FFmpeg; playback smoke tests need an audio output device and otherwise skip.

# Architecture

`src/` contains the React UI, Zustand stores, domain types, and typed Tauri bridge.
`src-tauri/src/lib.rs` owns Tauri setup, application state, command registration, and playback coordination.
`src-tauri/src/audio/` owns local and YouTube decoding, buffering, output, and normalization.
`src-tauri/src/providers/spotify/` owns Spotify API access and native Librespot playback.
`src-tauri/src/library/` and `src-tauri/src/db/` own scanning, metadata extraction, SQLite, and FTS5.
Keep TypeScript and Rust models, command names, event names, and provider URI formats synchronized.

# Conventions

Keep heavy audio decoding, metadata extraction, database work, and long-running operations in Rust.
Never block the React UI thread; send progress and playback state through Tauri events.
Use strict TypeScript, functional React components and hooks, ES modules, and typed bridge wrappers.
Keep local, YouTube, and Spotify tracks first-class in queues, search, transport controls, and state.
Coordinate the Symphonia/CPAL and Librespot players so only the selected source owns playback.
Preserve native gapless playback, smooth volume handling, synced lyrics, and the dark glass interface.
Store credentials and session material only through the OS keychain or application data directory.
Add or update tests for behavior changes; do not replace working paths with mocks or placeholders.

# Do Not Touch

Do not edit generated `dist/`, `node_modules/`, `src-tauri/target/`, or `src-tauri/gen/` content.
Do not introduce Electron or move local audio decoding into JavaScript or Web Audio.
Do not commit credentials, OAuth tokens, session caches, personal music, logs, or machine-local files.
Do not install to `/Applications`, publish releases, commit, push, or rewrite history without explicit approval.

# Done Gate

Before reporting code work complete, run `npm run build`, `cargo check --manifest-path src-tauri/Cargo.toml`, and the relevant Rust tests.
For playback changes, launch the Tauri app and exercise the affected local, YouTube, or Spotify path when credentials and hardware are available.
Report skipped hardware-, credential-, or FFmpeg-dependent checks explicitly; never present them as passes.
