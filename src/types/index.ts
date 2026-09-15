/**
 * Wire types. Each interface mirrors a Rust struct that serializes with
 * `#[serde(rename_all = "camelCase")]`, so the field names here are exactly what crosses IPC.
 */

export type ProviderType = 'local' | 'spotify' | 'ytmusic';

/** Accepted by the `library_get_*` commands; `all` is a filter, not a provider. */
export type ProviderFilter = 'all' | ProviderType;

export type PlaybackStatus = 'playing' | 'paused' | 'stopped';

export type RepeatMode = 'off' | 'track' | 'queue';

export type TrackSort = 'title' | 'artist' | 'album' | 'duration' | 'format' | 'added';

export type LyricsProvider = 'embedded' | 'lrclib' | 'netease' | 'none';

export type LibraryTab = 'tracks' | 'albums' | 'artists';

/** Main-content routes. Lyrics and the queue are overlays, not routes. */
export type AppView = 'library' | 'playlists' | 'settings';

export type RomanizationMode = 'original' | 'romanized' | 'dual';

/**
 * A row from the local SQLite library. Remote provider results are a structural subset of this
 * shape, so one queue type covers local, Spotify and YouTube Music entries.
 */
export interface TrackRecord {
  id: string;
  provider: ProviderType;
  title: string;
  artist: string;
  album: string;
  albumArtist?: string;
  durationMs: number;
  year?: number;
  genre?: string;
  filePath?: string;
  /** Absolute filesystem path for local tracks, http(s) URL for remote ones. */
  artworkUrl?: string;
  bitrate?: number;
  format?: string;
  loudnessLufs?: number;
  trackNumber?: number;
  discNumber?: number;
}

export interface AlbumRecord {
  provider: ProviderType;
  album: string;
  albumArtist?: string;
  year?: number;
  artworkUrl?: string;
  trackCount: number;
  durationMs: number;
}

export interface ArtistRecord {
  provider: ProviderType;
  artist: string;
  albumCount: number;
  trackCount: number;
}

export interface ScanSummary {
  scanned: number;
  indexed: number;
  removed: number;
  durationMs: number;
}

export interface AppConfig {
  spotifyClientId?: string;
  musicFolders: string[];
}

export interface EngineTrack {
  id: string;
  title: string;
  artist: string;
  /** Null for streamed (YouTube Music) tracks. */
  filePath?: string | null;
  streamUrl?: string | null;
  durationMs: number;
  loudnessLufs?: number;
}

export interface ProviderTrack {
  id: string;
  provider: ProviderType;
  title: string;
  artist: string;
  album: string;
  durationMs: number;
  artworkUrl?: string;
}

export interface ProviderPlaylist {
  id: string;
  title: string;
  description?: string;
  artworkUrl?: string;
  trackCount: number;
  provider: ProviderType;
}

export interface SearchResults {
  tracks: ProviderTrack[];
  playlists: ProviderPlaylist[];
}

export interface LyricLine {
  timeMs: number;
  text: string;
  romanizedText?: string;
}

export interface ParsedLyrics {
  trackId: string;
  lines: LyricLine[];
  hasRomanization: boolean;
  provider: LyricsProvider;
}

export interface EngineState {
  currentTrack?: EngineTrack;
  nextTrack?: EngineTrack;
  status: PlaybackStatus;
  positionMs: number;
  durationMs: number;
  volume: number;
  isNormalizing: boolean;
  isGapless: boolean;
}

export interface PlaybackProgress {
  positionMs: number;
  durationMs: number;
}

export interface ScanProgress {
  scanned: number;
  total: number;
  currentFile: string;
}

export interface SpotifyPlaybackState {
  track: ProviderTrack | null;
  isPlaying: boolean;
  progressMs: number;
  durationMs: number;
  volumePercent?: number;
  deviceName?: string;
}

export type MediaKeyAction = 'play_pause' | 'next' | 'prev';

export type ThemePreset =
  | 'obsidian'
  | 'midnight'
  | 'cybervolt'
  | 'rose-pine'
  | 'nord'
  | 'amoled'
  | 'dynamic';
