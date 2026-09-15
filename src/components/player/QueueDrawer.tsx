import React from 'react';
import { ChevronDown, ChevronUp, Play, Volume2, X } from 'lucide-react';
import { usePlayerStore } from '../../stores/playerStore';
import { artworkSrc, formatDuration } from '../../services/format';
import type { TrackRecord } from '../../types';

interface QueueRowProps {
  track: TrackRecord;
  orderPosition: number;
  isCurrent: boolean;
  onJump: () => void;
  onMove: (direction: -1 | 1) => void;
  onRemove: () => void;
  canMoveUp: boolean;
  canMoveDown: boolean;
}

const QueueRow: React.FC<QueueRowProps> = ({
  track,
  isCurrent,
  onJump,
  onMove,
  onRemove,
  canMoveUp,
  canMoveDown,
}) => {
  const art = artworkSrc(track.artworkUrl);
  return (
    <div
      className={`group flex items-center space-x-2 px-2 py-1.5 rounded-lg cursor-pointer transition-colors ${
        isCurrent ? 'accent-soft' : 'hover:bg-white/5'
      }`}
      onClick={onJump}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onJump();
        }
      }}
      tabIndex={0}
      role="button"
      aria-label={`Play ${track.title} by ${track.artist}`}
    >
      <div className="w-8 h-8 rounded bg-zinc-800 border border-white/10 overflow-hidden shrink-0 flex items-center justify-center">
        {art ? (
          <img src={art} alt="" className="w-full h-full object-cover" />
        ) : (
          <Play className={`w-3 h-3 ${isCurrent ? 'accent-text' : 'text-zinc-500'}`} />
        )}
      </div>
      <div className="min-w-0 flex-1">
        <p
          className={`text-[11px] truncate ${isCurrent ? 'accent-text font-semibold' : 'text-zinc-200'}`}
        >
          {track.title}
        </p>
        <p className="text-[10px] text-zinc-500 truncate">
          {track.artist} · {formatDuration(track.durationMs)}
        </p>
      </div>

      {isCurrent && <Volume2 className="w-3 h-3 accent-text shrink-0" />}

      <div className="flex items-center space-x-0.5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
        <button
          onClick={(event) => {
            event.stopPropagation();
            onMove(-1);
          }}
          disabled={!canMoveUp}
          title="Move up"
          aria-label="Move up"
          className="p-1 rounded text-zinc-400 hover:text-white disabled:opacity-25"
        >
          <ChevronUp className="w-3 h-3" />
        </button>
        <button
          onClick={(event) => {
            event.stopPropagation();
            onMove(1);
          }}
          disabled={!canMoveDown}
          title="Move down"
          aria-label="Move down"
          className="p-1 rounded text-zinc-400 hover:text-white disabled:opacity-25"
        >
          <ChevronDown className="w-3 h-3" />
        </button>
        <button
          onClick={(event) => {
            event.stopPropagation();
            onRemove();
          }}
          disabled={isCurrent}
          title={isCurrent ? 'The playing track stays in the queue' : 'Remove from queue'}
          aria-label={isCurrent ? 'The playing track stays in the queue' : 'Remove from queue'}
          className="p-1 rounded text-zinc-400 hover:text-red-400 disabled:opacity-25"
        >
          <X className="w-3 h-3" />
        </button>
      </div>
    </div>
  );
};

export const QueueDrawer: React.FC = () => {
  const isQueueOpen = usePlayerStore((s) => s.isQueueOpen);
  const toggleQueue = usePlayerStore((s) => s.toggleQueue);
  const queue = usePlayerStore((s) => s.queue);
  const order = usePlayerStore((s) => s.order);
  const cursor = usePlayerStore((s) => s.cursor);
  const jumpTo = usePlayerStore((s) => s.jumpTo);
  const moveInQueue = usePlayerStore((s) => s.moveInQueue);
  const removeFromQueue = usePlayerStore((s) => s.removeFromQueue);

  const history = order.slice(0, cursor);
  const current = order[cursor];
  const upcoming = order.slice(cursor + 1);

  const row = (queueIndex: number, orderPosition: number, isCurrent: boolean) => {
    const track = queue[queueIndex];
    if (!track) return null;
    return (
      <QueueRow
        key={`${queueIndex}-${track.id}`}
        track={track}
        orderPosition={orderPosition}
        isCurrent={isCurrent}
        onJump={() => jumpTo(queueIndex)}
        onMove={(direction) => moveInQueue(orderPosition, direction)}
        onRemove={() => removeFromQueue(queueIndex)}
        canMoveUp={orderPosition > 0 && orderPosition - 1 !== cursor}
        canMoveDown={orderPosition < order.length - 1 && orderPosition + 1 !== cursor}
      />
    );
  };

  return (
    <>
      {isQueueOpen && (
        <div
          className="absolute inset-0 z-20 bg-black/30 backdrop-blur-[2px]"
          onClick={() => toggleQueue(false)}
        />
      )}
      <div
        className={`absolute inset-y-0 right-0 w-80 z-30 transition-transform duration-200 ease-out ${
          isQueueOpen ? 'translate-x-0' : 'translate-x-full pointer-events-none'
        }`}
      >
        <aside className="h-full glass-panel flex flex-col">
          <header className="flex items-center justify-between px-4 py-3 border-b border-white/5 shrink-0">
            <div>
              <h2 className="text-xs font-semibold text-zinc-200 uppercase tracking-wider">Queue</h2>
              <p className="text-[10px] text-zinc-500 font-mono">
                {queue.length === 0 ? 'empty' : `${cursor + 1} of ${queue.length}`}
              </p>
            </div>
            <button
              onClick={() => toggleQueue(false)}
              className="p-1.5 rounded-lg text-zinc-400 hover:text-zinc-100 hover:bg-white/10 transition-colors"
              aria-label="Close queue"
            >
              <X className="w-4 h-4" />
            </button>
          </header>

          <div className="flex-1 overflow-y-auto px-2 py-3 space-y-4">
            {queue.length === 0 && (
              <p className="px-4 py-10 text-center text-xs text-zinc-500 leading-relaxed">
                Nothing queued yet.
                <br />
                Play a track, album or playlist to fill this up.
              </p>
            )}

            {current !== undefined && queue[current] && (
              <section>
                <h3 className="px-3 pb-1.5 text-[10px] font-semibold text-zinc-500 uppercase tracking-wider">
                  Now playing
                </h3>
                {row(current, cursor, true)}
              </section>
            )}

            {upcoming.length > 0 && (
              <section>
                <h3 className="px-3 pb-1.5 text-[10px] font-semibold text-zinc-500 uppercase tracking-wider">
                  Up next · {upcoming.length}
                </h3>
                {upcoming.map((queueIndex, offset) => row(queueIndex, cursor + 1 + offset, false))}
              </section>
            )}

            {history.length > 0 && (
              <section>
                <h3 className="px-3 pb-1.5 text-[10px] font-semibold text-zinc-500 uppercase tracking-wider">
                  History
                </h3>
                {history.map((queueIndex, orderPosition) => row(queueIndex, orderPosition, false))}
              </section>
            )}
          </div>
        </aside>
      </div>
    </>
  );
};
