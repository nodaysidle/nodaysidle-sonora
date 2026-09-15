# CLAUDE.md — Sonora

## Build, Test & Run Commands
- **Install dependencies**: `npm install`
- **Run dev (desktop app)**: `npm run tauri dev`
- **Frontend only dev**: `npm run dev` (Vite port 1420 / 5173)
- **Frontend build & typecheck**: `npm run build` (`tsc && vite build`)
- **Backend check**: `cd src-tauri && cargo check`
- **Backend test**: `cd src-tauri && cargo test`
- **Full production build**: `npm run tauri build`

## Architecture & Conventions
- **Stack**: Tauri v2, Rust 2021, React 19, TypeScript, Vite, Tailwind CSS, Zustand, SQLite (rusqlite).
- **Style**: Dark-mode glassmorphism, native window vibrancy (`window-vibrancy`), high-contrast accents (#C8FF00 / Cybervolt, Obsidian, Nord).
- **Performance**: Rust handles all audio decoding (`symphonia` + `cpal`), local file metadata extraction (`lofty`), and SQLite FTS5 queries.
- **Audio Engine**: Gapless playback via dual-buffer pre-roll; loudness normalization via EBU R128 (-14 LUFS target).
- **Lyrics & Romanization**: Real-time LRCLIB synced lyrics with automated script detection and transliteration (Romaji, Hangul, Pinyin).
- **Conventions**: ES Modules (`"type": "module"`), functional React hooks only, strict TypeScript types, zero mock placeholders.
