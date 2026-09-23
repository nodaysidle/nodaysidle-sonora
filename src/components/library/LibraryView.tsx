import React, { useEffect } from 'react';
import {
  AlertCircle,
  FolderSync,
  HardDrive,
  Play,
  PlaySquare,
  Radio,
  Search,
  X,
} from 'lucide-react';
import { useLibraryStore } from '../../stores/libraryStore';
import { usePlayerStore } from '../../stores/playerStore';
import { tauriBridge } from '../../services/tauriBridge';
import {
  artworkSrc,
  errorMessage,
  formatDuration,
  formatLongDuration,
  pluralize,
} from '../../services/format';
import type {
  AlbumRecord,
  ArtistRecord,
  ProviderTrack,
  ProviderType,
  TrackRecord,
  TrackSort,
} from '../../types';

const PROVIDER_BADGES: Record<ProviderType, React.ReactNode> = {
  local: <HardDrive className="w-3.5 h-3.5 text-amber-400" />,
  spotify: <Radio className="w-3.5 h-3.5 text-emerald-400" />,
  ytmusic: <PlaySquare className="w-3.5 h-3.5 text-red-400" />,
};

const remoteTrackRecord = (track: ProviderTrack): TrackRecord => ({ ...track, filePath: undefined });

interface SortHeaderProps {
  label: string;
  value: TrackSort;
  activeSort: TrackSort;
  onSort: (sort: TrackSort) => void;
  className?: string;
}

const SortHeader: React.FC<SortHeaderProps> = ({ label, value, activeSort, onSort, className }) => (
  <th
    className={`py-2.5 px-3 ${className ?? ''}`}
    aria-sort={activeSort === value ? 'ascending' : 'none'}
  >
    <button
      onClick={() => onSort(value)}
      className={`uppercase tracking-wider transition-colors hover:text-zinc-200 ${
        activeSort === value ? 'accent-text' : ''
      }`}
    >
      {label}
    </button>
  </th>
);

const EmptyState: React.FC<{
  title: string;
  body: string;
  action?: { label: string; onClick: () => void };
  secondary?: { label: string; onClick: () => void };
}> = ({ title, body, action, secondary }) => (
  <div className="flex-1 flex flex-col items-center justify-center text-center px-6">
    <div className="max-w-sm space-y-3">
      <h3 className="text-sm font-semibold text-zinc-200">{title}</h3>
      <p className="text-xs text-zinc-500 leading-relaxed">{body}</p>
      {action && (
        <button
          onClick={action.onClick}
          className="mt-2 inline-flex items-center space-x-2 px-3.5 py-2 rounded-lg text-xs font-medium accent-soft accent-text accent-border border"
        >
          <FolderSync className="w-3.5 h-3.5" />
          <span>{action.label}</span>
        </button>
      )}
      {secondary && (
        <button onClick={secondary.onClick} className="block mx-auto text-[11px] text-zinc-400 hover:text-zinc-200 underline">
          {secondary.label}
        </button>
      )}
    </div>
  </div>
);

