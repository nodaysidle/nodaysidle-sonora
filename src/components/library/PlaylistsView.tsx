import React, { useEffect } from 'react';
import { AlertCircle, ChevronLeft, ListMusic, Play, RefreshCw, X } from 'lucide-react';
import { useLibraryStore } from '../../stores/libraryStore';
import { usePlayerStore } from '../../stores/playerStore';
import { artworkSrc, formatDuration, pluralize } from '../../services/format';
import type { AppView } from '../../types';

interface PlaylistsViewProps {
  onNavigate: (view: AppView) => void;
}

export const PlaylistsView: React.FC<PlaylistsViewProps> = ({ onNavigate }) => {
  const playlists = useLibraryStore((s) => s.playlists);
  const playlistTracks = useLibraryStore((s) => s.playlistTracks);
  const selectedPlaylist = useLibraryStore((s) => s.selectedPlaylist);
  const spotifyConnected = useLibraryStore((s) => s.spotifyConnected);
  const isLoading = useLibraryStore((s) => s.isLoading);
  const error = useLibraryStore((s) => s.error);
  const loadPlaylists = useLibraryStore((s) => s.loadPlaylists);
  const openPlaylist = useLibraryStore((s) => s.openPlaylist);

  const playFromList = usePlayerStore((s) => s.playFromList);
  const currentTrack = usePlayerStore((s) => s.currentTrack);

  useEffect(() => {
    if (spotifyConnected) void loadPlaylists();
  }, [spotifyConnected, loadPlaylists]);

  if (!spotifyConnected) {
    return (
      <div className="flex-1 flex items-center justify-center p-8 bg-zinc-950/40">
        <div className="max-w-sm text-center space-y-3">
          <ListMusic className="w-8 h-8 text-zinc-600 mx-auto" />
          <h3 className="text-sm font-semibold text-zinc-200">No playlists yet</h3>
          <p className="text-xs text-zinc-500 leading-relaxed">
            Spotify playlists show up here once your account is connected. Local folders are browsed
            through Albums and Artists instead.
          </p>
          <button
            onClick={() => onNavigate('settings')}
            className="mt-2 inline-flex items-center space-x-2 px-3.5 py-2 rounded-lg text-xs font-medium accent-soft accent-text accent-border border"
          >
            <span>Open Settings</span>
          </button>
        </div>
      </div>
    );
  }

  if (selectedPlaylist) {
    return (
      <div className="flex-1 flex flex-col p-6 overflow-hidden bg-zinc-950/40">
        <div className="flex items-center justify-between mb-5 gap-4">
          <div className="flex items-center space-x-3 min-w-0">
            <button
              onClick={() => void openPlaylist(null)}
              className="p-1.5 rounded-lg text-zinc-400 hover:text-white hover:bg-white/10 transition-colors"
              aria-label="Back to playlists"
            >
              <ChevronLeft className="w-4 h-4" />
            </button>
            <div className="min-w-0">
              <h2 className="text-sm font-semibold text-zinc-100 truncate">
                {selectedPlaylist.title}
              </h2>
              <p className="text-[11px] text-zinc-500 font-mono">
                {pluralize(playlistTracks.length, 'track')}
                {selectedPlaylist.description ? ` · ${selectedPlaylist.description}` : ''}
              </p>
            </div>
          </div>
          <button
            onClick={() => playFromList(playlistTracks, 0)}
            disabled={playlistTracks.length === 0}
            className="shrink-0 inline-flex items-center space-x-2 px-3.5 py-2 rounded-lg text-xs font-semibold accent-bg text-black hover:scale-105 transition-transform disabled:opacity-40 disabled:hover:scale-100"
          >
            <Play className="w-3.5 h-3.5 fill-black" />
            <span>Play</span>
          </button>
        </div>

        {error && (
          <div className="mb-3 flex items-start space-x-2 px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/25 text-[11px] text-red-200">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-px text-red-400" />
            <span className="flex-1 leading-snug">{error}</span>
          </div>
        )}

        <div className="flex-1 overflow-y-auto">
          {isLoading && <p className="text-xs text-zinc-500 italic">Loading tracks…</p>}
          <ul className="divide-y divide-white/5">
            {playlistTracks.map((track, index) => {
              const isCurrent = currentTrack?.id === track.id;
              return (
                <li
                  key={`${track.id}-${index}`}
                  onClick={() => playFromList(playlistTracks, index)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter' || event.key === ' ') {
                      event.preventDefault();
                      playFromList(playlistTracks, index);
                    }
                  }}
                  tabIndex={0}
                  role="button"
                  aria-label={`Play ${track.title} by ${track.artist}`}
                  className={`group flex items-center space-x-3 px-3 py-2.5 rounded-lg cursor-pointer transition-colors ${
                    isCurrent ? 'accent-soft' : 'hover:bg-white/5'
                  }`}
                >
                  <span className="w-5 text-[11px] font-mono text-zinc-500 text-right">
                    {index + 1}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p
                      className={`text-xs truncate ${isCurrent ? 'accent-text font-semibold' : 'text-zinc-100'}`}
                    >
                      {track.title}
                    </p>
                    <p className="text-[11px] text-zinc-500 truncate">
                      {track.artist} · {track.album}
                    </p>
                  </div>
                  <span className="text-[11px] font-mono text-zinc-500">
                    {formatDuration(track.durationMs)}
                  </span>
                  <Play className="w-3.5 h-3.5 accent-text opacity-0 group-hover:opacity-100 transition-opacity" />
                </li>
              );
            })}
          </ul>
          {!isLoading && playlistTracks.length === 0 && !error && (
            <p className="text-xs text-zinc-500 italic">This playlist came back empty.</p>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col p-6 overflow-hidden bg-zinc-950/40">
      <div className="flex items-center justify-between mb-5">
        <h2 className="text-sm font-semibold text-zinc-200 uppercase tracking-wider">Playlists</h2>
        <button
          onClick={() => void loadPlaylists()}
          disabled={isLoading}
          className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg text-[11px] text-zinc-300 border border-white/10 hover:bg-white/5 transition-colors disabled:opacity-50"
        >
          <RefreshCw className={`w-3.5 h-3.5 ${isLoading ? 'animate-spin' : ''}`} />
          <span>Refresh</span>
        </button>
      </div>

      {error && (
        <div className="mb-3 flex items-start space-x-2 px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/25 text-[11px] text-red-200">
          <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-px text-red-400" />
          <span className="flex-1 leading-snug">{error}</span>
          <button
            onClick={() => useLibraryStore.setState({ error: null })}
            className="text-red-300 hover:text-white"
            aria-label="Dismiss error"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      )}

      <div className="flex-1 overflow-y-auto">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-5">
          {playlists.map((playlist) => {
            const art = artworkSrc(playlist.artworkUrl);
            return (
              <button
                key={playlist.id}
                onClick={() => void openPlaylist(playlist)}
                className="group text-left"
              >
                <div className="relative aspect-square rounded-xl overflow-hidden bg-zinc-800/80 border border-white/10">
                  {art ? (
                    <img src={art} alt={playlist.title} className="w-full h-full object-cover" />
                  ) : (
                    <div className="w-full h-full flex items-center justify-center">
                      <ListMusic className="w-8 h-8 text-zinc-600" />
                    </div>
                  )}
                  <div className="absolute inset-0 bg-black/50 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
                    <span className="w-10 h-10 rounded-full accent-bg text-black flex items-center justify-center accent-shadow">
                      <Play className="w-4 h-4 fill-black ml-0.5" />
                    </span>
                  </div>
                </div>
                <p className="mt-2 text-xs font-medium text-zinc-100 truncate">{playlist.title}</p>
                <p className="text-[10px] text-zinc-500 font-mono">
                  {pluralize(playlist.trackCount, 'track')}
                </p>
              </button>
            );
          })}
        </div>

        {!isLoading && playlists.length === 0 && !error && (
          <p className="py-10 text-center text-xs text-zinc-500">
            Spotify returned no playlists for this account.
          </p>
        )}
      </div>
    </div>
  );
};
