/**
 * The single module that talks to Tauri. Every `invoke` name, argument key and event name mirrors
 * the frozen Rust command surface in `src-tauri/src/lib.rs` exactly.
 *
 * Outside the Tauri webview (plain `npm run dev` in a browser) read commands resolve to empty
 * results so the UI renders its empty states, and `isTauriEnvironment()` lets callers tell the user
 * that an action needs the desktop app instead of pretending it worked.
 */
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { appDataDir } from '@tauri-apps/api/path';
import { open } from '@tauri-apps/plugin-dialog';
import type {
  AlbumRecord,
  AppConfig,
  ArtistRecord,
  EngineState,
  EngineTrack,
  ParsedLyrics,
  PlaybackProgress,
  ProviderFilter,
  ProviderPlaylist,
  ProviderTrack,
  MediaKeyAction,
  ScanProgress,
  ScanSummary,
  SearchResults,
  TrackRecord,
  TrackSort,
  SpotifyPlaybackState,
} from '../types';

export const isTauriEnvironment = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/**
 * Local artwork paths are absolute filesystem paths; the asset protocol turns them into something
 * an `<img>` can load. Remote URLs and the browser fallback pass through untouched.
 */
export const toAssetUrl = (path: string): string =>
  isTauriEnvironment() ? convertFileSrc(path) : '';

const call = async <T>(
  command: string,
  args: Record<string, unknown>,
  fallback: T,
): Promise<T> => (isTauriEnvironment() ? invoke<T>(command, args) : fallback);

const subscribe = async <T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> => {
  if (!isTauriEnvironment()) return () => {};
  return listen<T>(event, (e) => handler(e.payload));
};

export const EVENTS = {
  progress: 'sonora://playback-progress',
  status: 'sonora://playback-status',
  trackEnded: 'sonora://track-ended',
  scanProgress: 'sonora://scan-progress',
  libraryChanged: 'sonora://library-changed',
  mediaKey: 'sonora://media-key-event',
} as const;

