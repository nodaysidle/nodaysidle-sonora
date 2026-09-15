import { create } from 'zustand';
import { isTauriEnvironment, tauriBridge } from '../services/tauriBridge';
import { errorMessage as message } from '../services/format';
import type {
  AlbumRecord,
  AppConfig,
  ArtistRecord,
  LibraryTab,
  ProviderFilter,
  ProviderPlaylist,
  ProviderTrack,
  ScanProgress,
  ScanSummary,
  TrackRecord,
  TrackSort,
} from '../types';

type Unsubscribe = () => void;

const EMPTY_PROGRESS: ScanProgress = { scanned: 0, total: 0, currentFile: '' };

/** Client-side ordering for provider results, which arrive unsorted from the remote APIs. */
const sortedCopy = (tracks: TrackRecord[], sort: TrackSort): TrackRecord[] => {
  const rows = [...tracks];
  switch (sort) {
    case 'title':
      return rows.sort((a, b) => a.title.localeCompare(b.title));
    case 'artist':
      return rows.sort((a, b) => a.artist.localeCompare(b.artist));
    case 'album':
      return rows.sort((a, b) => a.album.localeCompare(b.album));
    case 'duration':
      return rows.sort((a, b) => a.durationMs - b.durationMs);
    case 'format':
    case 'added':
      return rows;
    default:
      return rows;
  }
};

const remoteTrackRecord = (track: ProviderTrack): TrackRecord => ({
  ...track,
  filePath: undefined,
});

const remoteTracksFor = async (
  provider: ProviderFilter,
  query: string,
  spotifyConnected: boolean,
): Promise<{ tracks: TrackRecord[]; errors: unknown[] }> => {
  const trimmedQuery = query.trim();
  const jobs: Promise<ProviderTrack[]>[] = [];

  if (provider === 'spotify' && spotifyConnected) {
    jobs.push(
      trimmedQuery
        ? tauriBridge.spotifySearch(trimmedQuery, 50).then((results) => results.tracks)
        : tauriBridge.spotifyLibrary(200),
    );
  }
  if (provider === 'ytmusic') {
    jobs.push(tauriBridge.ytmusicSearch(trimmedQuery || 'Trending Hits', 50).then((results) => results.tracks));
  }
  if (provider === 'all') {
    if (spotifyConnected && trimmedQuery) {
      jobs.push(tauriBridge.spotifySearch(trimmedQuery, 50).then((results) => results.tracks));
    } else if (spotifyConnected) {
      jobs.push(tauriBridge.spotifyLibrary(200));
    }
    if (trimmedQuery) {
      jobs.push(tauriBridge.ytmusicSearch(trimmedQuery, 50).then((results) => results.tracks));
    }
  }

  const results = await Promise.allSettled(jobs);
  return {
    tracks: results.flatMap((result) => (result.status === 'fulfilled' ? result.value.map(remoteTrackRecord) : [])),
    errors: results.flatMap((result) => (result.status === 'rejected' ? [result.reason] : [])),
  };
};

const aggregateAlbums = (tracks: TrackRecord[]): AlbumRecord[] => {
  const albums = new Map<string, AlbumRecord>();
  for (const track of tracks) {
    const key = `${track.provider}\0${track.albumArtist ?? track.artist}\0${track.album}`;
    const current = albums.get(key);
    if (current) {
      current.trackCount += 1;
      current.durationMs += track.durationMs;
      continue;
    }
    albums.set(key, {
      provider: track.provider,
      album: track.album,
      albumArtist: track.albumArtist ?? track.artist,
      year: track.year,
      artworkUrl: track.artworkUrl,
      trackCount: 1,
      durationMs: track.durationMs,
    });
  }
  return [...albums.values()].sort((a, b) => a.album.localeCompare(b.album));
};

const aggregateArtists = (tracks: TrackRecord[]): ArtistRecord[] => {
  const artists = new Map<string, ArtistRecord & { albums: Set<string> }>();
  for (const track of tracks) {
    const key = `${track.provider}\0${track.artist}`;
    const current = artists.get(key);
    if (current) {
      current.trackCount += 1;
      current.albums.add(track.album);
      current.albumCount = current.albums.size;
      continue;
    }
    artists.set(key, {
      provider: track.provider,
      artist: track.artist,
      albumCount: 1,
      trackCount: 1,
      albums: new Set([track.album]),
    });
  }
  return [...artists.values()]
    .map(({ albums: _albums, ...artist }) => artist)
    .sort((a, b) => a.artist.localeCompare(b.artist));
};

interface LibraryStore {
  tracks: TrackRecord[];
  albums: AlbumRecord[];
  artists: ArtistRecord[];
  playlists: ProviderPlaylist[];
  playlistTracks: ProviderTrack[];
  selectedPlaylist: ProviderPlaylist | null;

  activeTab: LibraryTab;
  activeProviderFilter: ProviderFilter;
  searchQuery: string;
  sort: TrackSort;

  isLoading: boolean;
  error: string | null;

