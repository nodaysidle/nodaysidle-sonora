# ARD — Architecture Reference Document: Sonora

## 1. Architectural Strategy & Technology Stack

| Domain | Technology | Rationale |
|---|---|---|
| **App Framework** | **Tauri v2** | Native OS windowing, small bundle (~15MB), sub-90MB RAM, zero Chromium bloat, seamless Rust <-> JS IPC. |
| **Backend Core** | **Rust (2021 edition)** | Memory safety, zero-cost abstractions, real-time deterministic audio decoding, multi-threaded library scanning. |
| **Audio Pipeline** | **Symphonia + CPAL + Rubato** | Low-latency cross-platform audio decoding (FLAC, MP3, AAC, OGG, WAV), sample rate conversion, and hardware output stream. |
| **Loudness Normalization** | **EBU R128 (`ebur128` crate)** | Studio broadcast standard ITU-R BS.1770-4. Real-time gain adjustment to target -14 LUFS without dynamic range compression. |
| **Metadata & Tags** | **Lofty crate** | Blazing fast ID3v2, Vorbis Comments, MP4/AAC tags, and embedded artwork extraction. |
| **Local Database** | **SQLite + rusqlite (FTS5)** | Embedded SQL database for track indexing, playlists, history, and full-text search. |
| **Frontend Framework** | **React 19 + TypeScript + Vite** | Modern reactive UI, fast HMR development, typed contracts with Rust backend. |
| **Styling & Effects** | **Tailwind CSS + CSS Backdrop Filter** | Liquid glassmorphism, native window transparency, responsive flexbox/grid layout. |
| **State Management** | **Zustand** | Minimalist atomic store with bi-directional synchronization to Rust Tauri state via events. |
| **Icons & Design System** | **Lucide Icons** | Clean, minimalist, modern native media iconography. |

---

## 2. High-Level Architecture Diagram

```
+-------------------------------------------------------------------------+
|                        Sonora Desktop UI (Webview)                      |
|                                                                         |
|  +-------------------+  +---------------------+  +-------------------+  |
|  |   Sidebar Nav     |  |    Main Content     |  |   Synced Lyrics   |  |
|  | - Library         |  | - Albums / Tracks   |  | - Auto-scroll     |  |
|  | - Playlists       |  | - Unified Search    |  | - Romaji / Pinyin |  |
|  | - Source Switcher |  | - Queue Management  |  | - Active Line Glow|  |
|  +-------------------+  +---------------------+  +-------------------+  |
|                                                                         |
|  +-------------------------------------------------------------------+  |
|  |        Player Bar: Transport Controls, Timeline, Volume, EQ       |  |
|  +-------------------------------------------------------------------+  |
|                                 |                                       |
|                  Zustand Store  |  Tauri IPC (Commands & Events)        |
+---------------------------------v---------------------------------------+
                                  |
+---------------------------------v---------------------------------------+
|                         Rust Native Core (Tauri)                        |
|                                                                         |
|  +--------------------+  +----------------------+  +-----------------+  |
|  |  Audio Engine      |  |  Music Provider      |  |  Library Index  |  |
|  |  - Symphonia       |  |  Trait Coordinator   |  |  - Lofty tagger |  |
|  |  - CPAL output     |  |  + Local Files       |  |  - notify watch |  |
|  |  - Gapless buffer  |  |  + Spotify API       |  |  - SQLite (FTS5)|  |
|  |  - EBU R128 filter |  |  + YouTube InnerTube |  |                 |  |
|  +--------------------+  +----------------------+  +-----------------+  |
|                                                                         |
|  +--------------------+  +----------------------+  +-----------------+  |
|  |  Lyrics & Romaji   |  |  Platform Native     |  |  Secure Keyring |  |
|  |  - LRCLIB client   |  |  - MPRIS (Linux)     |  |  - Spotify Token|  |
|  |  - Kana/Hangul/    |  |  - SMTC (Windows)    |  |  - Credential   |  |
|  |    Pinyin engine   |  |  - NowPlaying (macOS)|  |    storage      |  |
|  +--------------------+  +----------------------+  +-----------------+  |
+-------------------------------------------------------------------------+
```

