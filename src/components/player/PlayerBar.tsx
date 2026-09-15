import React, { useRef, useState } from 'react';
import {
  AlertCircle,
  ListMusic,
  Loader2,
  Mic2,
  Pause,
  Play,
  Repeat,
  Repeat1,
  Shuffle,
  SkipBack,
  SkipForward,
  Volume2,
  VolumeX,
  X,
} from 'lucide-react';
import { usePlayerStore } from '../../stores/playerStore';
import { artworkSrc, formatDuration } from '../../services/format';

const Scrubber: React.FC = () => {
  const positionMs = usePlayerStore((s) => s.positionMs);
  const durationMs = usePlayerStore((s) => s.durationMs);
  const currentTrack = usePlayerStore((s) => s.currentTrack);
  const seek = usePlayerStore((s) => s.seek);

  const barRef = useRef<HTMLDivElement | null>(null);
  const [dragMs, setDragMs] = useState<number | null>(null);
  const [hoverMs, setHoverMs] = useState<number | null>(null);

  const duration = durationMs || currentTrack?.durationMs || 0;
  const shownMs = dragMs ?? positionMs;
  const percent = duration > 0 ? Math.min(100, (shownMs / duration) * 100) : 0;

  const msAtClientX = (clientX: number): number => {
    const element = barRef.current;
    if (!element || duration <= 0) return 0;
    const rect = element.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    return Math.round(ratio * duration);
  };

  return (
    <div className="w-full flex items-center gap-3 text-[10px] font-mono text-zinc-400">
      <span>{formatDuration(shownMs)}</span>
      <div
        ref={barRef}
        role="slider"
        aria-label="Seek"
        aria-valuemin={0}
        aria-valuemax={duration}
        aria-valuenow={shownMs}
        tabIndex={0}
        className="relative flex-1 h-4 flex items-center cursor-pointer group"
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          setDragMs(msAtClientX(event.clientX));
        }}
        onPointerMove={(event) => {
          const value = msAtClientX(event.clientX);
          setHoverMs(value);
          if (dragMs !== null) setDragMs(value);
        }}
        onPointerUp={(event) => {
          if (dragMs === null) return;
          event.currentTarget.releasePointerCapture(event.pointerId);
          seek(msAtClientX(event.clientX));
          setDragMs(null);
        }}
        onPointerLeave={() => setHoverMs(null)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowRight') seek(Math.min(duration, shownMs + 5000));
          if (event.key === 'ArrowLeft') seek(Math.max(0, shownMs - 5000));
        }}
      >
        <div className="w-full h-1.5 bg-white/10 rounded-full overflow-hidden">
          <div className="h-full accent-bg rounded-full" style={{ width: `${percent}%` }} />
        </div>
        {/* Hover timestamp tooltip */}
        {hoverMs !== null && duration > 0 && (
          <div
            className="absolute -top-7 px-1.5 py-0.5 rounded bg-zinc-900/90 border border-white/10 text-[10px] font-mono text-zinc-200 pointer-events-none -translate-x-1/2"
            style={{ left: `${(hoverMs / duration) * 100}%` }}
          >
            {formatDuration(hoverMs)}
          </div>
        )}
      </div>
      <span>{formatDuration(duration)}</span>
    </div>
  );
};