export const tauriBridge = {
  isTauriEnvironment,
  toAssetUrl,

  // ---------------------------------------------------------------------------------------
  // Playback
  // ---------------------------------------------------------------------------------------

  loadTrack(track: EngineTrack, autoPlay = true): Promise<void> {
    return call('playback_load_track', { track, autoPlay }, undefined);
  },

  /**
   * Feeds the gapless pre-roll. Pass `null` at the end of the queue so the engine knows to stop.
   */
  setNextTrack(track: EngineTrack | null): Promise<void> {
    return call('playback_set_next_track', { track }, undefined);
  },

  play(): Promise<void> {
    return call('playback_play', {}, undefined);
  },

  pause(): Promise<void> {
    return call('playback_pause', {}, undefined);
  },

  stop(): Promise<void> {
    return call('playback_stop', {}, undefined);
  },

  seek(positionMs: number): Promise<void> {
    return call('playback_seek', { positionMs }, undefined);
  },

  setVolume(volume: number): Promise<void> {
    return call('playback_set_volume', { volume }, undefined);
  },

  toggleNormalization(enabled: boolean): Promise<boolean> {
    return call('playback_toggle_normalization', { enabled }, enabled);
  },

  getState(): Promise<EngineState | null> {
    return call<EngineState | null>('playback_get_state', {}, null);
  },

  // ---------------------------------------------------------------------------------------
  // Library
  // ---------------------------------------------------------------------------------------

  scanDirectory(dirPath: string): Promise<ScanSummary> {
    return call<ScanSummary>(
      'library_scan_directory',
      { dirPath },
      { scanned: 0, indexed: 0, removed: 0, durationMs: 0 },
    );
  },

  getTracks(options: {
    query?: string;
    provider?: ProviderFilter;
    sort?: TrackSort;
    limit?: number;
    offset?: number;
  } = {}): Promise<TrackRecord[]> {
    const { query, provider, sort, limit, offset } = options;
    return call<TrackRecord[]>(
      'library_get_tracks',
      {
        query: query || null,
        provider: provider && provider !== 'all' ? provider : null,
        sort: sort ?? null,
        limit: limit ?? null,
        offset: offset ?? null,
      },
      [],
    );
  },

  getAlbums(provider: ProviderFilter = 'all', query?: string): Promise<AlbumRecord[]> {
    return call<AlbumRecord[]>(
      'library_get_albums',
      { provider: provider !== 'all' ? provider : null, query: query?.trim() || null },
      [],
    );
  },

  getArtists(provider: ProviderFilter = 'all', query?: string): Promise<ArtistRecord[]> {
    return call<ArtistRecord[]>(
      'library_get_artists',
      { provider: provider !== 'all' ? provider : null, query: query?.trim() || null },
      [],
    );
  },

  countTracks(provider: ProviderFilter = 'all'): Promise<number> {
    return call<number>('library_count_tracks', { provider: provider !== 'all' ? provider : null }, 0);
  },

  getConfig(): Promise<AppConfig> {
    return call<AppConfig>('library_get_config', {}, { musicFolders: [] });
  },

  setMusicFolders(folders: string[]): Promise<void> {
    return call('library_set_music_folders', { folders }, undefined);
  },

  /** An empty string clears the stored Client ID. */
  setSpotifyClientId(clientId: string): Promise<void> {
    return call('library_set_spotify_client_id', { clientId }, undefined);
  },

  // ---------------------------------------------------------------------------------------
  // Lyrics
  // ---------------------------------------------------------------------------------------

  getLyrics(track: TrackRecord): Promise<ParsedLyrics> {
    return call<ParsedLyrics>(
      'lyrics_get_for_track',
      { track },
      { trackId: track.id, lines: [], hasRomanization: false, provider: 'none' },
    );
  },

  // ---------------------------------------------------------------------------------------
  // Spotify
  // ---------------------------------------------------------------------------------------

  spotifyAuthenticate(): Promise<boolean> {
    return call<boolean>('spotify_authenticate', {}, false);
  },

  spotifyStatus(): Promise<boolean> {
    return call<boolean>('spotify_status', {}, false);
  },

  spotifyDisconnect(): Promise<void> {
    return call('spotify_disconnect', {}, undefined);
  },

  spotifySearch(query: string, limit = 25): Promise<SearchResults> {
    return call<SearchResults>('spotify_search', { query, limit }, { tracks: [], playlists: [] });
  },

  spotifyPlaylists(): Promise<ProviderPlaylist[]> {
    return call<ProviderPlaylist[]>('spotify_playlists', {}, []);
  },

  spotifyPlaylistTracks(playlistId: string): Promise<ProviderTrack[]> {
    return call<ProviderTrack[]>('spotify_playlist_tracks', { playlistId }, []);
  },

  spotifyLibrary(limit = 50): Promise<ProviderTrack[]> {
    return call<ProviderTrack[]>('spotify_library', { limit }, []);
  },

  /** Spotify audio is DRM protected, so playback is delegated to the user's own Spotify client. */
  spotifyPlay(uri: string): Promise<void> {
    return call('spotify_play', { uri }, undefined);
  },

  spotifyPlaybackState(): Promise<SpotifyPlaybackState> {
    return call<SpotifyPlaybackState>(
      'spotify_playback_state',
      {},
      { track: null, isPlaying: false, progressMs: 0, durationMs: 0 },
    );
  },

  spotifyResume(): Promise<void> {
    return call('spotify_resume', {}, undefined);
  },

  spotifyPause(): Promise<void> {
    return call('spotify_pause', {}, undefined);
  },

  spotifyNext(): Promise<void> {
    return call('spotify_next', {}, undefined);
  },

  spotifyPrevious(): Promise<void> {
    return call('spotify_previous', {}, undefined);
  },

  spotifySeek(positionMs: number): Promise<void> {
    return call('spotify_seek', { positionMs }, undefined);
  },

  spotifySetVolume(volume: number): Promise<void> {
    return call('spotify_set_volume', { volume }, undefined);
  },

  spotifyResolveStream(title: string, artist: string, durationMs: number): Promise<string> {
    if (!isTauriEnvironment()) return Promise.reject(new Error('Not running in the Sonora app.'));
    return invoke<string>('spotify_resolve_stream', { title, artist, durationMs });
  },

  // ---------------------------------------------------------------------------------------
  // YouTube Music
  // ---------------------------------------------------------------------------------------

  ytmusicSearch(query: string, limit = 25): Promise<SearchResults> {
    return call<SearchResults>('ytmusic_search', { query, limit }, { tracks: [], playlists: [] });
  },

  /** Returns a direct audio URL, which is then handed to the engine as `streamUrl`. */
  ytmusicResolveStream(videoId: string): Promise<string> {
    if (!isTauriEnvironment()) return Promise.reject(new Error('Not running in the Sonora app.'));
    return invoke<string>('ytmusic_resolve_stream', { videoId });
  },

  // ---------------------------------------------------------------------------------------
  // Native dialogs
  // ---------------------------------------------------------------------------------------

  /** Returns `null` in a browser or when the user cancels. */
  async pickMusicFolder(): Promise<string | null> {
    if (!isTauriEnvironment()) return null;
    const selected = await open({ directory: true, multiple: false, title: 'Add music folder' });
    return typeof selected === 'string' ? selected : null;
  },

  /** The JSON file the backend reads config from; the only place a Spotify Client ID can be set. */
  async settingsFilePath(): Promise<string | null> {
    if (!isTauriEnvironment()) return null;
    try {
      return `${await appDataDir()}/settings.json`;
    } catch {
      return null;
    }
  },

  // ---------------------------------------------------------------------------------------
  // Events
  // ---------------------------------------------------------------------------------------

  onPlaybackProgress(handler: (payload: PlaybackProgress) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.progress, handler);
  },

  onPlaybackStatus(handler: (payload: EngineState) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.status, handler);
  },

  onTrackEnded(handler: (payload: { trackId: string }) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.trackEnded, handler);
  },

  onScanProgress(handler: (payload: ScanProgress) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.scanProgress, handler);
  },

  onLibraryChanged(handler: (payload: ScanSummary) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.libraryChanged, handler);
  },

  onMediaKey(handler: (payload: { action: MediaKeyAction }) => void): Promise<UnlistenFn> {
    return subscribe(EVENTS.mediaKey, handler);
  },

  onSpotifyAuthUrl(handler: (url: string) => void): Promise<UnlistenFn> {
    return subscribe('spotify://auth-url', handler);
  },

  async openUrl(url: string): Promise<void> {
    try {
      const { openUrl } = await import('@tauri-apps/plugin-opener');
      await openUrl(url);
    } catch {
      window.open(url, '_blank');
    }
  },
};
