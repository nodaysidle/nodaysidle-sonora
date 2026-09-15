# TRD — Technical Requirements Document: Sonora

## 1. Data Models & Schemas

### 1.1 Core TypeScript & Rust Unified Entities

```typescript
export type ProviderType = 'local' | 'spotify' | 'ytmusic';

export interface Track {
  id: string;                      // Canonical URI, e.g. "local:///path/to/song.flac"
  provider: ProviderType;
  title: string;
  artist: string;
  album: string;
  albumArtist?: string;
  durationMs: number;
  year?: number;
  genre?: string;
  artworkUrl?: string;             // Local custom URI "asset://..." or remote HTTPS URL
  bitrate?: number;
  format?: string;                 // "flac", "mp3", "aac", "opus"
  replayGainLufs?: number;         // Integrated loudness in LUFS (-14.0 target)
  trackNumber?: number;
  discNumber?: number;
}

export interface Playlist {
  id: string;
  title: string;
  description?: string;
  artworkUrl?: string;
  provider: ProviderType | 'mixed';
  trackCount: number;
  tracks?: Track[];
}

export interface LyricsLine {
  timeMs: number;
  text: string;
  romanizedText?: string;
}

export interface SyncedLyrics {
  trackId: string;
  lines: LyricsLine[];
  hasRomanization: boolean;
  provider: 'embedded' | 'lrclib' | 'netease';
}

export interface PlaybackState {
  currentTrack: Track | null;
  isPlaying: boolean;
  positionMs: number;
  durationMs: number;
  volume: number;                  // 0.0 to 1.0
  isMuted: boolean;
  isNormalizing: boolean;          // EBU R128 Loudness Normalization enabled
  isGapless: boolean;
  repeatMode: 'off' | 'track' | 'queue';
  shuffle: boolean;
}

export type ThemePreset = 
  | 'obsidian'
  | 'midnight'
  | 'cybervolt'
  | 'rose-pine'
  | 'nord'
  | 'amoled'
  | 'dynamic';
```

---

## 2. Tauri Command Specifications (IPC Interface)

| Command | Arguments | Returns | Description |
|---|---|---|---|
| `playback_load_track` | `track: Track, auto_play: bool` | `Result<(), String>` | Prepares track in audio engine, initializes decoding. |
| `playback_play` | None | `Result<(), String>` | Resumes or starts audio output stream. |
| `playback_pause` | None | `Result<(), String>` | Pauses output stream without resetting decoder position. |
| `playback_seek` | `position_ms: u64` | `Result<(), String>` | Performs sample-accurate seek in current decoder. |
| `playback_set_volume` | `volume: f32` | `Result<(), String>` | Sets linear output volume [0.0 - 1.0]. |
| `playback_toggle_normalization`| `enabled: bool` | `Result<bool, String>` | Toggles real-time EBU R128 loudness matching. |
| `library_scan_directory` | `path: String` | `Result<usize, String>` | Scans local music folder, indexes metadata into SQLite. |
| `library_get_tracks` | `query: Option<String>, limit: u32, offset: u32` | `Result<Vec<Track>, String>` | Queries tracks with optional FTS search. |
| `lyrics_get_for_track` | `track: Track` | `Result<SyncedLyrics, String>` | Resolves embedded or network synced lyrics with romaji. |
| `spotify_authenticate` | None | `Result<bool, String>` | Triggers OAuth PKCE loop in system browser. |
| `ytmusic_search` | `query: String, limit: u32` | `Result<Vec<Track>, String>` | Queries YouTube Music InnerTube endpoint. |
| `ytmusic_resolve_stream` | `video_id: String` | `Result<String, String>` | Extracts direct audio stream URL for playback. |

---

## 3. Tauri Events (Rust -> Frontend)

| Event Name | Payload | Frequency | Purpose |
|---|---|---|---|
| `sonora://playback-progress` | `{ position_ms: u64, duration_ms: u64 }` | Every 100ms | Smooth slider updates and lyrics sync lock. |
| `sonora://playback-status` | `PlaybackState` | On status transition | Updates play/pause buttons, track metadata bar. |
| `sonora://track-ended` | `{ track_id: String }` | On track EOF | Triggers next queue track transition gaplessly. |
| `sonora://scan-progress` | `{ scanned: u32, total: u32, current_file: String }` | Throttled (50ms) | Displays progress bar in Library settings. |
| `sonora://media-key-event` | `{"action": "play_pause" \| "next" \| "prev"}` | On OS shortcut | Media key handling from macOS/Windows/Linux. |

---

## 4. SQLite Database Schema DDL

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS tracks (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    album_artist TEXT,
    duration_ms INTEGER NOT NULL,
    year INTEGER,
    genre TEXT,
    file_path TEXT,
    artwork_url TEXT,
    bitrate INTEGER,
    format TEXT,
    loudness_lufs REAL,
    track_number INTEGER,
    disc_number INTEGER,
    date_added INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
    title,
    artist,
    album,
    content='tracks',
    content_rowid='rowid'
);

CREATE TABLE IF NOT EXISTS playlists (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    artwork_url TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position),
    FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS lyrics_cache (
    track_id TEXT PRIMARY KEY,
    raw_lrc TEXT NOT NULL,
    romanized_json TEXT,
    cached_at INTEGER NOT NULL
);
```

---

## 5. Synced Lyrics & Romanization Specifications

- **LRC Format Support**: Standard `[mm:ss.xx]` and high-precision `[mm:ss.xxx]` timecodes.
- **Romanization Mapping Rules**:
  - Script Detection: Regex matching of Unicode ranges for CJK Unified Ideographs, Hiragana/Katakana, Hangul Syllables, Cyrillic.
  - Romanized payload is structured alongside original lyrics line:
    ```json
    {
      "timeMs": 24350,
      "text": "夜に駆ける",
      "romanizedText": "Yoru ni Kakeru"
    }
    ```
  - UI provides an interactive 3-way toggle button: `[Original] [Romaji] [Dual]`.

---

## 6. Gapless & Loudness Normalization Engine Details

- **Audio Device Buffer**: 1024 or 2048 frames @ 44.1kHz / 48kHz to ensure glitch-free audio on all OS drivers.
- **Pre-buffering Threshold**: When `duration - position <= 5.0 seconds`, trigger background thread to decode the first 10 seconds of next track into a secondary circular buffer.
- **Loudness Normalization Equation**:
  $$G = 10^{\frac{\text{Target LUFS} (-14.0) - \text{Track LUFS}}{20}}$$
  Gain clamp: $G \in [0.1, 4.0]$ to prevent distortion or silence on malformed tags.