---

## 3. Detailed Component Architecture

### 3.1 Gapless Audio Engine & Normalization
```
                 [ Track N (Playing) ] ----> [ Decoder A ] ---\
                                                               +--> [ Resampler / R128 Gain ] --> [ CPAL Device ]
  [ Pre-load ]-> [ Track N+1 (Queued) ] ---> [ Decoder B ] ---/
```
- **Dual Decoder Ring**: When Track $N$ reaches within 5 seconds of track completion, Decoder $B$ opens Track $N+1$, decodes the header, and fills a pre-roll ring buffer.
- **Boundary Splicing**: At the exact sample where Track $N$ ends, the audio output mixer switches without tearing or clock starvation, achieving true sample-accurate gapless playback.
- **Loudness Normalization**:
  - Local tracks: Pre-calculated ReplayGain / BS.1770 LUFS stored in SQLite.
  - Streaming tracks: Real-time windowed loudness estimation or stream metadata.
  - Gain calculation: $Gain_{dB} = Target_{LUFS} (-14.0) - Track_{LUFS}$. Applied as a smooth linear float multiplication to avoid clipping.

### 3.2 Unified Music Provider Interface
Rust defines a polymorphic trait `MusicProvider`:
```rust
#[async_trait]
pub trait MusicProvider: Send + Sync {
    fn provider_id(&self) -> ProviderId;
    async fn search(&self, query: &str, limit: usize) -> Result<SearchResults>;
    async fn get_track_stream(&self, track_id: &str) -> Result<TrackAudioSource>;
    async fn get_album(&self, album_id: &str) -> Result<Album>;
    async fn get_user_playlists(&self) -> Result<Vec<Playlist>>;
}
```
All providers emit uniform `Track`, `Album`, `Artist`, and `Playlist` structs with canonical URI schemes:
- `local://path/to/song.flac`
- `spotify://track/4cOdK2wGLETKBW3PvgPWqT`
- `ytmusic://track/dQw4w9WgXcQ`

### 3.3 Lyrics & Romanization Pipeline
1. Track starts playing -> Client requests lyrics by `(title, artist, album, duration)`.
2. Provider hierarchy:
   - Check local file tags (ID3v2 `USLT` / `SYLT` or Vorbis `LYRICS`).
   - Query LRCLIB API (`https://lrclib.net/api/get`).
   - Query NetEase Cloud Music fallback API.
3. Romanization processing:
   - Evaluates script detection on each line (Unicode blocks for Hiragana `\u3040-\u309F`, Katakana `\u30A0-\u30FF`, CJK Ideographs `\u4E00-\u9FAF`, Hangul `\uAC00-\uD7AF`, Cyrillic `\u0400-\u04FF`).
   - Transliterates to Hepburn Romaji, Revised Romanization of Korean, Pinyin, or Latin Cyrillic.
   - Preserves timestamps for synchronization.

### 3.4 Storage & Database Architecture
- Embedded SQLite database located in `sonora.db`.
- SQLite FTS5 table `tracks_fts` enables sub-millisecond typo-tolerant searches across 100,000+ tracks.
- Wal-mode (`PRAGMA journal_mode=WAL;`) enabled for concurrent reads during background library indexing.

---

## 4. Key Architectural Decisions (ADR)

- **ADR-001: Tauri v2 over Electron**: Electron consumes 400MB-1GB of memory and exhibits non-native window borders and latency. Tauri v2 gives native system control, direct low-latency Rust audio decoding, and a lightweight footprint.
- **ADR-002: Rust Audio Pipeline over Web Audio API**: Web Audio API inside WebViews suffers from OS thread throttling, background suspension, and lacks sample-accurate gapless multi-file prebuffering. Running decoding in a dedicated high-priority Rust audio thread guarantees zero stutter.
- **ADR-003: OAuth 2.0 PKCE for Spotify**: Avoids shipping embedded client secrets, allows user-configured Spotify developer app client IDs, and ensures compliant, secure authentication.
- **ADR-004: In-Memory Romanization Cache**: Romanized lyrics are computed once per song and cached locally alongside the LRC text, eliminating repeated CPU overhead.
