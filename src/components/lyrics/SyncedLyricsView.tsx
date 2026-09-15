import React, { useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, Languages, Loader2, X } from 'lucide-react';
import { usePlayerStore } from '../../stores/playerStore';
import { tauriBridge } from '../../services/tauriBridge';
import { errorMessage, formatDuration } from '../../services/format';
import type { ParsedLyrics, RomanizationMode, TrackRecord } from '../../types';

const MODES: { id: RomanizationMode; label: string }[] = [
  { id: 'original', label: 'Original' },
  { id: 'romanized', label: 'Romaji' },
  { id: 'dual', label: 'Dual' },
];

const PROVIDER_LABELS: Record<ParsedLyrics['provider'], string> = {
  embedded: 'embedded tags',
  lrclib: 'LRCLIB',
  netease: 'NetEase',
  none: 'no source',
};

const LyricLineItem = React.memo<{
  line: { timeMs: number; text: string; romanizedText?: string };
  isActive: boolean;
  romanizationMode: RomanizationMode;
  onSeek: (ms: number) => void;
  activeRef?: React.Ref<HTMLDivElement>;
}>(({ line, isActive, romanizationMode, onSeek, activeRef }) => {
  const showRomanized = romanizationMode !== 'original' && Boolean(line.romanizedText);
  return (
    <div
      ref={activeRef}
      onClick={() => onSeek(line.timeMs)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onSeek(line.timeMs);
        }
      }}
      tabIndex={0}
      role="button"
      aria-label={`Seek to ${line.text}`}
      className={`cursor-pointer transition-all duration-300 ${
        isActive
          ? 'scale-105 opacity-100 font-bold accent-text'
          : 'opacity-35 hover:opacity-75 text-zinc-300'
      }`}
    >
      {romanizationMode === 'dual' && showRomanized && (
        <p className="text-xs uppercase tracking-wider accent-text opacity-80 mb-1 font-mono">
          {line.romanizedText}
        </p>
      )}
      <p className="text-xl md:text-2xl tracking-wide leading-relaxed">
        {romanizationMode === 'romanized' && showRomanized ? line.romanizedText : line.text}
      </p>
    </div>
  );
});

export const SyncedLyricsView: React.FC = () => {
  const currentTrack = usePlayerStore((s) => s.currentTrack);
  const positionMs = usePlayerStore((s) => s.positionMs);
  const seek = usePlayerStore((s) => s.seek);
  const toggleLyrics = usePlayerStore((s) => s.toggleLyrics);
  const romanizationMode = usePlayerStore((s) => s.romanizationMode);
  const setRomanizationMode = usePlayerStore((s) => s.setRomanizationMode);

  const [lyrics, setLyrics] = useState<ParsedLyrics | null>(null);
  const [track, setTrack] = useState<TrackRecord | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const activeLineRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!currentTrack) {
      setLyrics(null);
      setTrack(null);
      setError(null);
      return;
    }

    let cancelled = false;
    setTrack(currentTrack);
    setIsLoading(true);
    setError(null);

    void tauriBridge
      .getLyrics(currentTrack)
      .then((result) => {
        if (cancelled) return;
        setLyrics(result);
      })
      .catch((caught) => {
        if (cancelled) return;
        setLyrics(null);
        setError(errorMessage(caught));
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [currentTrack]);

  const lines = lyrics?.lines ?? [];
  const hasRomanization = lyrics?.hasRomanization ?? false;

  const activeIndex = useMemo(() => {
    let index = -1;
    for (let i = 0; i < lines.length; i += 1) {
      if (lines[i].timeMs <= positionMs) index = i;
      else break;
    }
    return index;
  }, [lines, positionMs]);

  // Scroll only when the active line changes — following the 100 ms progress tick would jitter.
  useEffect(() => {
    activeLineRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, [activeIndex]);

  const header = (
    <div className="flex items-center justify-between px-8 py-4 border-b border-white/5 z-10 shrink-0">
      <div className="flex items-center space-x-3 min-w-0">
        <Languages className="w-4 h-4 accent-text shrink-0" />
        <div className="min-w-0">
          <p className="text-xs font-semibold text-zinc-300 uppercase tracking-wider truncate">
            {track ? track.title : 'Lyrics'}
          </p>
          <p className="text-[10px] text-zinc-500 font-mono truncate">
            {track ? `${track.artist} · ${PROVIDER_LABELS[lyrics?.provider ?? 'none']}` : 'idle'}
          </p>
        </div>
      </div>

      <div className="flex items-center space-x-1 bg-white/5 p-1 rounded-lg border border-white/10 text-[11px] shrink-0">
        {MODES.map((mode) => (
          <button
            key={mode.id}
            onClick={() => setRomanizationMode(mode.id)}
            disabled={mode.id !== 'original' && !hasRomanization}
            title={
              mode.id !== 'original' && !hasRomanization
                ? 'These lyrics have no romanization'
                : undefined
            }
            className={`px-2.5 py-1 rounded-md transition-all disabled:opacity-30 ${
              romanizationMode === mode.id
                ? 'bg-white/15 text-white font-semibold'
                : 'text-zinc-400 hover:text-zinc-200'
            }`}
          >
            {mode.label}
          </button>
        ))}
      </div>

      <button
        onClick={() => toggleLyrics(false)}
        className="p-1.5 rounded-lg hover:bg-white/10 text-zinc-400 hover:text-zinc-200 transition-colors shrink-0"
        aria-label="Close lyrics"
      >
        <X className="w-4 h-4" />
      </button>
    </div>
  );

  const body = () => {
    if (!currentTrack) {
      return (
        <div className="flex-1 flex items-center justify-center">
          <p className="text-xs text-zinc-500 italic">Play a track to see its lyrics.</p>
        </div>
      );
    }
    if (isLoading) {
      return (
        <div className="flex-1 flex items-center justify-center">
          <Loader2 className="w-5 h-5 animate-spin text-zinc-500" />
        </div>
      );
    }
    if (error) {
      return (
        <div className="flex-1 flex items-center justify-center px-8">
          <div className="flex items-start space-x-2 max-w-md text-[11px] text-red-200">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-px text-red-400" />
            <span>{error}</span>
          </div>
        </div>
      );
    }
    if (lines.length === 0) {
      return (
        <div className="flex-1 flex flex-col items-center justify-center px-8 text-center space-y-2">
          <p className="text-xs text-zinc-300">No lyrics found for this track.</p>
          <p className="text-[11px] text-zinc-500 leading-relaxed max-w-sm">
            Sonora looked in the file's own tags first, then LRCLIB and NetEase. Nothing matched
            “{currentTrack.title}” by {currentTrack.artist}
            {currentTrack.durationMs > 0
              ? ` (${formatDuration(currentTrack.durationMs)})`
              : ''}
            .
          </p>
        </div>
      );
    }

    return (
      <div className="flex-1 overflow-y-auto px-12 py-24 space-y-8 text-center">
        {lines.map((line, index) => (
          <LyricLineItem
            key={`${line.timeMs}-${index}`}
            line={line}
            isActive={index === activeIndex}
            romanizationMode={romanizationMode}
            onSeek={seek}
            activeRef={index === activeIndex ? activeLineRef : undefined}
          />
        ))}
      </div>
    );
  };

  return (
    <div className="flex-1 flex flex-col h-full bg-zinc-950/70 backdrop-blur-2xl relative overflow-hidden">
      {header}
      {body()}
    </div>
  );
};