export const LibraryView: React.FC = () => {
  const {
    tracks,
    albums,
    artists,
    playlists,
    activeTab,
    activeProviderFilter,
    searchQuery,
    sort,
    isLoading,
    error,
    isScanning,
    scanProgress,
    scanSummary,
    config,
    spotifyConnected,
    setTab,
    setSearchQuery,
    setSort,
    refresh,
    loadPlaylists,
    clearError,
    pickAndScan,
  } = useLibraryStore();
  const playFromList = usePlayerStore((s) => s.playFromList);
  const currentTrack = usePlayerStore((s) => s.currentTrack);
  const setNotice = usePlayerStore((s) => s.setNotice);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (spotifyConnected && searchQuery.trim() && playlists.length === 0) void loadPlaylists();
  }, [loadPlaylists, playlists.length, searchQuery, spotifyConnected]);

  const addToPlaylist = async (track: TrackRecord, playlistId: string) => {
    if (!playlistId) return;
    const playlist = playlists.find((item) => item.id === playlistId);
    try {
      await tauriBridge.spotifyAddToPlaylist(playlistId, track.id);
      setNotice(`Added “${track.title}” to ${playlist?.title ?? 'playlist'}.`);
      useLibraryStore.setState((state) => ({
        playlists: state.playlists.map((item) =>
          item.id === playlistId ? { ...item, trackCount: item.trackCount + 1 } : item,
        ),
      }));
    } catch (caught) {
      setNotice(errorMessage(caught));
    }
  };

  const playAlbum = async (album: AlbumRecord) => {
    try {
      const rows =
        album.provider === 'local'
          ? await tauriBridge.getTracks({ query: album.album, provider: 'local', limit: 500 })
          : album.provider === 'spotify'
            ? (await tauriBridge.spotifySearch(`${album.album} ${album.albumArtist ?? ''}`, 50)).tracks.map(remoteTrackRecord)
            : (await tauriBridge.ytmusicSearch(`${album.album} ${album.albumArtist ?? ''}`, 50)).tracks.map(remoteTrackRecord);
      const albumTracks = rows.filter(
        (row) =>
          row.album === album.album &&
          (!album.albumArtist || (row.albumArtist ?? row.artist) === album.albumArtist),
      );
      if (albumTracks.length === 0) {
        setNotice(`No playable tracks found for “${album.album}”.`);
        return;
      }
      playFromList(albumTracks, 0);
    } catch (caught) {
      setNotice(errorMessage(caught));
    }
  };

  const playArtist = async (artist: ArtistRecord) => {
    try {
      const rows =
        artist.provider === 'local'
          ? await tauriBridge.getTracks({ query: artist.artist, provider: 'local', limit: 500 })
          : artist.provider === 'spotify'
            ? (await tauriBridge.spotifySearch(artist.artist, 50)).tracks.map(remoteTrackRecord)
            : (await tauriBridge.ytmusicSearch(artist.artist, 50)).tracks.map(remoteTrackRecord);
      const artistTracks = rows.filter((row) => row.artist === artist.artist);
      if (artistTracks.length === 0) {
        setNotice(`No playable tracks found for “${artist.artist}”.`);
        return;
      }
      playFromList(artistTracks, 0);
    } catch (caught) {
      setNotice(errorMessage(caught));
    }
  };

  const searchPlaceholder =
    activeProviderFilter === 'spotify'
      ? 'Search your Spotify library…'
      : activeProviderFilter === 'ytmusic'
        ? 'Search YouTube Music…'
        : 'Search titles, artists, albums…';

  const renderTracks = () => {
    const showPlaylistAction = Boolean(searchQuery.trim()) && spotifyConnected;
    if (tracks.length === 0) {
      if (searchQuery) {
        return <EmptyState title="No matches" body={`Nothing in this source matches “${searchQuery}”.`} />;
      }
      if (activeProviderFilter === 'spotify') {
        return spotifyConnected ? (
          <EmptyState
            title="Your Spotify library is empty"
            body="Sonora asked Spotify for your saved tracks and top tracks; both came back empty. Search above to find something to play."
          />
        ) : (
          <EmptyState
            title="Spotify is not connected"
            body="Add your Spotify app's Client ID in Settings, then press Connect. Spotify audio plays natively inside Sonora."
          />
        );
      }
      if (activeProviderFilter === 'ytmusic') {
        return (
          <EmptyState
            title="Search YouTube Music"
            body="YouTube Music has no library listing — search above to pull tracks in. Playback resolves a direct audio stream."
          />
        );
      }
      return (
        <EmptyState
          title="Your library is empty"
          body={
            config.musicFolders.length > 0
              ? `Sonora has indexed nothing yet from ${config.musicFolders.join(', ')}. Rescan if you have added files since.`
              : 'Point Sonora at a folder of audio files and it will index the metadata, artwork and loudness once, in the background.'
          }
          action={{
            label: isScanning
              ? `Scanning… ${scanProgress.scanned}${scanProgress.total ? ` / ${scanProgress.total}` : ''}`
              : 'Scan Music Folder',
            onClick: () => void pickAndScan(),
          }}
        />
      );
    }

    return (
      <div className="flex-1 overflow-y-auto">
        <table className="w-full text-left border-collapse">
          <thead className="sticky top-0 bg-zinc-950/80 backdrop-blur-sm z-10">
            <tr className="border-b border-white/5 text-[11px] font-semibold text-zinc-500">
              <SortHeader label="#" value="added" activeSort={sort} onSort={setSort} className="w-10" />
              <SortHeader label="Title" value="title" activeSort={sort} onSort={setSort} />
              <SortHeader label="Artist" value="artist" activeSort={sort} onSort={setSort} />
              <SortHeader label="Album" value="album" activeSort={sort} onSort={setSort} />
              <SortHeader label="Source" value="format" activeSort={sort} onSort={setSort} className="w-24" />
              <SortHeader
                label="Time"
                value="duration"
                activeSort={sort}
                onSort={setSort}
                className="w-20 text-right"
              />
              {showPlaylistAction && <th className="w-44 py-2.5 px-3 uppercase tracking-wider">Playlist</th>}
            </tr>
          </thead>
          <tbody className="divide-y divide-white/5 text-xs">
            {tracks.map((track, index) => {
              const isCurrent = currentTrack?.id === track.id;
              return (
                <tr
                  key={`${track.id}-${index}`}
                  onClick={() => playFromList(tracks, index)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      playFromList(tracks, index);
                    }
                  }}
                  tabIndex={0}
                  role="button"
                  aria-label={`Play ${track.title} by ${track.artist}`}
                  className={`group cursor-pointer transition-colors ${
                    isCurrent ? 'accent-soft accent-text' : 'hover:bg-white/5 text-zinc-300'
                  }`}
                >
                  <td className="py-3 px-3 font-mono text-zinc-500 text-[11px]">
                    <span className="group-hover:hidden">
                      {isCurrent ? <Play className="w-3 h-3 accent-text" /> : index + 1}
                    </span>
                    <Play className="w-3.5 h-3.5 hidden group-hover:inline accent-text" />
                  </td>
                  <td className="py-3 px-3 font-medium text-white truncate max-w-[16rem]">
                    {track.title}
                  </td>
                  <td className="py-3 px-3 text-zinc-400 truncate max-w-[12rem]">{track.artist}</td>
                  <td className="py-3 px-3 text-zinc-500 truncate max-w-[12rem]">{track.album}</td>
                  <td className="py-3 px-3">
                    <div className="flex items-center space-x-1.5">
                      {PROVIDER_BADGES[track.provider]}
                      <span className="text-[10px] uppercase font-mono text-zinc-500">
                        {track.format ?? track.provider}
                      </span>
                    </div>
                  </td>
                  <td className="py-3 px-3 text-right font-mono text-zinc-400 text-[11px]">
                    {formatDuration(track.durationMs)}
                  </td>
                  {showPlaylistAction && (
                    <td className="py-2 px-3" onClick={(event) => event.stopPropagation()}>
                      {track.provider === 'spotify' ? (
                        <select
                          defaultValue=""
                          aria-label={`Add ${track.title} to playlist`}
                          className="w-full rounded-md bg-zinc-900 border border-white/10 px-2 py-1 text-[11px] text-zinc-300 focus:outline-none accent-focus"
                          onKeyDown={(event) => event.stopPropagation()}
                          onChange={(event) => {
                            const playlistId = event.currentTarget.value;
                            event.currentTarget.value = '';
                            void addToPlaylist(track, playlistId);
                          }}
                        >
                          <option value="">Add to playlist…</option>
                          {playlists.map((playlist) => (
                            <option key={playlist.id} value={playlist.id}>
                              {playlist.title}
                            </option>
                          ))}
                        </select>
                      ) : (
                        <span className="text-zinc-700">—</span>
                      )}
                    </td>
                  )}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    );
  };

  const renderAlbums = () => {
    if (albums.length === 0) {
      return (
        <EmptyState
          title="No albums indexed"
          body={
            activeProviderFilter === 'local'
              ? 'Scan a music folder and every album with embedded metadata shows up here.'
              : 'Albums are built from the selected source. Search or connect a provider to load remote results.'
          }
          action={{ label: 'Scan Music Folder', onClick: () => void pickAndScan() }}
        />
      );
    }

    return (
      <div className="flex-1 overflow-y-auto">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-5 pb-4">
          {albums.map((album) => {
            const art = artworkSrc(album.artworkUrl);
            return (
              <button
                key={`${album.provider}-${album.albumArtist ?? ''}-${album.album}`}
                onClick={() => void playAlbum(album)}
                className="group text-left"
                title={`Play ${album.album}`}
              >
                <div className="relative aspect-square rounded-xl overflow-hidden bg-zinc-800/80 border border-white/10">
                  {art ? (
                    <img src={art} alt={album.album} className="w-full h-full object-cover" />
                  ) : (
                    <div className="w-full h-full flex items-center justify-center text-[10px] font-mono text-zinc-600">
                      NO ARTWORK
                    </div>
                  )}
                  <div className="absolute inset-0 bg-black/50 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
                    <span className="w-10 h-10 rounded-full accent-bg text-black flex items-center justify-center accent-shadow">
                      <Play className="w-4 h-4 fill-black ml-0.5" />
                    </span>
                  </div>
                </div>
                <p className="mt-2 text-xs font-medium text-zinc-100 truncate">{album.album}</p>
                <p className="text-[11px] text-zinc-500 truncate">
                  {album.albumArtist ?? 'Unknown artist'}
                  {album.year ? ` · ${album.year}` : ''}
                </p>
                <p className="text-[10px] text-zinc-600 font-mono">
                  {pluralize(album.trackCount, 'track')} · {formatLongDuration(album.durationMs)}
                </p>
              </button>
            );
          })}
        </div>
      </div>
    );
  };

  const renderArtists = () => {
    if (artists.length === 0) {
      return (
        <EmptyState
          title="No artists indexed"
          body={
            activeProviderFilter === 'local'
              ? 'Artists come from the local library index. Scan a music folder to populate them.'
              : 'Artists are built from the selected source. Search or connect a provider to load remote results.'
          }
          action={{ label: 'Scan Music Folder', onClick: () => void pickAndScan() }}
        />
      );
    }

    return (
      <div className="flex-1 overflow-y-auto">
        <ul className="divide-y divide-white/5">
          {artists.map((artist) => (
            <li
              key={`${artist.provider}-${artist.artist}`}
              className="flex items-center justify-between py-3 px-3 hover:bg-white/5 rounded-lg transition-colors group"
            >
              <div className="min-w-0">
                <p className="text-xs font-medium text-zinc-100 truncate">{artist.artist}</p>
                <p className="text-[11px] text-zinc-500 font-mono">
                  {pluralize(artist.albumCount, 'album')} · {pluralize(artist.trackCount, 'track')}
                </p>
              </div>
              <button
                onClick={() => void playArtist(artist)}
                className="p-1.5 rounded-lg text-zinc-400 opacity-0 group-hover:opacity-100 hover:text-white hover:bg-white/10 transition-all"
                title={`Play ${artist.artist}`}
              >
                <Play className="w-3.5 h-3.5" />
              </button>
            </li>
          ))}
        </ul>
      </div>
    );
  };

  const tabs: { id: typeof activeTab; label: string }[] = [
    { id: 'tracks', label: 'Tracks' },
    { id: 'albums', label: 'Albums' },
    { id: 'artists', label: 'Artists' },
  ];

  return (
    <div className="flex-1 flex flex-col p-6 overflow-hidden bg-zinc-950/40">
      <div className="flex items-center justify-between mb-4 gap-4">
        <div className="flex items-center space-x-1 bg-white/5 p-1 rounded-lg border border-white/10 shrink-0">
          {tabs.map((tab) => (
            <button
              key={tab.id}
              onClick={() => setTab(tab.id)}
              className={`px-3 py-1.5 rounded-md text-[11px] font-medium transition-all ${
                activeTab === tab.id
                  ? 'bg-white/15 text-white font-semibold'
                  : 'text-zinc-400 hover:text-zinc-200'
              }`}
            >
              {tab.label}
            </button>
          ))}
        </div>

        <div className="relative flex-1 max-w-md">
          <Search className="w-4 h-4 absolute left-3 top-1/2 -translate-y-1/2 text-zinc-500" />
          <input
            type="text"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            placeholder={searchPlaceholder}
            className="w-full pl-9 pr-8 py-1.5 rounded-lg bg-white/5 border border-white/10 text-xs text-zinc-200 placeholder-zinc-500 focus:outline-none accent-focus"
          />
          {searchQuery && (
            <button
              onClick={() => setSearchQuery('')}
              className="absolute right-2.5 top-1/2 -translate-y-1/2 text-zinc-500 hover:text-zinc-200"
              aria-label="Clear search"
            >
              <X className="w-3.5 h-3.5" />
            </button>
          )}
        </div>

        <div className="text-[11px] text-zinc-500 font-mono shrink-0 flex items-center space-x-2">
          {isLoading && <span className="accent-text">loading…</span>}
          {isScanning && (
            <span className="accent-text">
              scanning {scanProgress.scanned}
              {scanProgress.total ? `/${scanProgress.total}` : ''}
            </span>
          )}
          {activeTab === 'tracks' && <span>{pluralize(tracks.length, 'track')}</span>}
          {activeTab === 'albums' && <span>{pluralize(albums.length, 'album')}</span>}
          {activeTab === 'artists' && <span>{pluralize(artists.length, 'artist')}</span>}
        </div>
      </div>

      {error && (
        <div className="mb-3 flex items-start space-x-2 px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/25 text-[11px] text-red-200">
          <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-px text-red-400" />
          <span className="flex-1 leading-snug">{error}</span>
          <button onClick={clearError} className="text-red-300 hover:text-white" aria-label="Dismiss error">
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      )}

      {isScanning && (
        <div className="mb-3 px-3 py-2 rounded-lg glass-panel text-[11px] text-zinc-400">
          <div className="flex items-center justify-between mb-1.5">
            <span>
              Indexing {scanProgress.scanned}
              {scanProgress.total ? ` of ${scanProgress.total}` : ''} files
            </span>
            <span className="font-mono text-zinc-500 truncate max-w-[50%]">
              {scanProgress.currentFile.split('/').pop()}
            </span>
          </div>
          <div className="h-1 bg-white/10 rounded-full overflow-hidden">
            <div
              className="h-full accent-bg rounded-full transition-all"
              style={{
                width: `${
                  scanProgress.total > 0
                    ? Math.min(100, (scanProgress.scanned / scanProgress.total) * 100)
                    : 0
                }%`,
              }}
            />
          </div>
        </div>
      )}

      {scanSummary && !isScanning && (
        <div className="mb-3 flex items-center justify-between px-3 py-2 rounded-lg glass-panel text-[11px] text-zinc-400">
          <span>
            Last scan indexed {pluralize(scanSummary.indexed, 'track')}
            {scanSummary.removed > 0 ? `, removed ${scanSummary.removed}` : ''} in{' '}
            {(scanSummary.durationMs / 1000).toFixed(1)}s
          </span>
          <button
            onClick={() => useLibraryStore.setState({ scanSummary: null })}
            className="text-zinc-500 hover:text-zinc-200"
            aria-label="Dismiss scan summary"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      )}

      {activeTab === 'tracks' && renderTracks()}
      {activeTab === 'albums' && renderAlbums()}
      {activeTab === 'artists' && renderArtists()}
    </div>
  );
};