export const PlayerBar: React.FC = () => {
  const currentTrack = usePlayerStore((s) => s.currentTrack);
  const nextTrack = usePlayerStore((s) => s.nextTrack);
  const status = usePlayerStore((s) => s.status);
  const isLoading = usePlayerStore((s) => s.isLoading);
  const volume = usePlayerStore((s) => s.volume);
  const isMuted = usePlayerStore((s) => s.isMuted);
  const isNormalizing = usePlayerStore((s) => s.isNormalizing);
  const isGapless = usePlayerStore((s) => s.isGapless);
  const spotifyDeviceName = usePlayerStore((s) => s.spotifyDeviceName);
  const isLyricsOpen = usePlayerStore((s) => s.isLyricsOpen);
  const isQueueOpen = usePlayerStore((s) => s.isQueueOpen);
  const shuffle = usePlayerStore((s) => s.shuffle);
  const repeatMode = usePlayerStore((s) => s.repeatMode);
  const notice = usePlayerStore((s) => s.notice);

  const playPause = usePlayerStore((s) => s.playPause);
  const next = usePlayerStore((s) => s.next);
  const previous = usePlayerStore((s) => s.previous);
  const setVolume = usePlayerStore((s) => s.setVolume);
  const toggleMute = usePlayerStore((s) => s.toggleMute);
  const toggleShuffle = usePlayerStore((s) => s.toggleShuffle);
  const cycleRepeat = usePlayerStore((s) => s.cycleRepeat);
  const toggleNormalization = usePlayerStore((s) => s.toggleNormalization);
  const toggleLyrics = usePlayerStore((s) => s.toggleLyrics);
  const toggleQueue = usePlayerStore((s) => s.toggleQueue);
  const setNotice = usePlayerStore((s) => s.setNotice);

  const isPlaying = status === 'playing';
  const art = artworkSrc(currentTrack?.artworkUrl);

  return (
    <footer className="relative h-20 glass-player grid grid-cols-[minmax(0,1fr)_minmax(18rem,32rem)_minmax(0,1fr)] items-center gap-6 px-6 select-none shrink-0 z-20">
      {notice && (
        <div className="absolute -top-11 right-4 max-w-md flex items-start space-x-2 px-3 py-2 rounded-lg glass-panel text-[11px] text-zinc-200 shadow-lg">
          <AlertCircle className="w-3.5 h-3.5 accent-text shrink-0 mt-px" />
          <span className="leading-snug">{notice}</span>
          <button
            onClick={() => setNotice(null)}
            className="text-zinc-500 hover:text-zinc-200 transition-colors"
            aria-label="Dismiss message"
          >
            <X className="w-3 h-3" />
          </button>
        </div>
      )}

      {/* Now playing */}
      <div className="flex items-center gap-3 min-w-0">
        {currentTrack ? (
          <>
            <div className="w-12 h-12 rounded-lg bg-zinc-800 border border-white/10 overflow-hidden shrink-0 flex items-center justify-center">
              {art ? (
                <img src={art} alt={currentTrack.album} className="w-full h-full object-cover" />
              ) : (
                <span className="text-zinc-500 font-bold text-[10px]">SONORA</span>
              )}
            </div>
            <div className="min-w-0">
              <h4 className="text-xs font-semibold text-zinc-100 truncate">{currentTrack.title}</h4>
              <p className="text-[11px] text-zinc-400 truncate">
                {currentTrack.artist}
                {currentTrack.album ? ` — ${currentTrack.album}` : ''}
              </p>
              <p className="text-[10px] text-zinc-600 truncate">
                {currentTrack.provider === 'spotify'
                  ? (spotifyDeviceName && spotifyDeviceName !== 'Sonora (Native)'
                      ? `Spotify · ${spotifyDeviceName}`
                      : 'Spotify (Native)')
                  : currentTrack.provider === 'ytmusic'
                    ? 'YouTube Music'
                    : 'Local library'}
              </p>
            </div>
          </>
        ) : (
          <div className="text-xs text-zinc-500 italic">Nothing playing</div>
        )}
      </div>

      {/* Transport and scrubber */}
      <div className="flex flex-col items-center gap-2 min-w-0">
        <div className="flex items-center gap-4">
          <button
            onClick={toggleShuffle}
            title={shuffle ? 'Shuffle on' : 'Shuffle off'}
            aria-label={shuffle ? 'Turn shuffle off' : 'Turn shuffle on'}
            className={`transition-colors ${shuffle ? 'accent-text' : 'text-zinc-400 hover:text-zinc-200'}`}
          >
            <Shuffle className="w-4 h-4" />
          </button>
          <button
            onClick={previous}
            disabled={!currentTrack}
            className="text-zinc-300 hover:text-white transition-colors disabled:opacity-40 disabled:hover:text-zinc-300"
            title="Previous"
            aria-label="Previous track"
          >
            <SkipBack className="w-4 h-4" />
          </button>
          <button
            onClick={playPause}
            disabled={!currentTrack}
            title={isPlaying ? 'Pause' : 'Play'}
            aria-label={isPlaying ? 'Pause' : 'Play'}
            className="w-9 h-9 rounded-full accent-bg text-black flex items-center justify-center hover:scale-105 transition-transform disabled:opacity-40 disabled:hover:scale-100"
          >
            {isLoading ? (
              <Loader2 className="w-4 h-4 animate-spin text-black" />
            ) : isPlaying ? (
              <Pause className="w-4 h-4 fill-black" />
            ) : (
              <Play className="w-4 h-4 fill-black ml-0.5" />
            )}
          </button>
          <button
            onClick={next}
            disabled={!currentTrack}
            className="text-zinc-300 hover:text-white transition-colors disabled:opacity-40 disabled:hover:text-zinc-300"
            title="Next"
            aria-label="Next track"
          >
            <SkipForward className="w-4 h-4" />
          </button>
          <button
            onClick={cycleRepeat}
            title={
              repeatMode === 'off'
                ? 'Repeat off'
                : repeatMode === 'track'
                  ? 'Repeat this track'
                  : 'Repeat queue'
            }
            aria-label={
              repeatMode === 'off'
                ? 'Turn repeat on'
                : repeatMode === 'track'
                  ? 'Repeat this track'
                  : 'Repeat queue'
            }
            className={`transition-colors ${
              repeatMode === 'off' ? 'text-zinc-400 hover:text-zinc-200' : 'accent-text'
            }`}
          >
            {repeatMode === 'track' ? <Repeat1 className="w-4 h-4" /> : <Repeat className="w-4 h-4" />}
          </button>
        </div>

        {/* Isolated Scrubber */}
        <Scrubber />
      </div>

      {/* Output controls */}
      <div className="flex items-center justify-end gap-2 min-w-0">
        <div
          className="hidden xl:flex items-center gap-1.5 px-2 py-1 rounded bg-white/5 text-[10px] font-mono text-zinc-500 shrink-0"
          title={
            isGapless && nextTrack
              ? `Gapless: “${nextTrack.title}” is pre-buffered`
              : currentTrack?.provider === 'spotify'
                ? 'Spotify playback is controlled by your Spotify client; Sonora cannot pre-buffer the next track'
                : 'Gapless pre-buffering is not ready for the next track'
          }
        >
          <span className={`w-1.5 h-1.5 rounded-full ${isGapless && nextTrack ? 'accent-bg' : 'bg-zinc-600'}`} />
          <span>GAPLESS</span>
        </div>

        <button
          onClick={toggleNormalization}
          title={isNormalizing ? 'EBU R128 normalization active (-14 LUFS)' : 'Normalization off'}
          aria-label={isNormalizing ? 'Turn loudness normalization off' : 'Turn loudness normalization on'}
          className={`px-2 py-1 rounded text-[10px] font-bold font-mono transition-colors ${
            isNormalizing ? 'accent-soft accent-text accent-border border' : 'bg-white/5 text-zinc-500 hover:text-zinc-400'
          }`}
        >
          R128
        </button>

        <button
          onClick={() => toggleQueue()}
          title="Play queue"
          aria-label="Toggle play queue"
          className={`p-1.5 rounded-lg transition-colors ${
            isQueueOpen ? 'bg-white/15 accent-text' : 'text-zinc-400 hover:text-zinc-200'
          }`}
        >
          <ListMusic className="w-4 h-4" />
        </button>

        <button
          onClick={() => toggleLyrics()}
          title="Synced lyrics and romanization"
          aria-label="Toggle synced lyrics"
          className={`p-1.5 rounded-lg transition-colors ${
            isLyricsOpen ? 'bg-white/15 accent-text' : 'text-zinc-400 hover:text-zinc-200'
          }`}
        >
          <Mic2 className="w-4 h-4" />
        </button>

        <div className="flex items-center space-x-2">
          <button
            onClick={toggleMute}
            className="text-zinc-400 hover:text-zinc-200"
            title={isMuted ? 'Unmute' : 'Mute'}
            aria-label={isMuted ? 'Unmute' : 'Mute'}
          >
            {isMuted || volume === 0 ? (
              <VolumeX className="w-4 h-4 text-red-400" />
            ) : (
              <Volume2 className="w-4 h-4" />
            )}
          </button>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={isMuted ? 0 : volume}
            onChange={(event) => setVolume(parseFloat(event.target.value))}
            aria-label="Volume"
            className="accent-range w-24 cursor-pointer"
          />
        </div>
      </div>
    </footer>
  );
};