  isScanning: boolean;
  scanProgress: ScanProgress;
  scanSummary: ScanSummary | null;

  config: AppConfig;
  spotifyConnected: boolean;
  isSpotifyConnecting: boolean;

  refresh: () => Promise<void>;
  setTab: (tab: LibraryTab) => void;
  setProviderFilter: (filter: ProviderFilter) => void;
  setSearchQuery: (query: string) => void;
  setSort: (sort: TrackSort) => void;
  clearError: () => void;

  scanFolder: (path: string) => Promise<void>;
  pickAndScan: () => Promise<void>;

  loadConfig: () => Promise<void>;
  saveMusicFolders: (folders: string[]) => Promise<void>;
  saveSpotifyClientId: (clientId: string) => Promise<void>;

  refreshSpotifyStatus: () => Promise<void>;
  connectSpotify: () => Promise<void>;
  disconnectSpotify: () => Promise<void>;
  loadPlaylists: () => Promise<void>;
  openPlaylist: (playlist: ProviderPlaylist | null) => Promise<void>;
}

let searchTimer: ReturnType<typeof setTimeout> | null = null;
let refreshSequence = 0;

export const useLibraryStore = create<LibraryStore>((set, get) => ({
  tracks: [],
  albums: [],
  artists: [],
  playlists: [],
  playlistTracks: [],
  selectedPlaylist: null,

  activeTab: 'tracks',
  activeProviderFilter: 'all',
  searchQuery: '',
  sort: 'title',

  isLoading: false,
  error: null,

  isScanning: false,
  scanProgress: EMPTY_PROGRESS,
  scanSummary: null,

  config: { musicFolders: [] },
  spotifyConnected: false,
  isSpotifyConnecting: false,

  refresh: async () => {
    const { activeTab, activeProviderFilter, searchQuery, sort } = get();
    const sequence = ++refreshSequence;
    set({ isLoading: true, error: null });
    try {
      if (activeTab === 'albums') {
        const localPromise =
          activeProviderFilter === 'local' || activeProviderFilter === 'all'
            ? tauriBridge.getAlbums(activeProviderFilter, searchQuery)
            : Promise.resolve([] as AlbumRecord[]);
        const remotePromise =
          activeProviderFilter === 'local'
            ? Promise.resolve({ tracks: [], errors: [] } as { tracks: TrackRecord[]; errors: unknown[] })
            : remoteTracksFor(activeProviderFilter, searchQuery, get().spotifyConnected);
        const [local, remote] = await Promise.allSettled([localPromise, remotePromise]);
        if (sequence !== refreshSequence) return;
        const localAlbums = local.status === 'fulfilled' ? local.value : [];
        const remoteAlbums = remote.status === 'fulfilled' ? aggregateAlbums(remote.value.tracks) : [];
        const errors = [
          ...(local.status === 'rejected' ? [local.reason] : []),
          ...(remote.status === 'fulfilled' ? remote.value.errors : remote.status === 'rejected' ? [remote.reason] : []),
        ];
        set({
          albums: [...localAlbums, ...remoteAlbums],
          error: errors.length > 0 ? errors.map(message).join(' ') : null,
        });
        return;
      }
      if (activeTab === 'artists') {
        const localPromise =
          activeProviderFilter === 'local' || activeProviderFilter === 'all'
            ? tauriBridge.getArtists(activeProviderFilter, searchQuery)
            : Promise.resolve([] as ArtistRecord[]);
        const remotePromise =
          activeProviderFilter === 'local'
            ? Promise.resolve({ tracks: [], errors: [] } as { tracks: TrackRecord[]; errors: unknown[] })
            : remoteTracksFor(activeProviderFilter, searchQuery, get().spotifyConnected);
        const [local, remote] = await Promise.allSettled([localPromise, remotePromise]);
        if (sequence !== refreshSequence) return;
        const localArtists = local.status === 'fulfilled' ? local.value : [];
        const remoteArtists = remote.status === 'fulfilled' ? aggregateArtists(remote.value.tracks) : [];
        const errors = [
          ...(local.status === 'rejected' ? [local.reason] : []),
          ...(remote.status === 'fulfilled' ? remote.value.errors : remote.status === 'rejected' ? [remote.reason] : []),
        ];
        set({
          artists: [...localArtists, ...remoteArtists],
          error: errors.length > 0 ? errors.map(message).join(' ') : null,
        });
        return;
      }

      const localPromise =
        activeProviderFilter === 'spotify' || activeProviderFilter === 'ytmusic'
          ? Promise.resolve([] as TrackRecord[])
          : tauriBridge.getTracks({ query: searchQuery, provider: activeProviderFilter, sort });
      const remotePromise =
        activeProviderFilter === 'local'
          ? Promise.resolve({ tracks: [], errors: [] } as { tracks: TrackRecord[]; errors: unknown[] })
          : remoteTracksFor(activeProviderFilter, searchQuery, get().spotifyConnected);
      const [local, remote] = await Promise.allSettled([localPromise, remotePromise]);
      if (sequence !== refreshSequence) return;
      const localTracks = local.status === 'fulfilled' ? local.value : [];
      const remoteResult = remote.status === 'fulfilled' ? remote.value : { tracks: [], errors: [remote.reason] };
      const errors = [
        ...(local.status === 'rejected' ? [local.reason] : []),
        ...remoteResult.errors,
      ];
      set({
        tracks: sortedCopy([...localTracks, ...remoteResult.tracks], sort),
        error: errors.length > 0 ? errors.map(message).join(' ') : null,
      });
    } catch (error) {
      if (sequence === refreshSequence) set({ error: message(error) });
    } finally {
      if (sequence === refreshSequence) set({ isLoading: false });
    }
  },

  setTab: (activeTab) => {
    set({ activeTab });
    void get().refresh();
  },

  setProviderFilter: (activeProviderFilter) => {
    set({ activeProviderFilter });
    void get().refresh();
  },

  /** Debounced: the local library answers through FTS5, the remote ones through an HTTP API. */
  setSearchQuery: (searchQuery) => {
    set({ searchQuery });
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void get().refresh(), 250);
  },

  setSort: (sort) => {
    set({ sort });
    void get().refresh();
  },

  clearError: () => set({ error: null }),

  scanFolder: async (path) => {
    if (!isTauriEnvironment()) {
      set({ error: 'Scanning folders needs the Sonora desktop app.' });
      return;
    }
    set({ isScanning: true, error: null, scanSummary: null, scanProgress: EMPTY_PROGRESS });

    // Subscribe before the command starts so no progress event is missed.
    let unbind: Unsubscribe = () => {};
    try {
      unbind = await tauriBridge.onScanProgress((progress) => set({ scanProgress: progress }));
      const summary = await tauriBridge.scanDirectory(path);
      set({ scanSummary: summary });
      await get().loadConfig();
      await get().refresh();
    } catch (error) {
      set({ error: message(error) });
    } finally {
      unbind();
      set({ isScanning: false });
    }
  },

  pickAndScan: async () => {
    if (!isTauriEnvironment()) {
      set({ error: 'Scanning folders needs the Sonora desktop app.' });
      return;
    }
    const folder = await tauriBridge.pickMusicFolder();
    // A cancelled dialog is a deliberate no-op, not an error.
    if (!folder) return;
    await get().scanFolder(folder);
  },

  loadConfig: async () => {
    try {
      set({ config: await tauriBridge.getConfig() });
    } catch (error) {
      set({ error: message(error) });
    }
  },

  saveMusicFolders: async (folders) => {
    try {
      await tauriBridge.setMusicFolders(folders);
      set({ config: { ...get().config, musicFolders: folders } });
    } catch (error) {
      set({ error: message(error) });
    }
  },

  saveSpotifyClientId: async (clientId) => {
    const trimmed = clientId.trim();
    try {
      await tauriBridge.setSpotifyClientId(trimmed);
      set({
        config: { ...get().config, spotifyClientId: trimmed === '' ? undefined : trimmed },
      });
    } catch (error) {
      set({ error: message(error) });
    }
  },

  refreshSpotifyStatus: async () => {
    try {
      const connected = await tauriBridge.spotifyStatus();
      set((state) => ({
        spotifyConnected: connected,
        error:
          state.error === 'Not connected to Spotify.' || state.error?.startsWith('Spotify ')
            ? null
            : state.error,
      }));
    } catch (error) {
      set({ error: message(error) });
    }
  },

  connectSpotify: async () => {
    if (!isTauriEnvironment()) {
      set({ error: 'Connecting to Spotify needs the Sonora desktop app.' });
      return;
    }
    set({ error: null, isSpotifyConnecting: true });
    try {
      const connected = await tauriBridge.spotifyAuthenticate();
      set({ spotifyConnected: connected });
      if (connected) await get().loadPlaylists();
    } catch (error) {
      set({ error: message(error), spotifyConnected: false });
    } finally {
      set({ isSpotifyConnecting: false });
    }
  },

  disconnectSpotify: async () => {
    try {
      await tauriBridge.spotifyDisconnect();
      set({ spotifyConnected: false, playlists: [], playlistTracks: [], selectedPlaylist: null });
      await get().refresh();
    } catch (error) {
      set({ error: message(error) });
    }
  },

  loadPlaylists: async () => {
    set({ isLoading: true, error: null });
    try {
      set({ playlists: await tauriBridge.spotifyPlaylists() });
    } catch (error) {
      set({ playlists: [], error: message(error) });
    } finally {
      set({ isLoading: false });
    }
  },

  openPlaylist: async (playlist) => {
    if (!playlist) {
      set({ selectedPlaylist: null, playlistTracks: [] });
      return;
    }
    set({ selectedPlaylist: playlist, playlistTracks: [], isLoading: true, error: null });
    try {
      set({ playlistTracks: await tauriBridge.spotifyPlaylistTracks(playlist.id) });
    } catch (error) {
      set({ error: message(error) });
    } finally {
      set({ isLoading: false });
    }
  },
}));
