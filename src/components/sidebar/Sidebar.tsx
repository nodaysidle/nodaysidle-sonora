import React from 'react';
import {
  Compass,
  Disc3,
  FolderSync,
  HardDrive,
  ListMusic,
  Music2,
  PlaySquare,
  Radio,
  Settings,
} from 'lucide-react';
import { useLibraryStore } from '../../stores/libraryStore';
import { usePlayerStore } from '../../stores/playerStore';
import { pluralize } from '../../services/format';
import type { AppView, ProviderFilter } from '../../types';

interface SidebarProps {
  activeView: AppView;
  onNavigate: (view: AppView) => void;
}

const SOURCES: { id: ProviderFilter; label: string; icon: typeof Compass; color?: string }[] = [
  { id: 'all', label: 'All Sources', icon: Compass },
  { id: 'local', label: 'Local Files', icon: HardDrive, color: 'text-amber-400' },
  { id: 'spotify', label: 'Spotify', icon: Radio, color: 'text-emerald-400' },
  { id: 'ytmusic', label: 'YouTube Music', icon: PlaySquare, color: 'text-red-400' },
];

const LibraryLink: React.FC<{
  icon: typeof Music2;
  label: string;
  isActive: boolean;
  badge?: string;
  onClick: () => void;
}> = ({ icon: Icon, label, isActive, badge, onClick }) => (
  <button
    onClick={onClick}
    className={`relative w-full flex items-center gap-3 px-3 py-2 rounded-lg text-xs transition-colors ${
      isActive
        ? 'accent-soft accent-text font-semibold'
        : 'text-zinc-400 hover:text-zinc-200 hover:bg-white/5 font-medium'
    }`}
  >
    {isActive && <span className="absolute left-0 top-1/2 -translate-y-1/2 h-4 w-0.5 rounded-full accent-bg" />}
    <Icon className={`w-4 h-4 ${isActive ? 'accent-text' : 'text-zinc-500'}`} />
    <span className="flex-1 text-left">{label}</span>
    {badge && <span className="text-[10px] font-mono text-zinc-500">{badge}</span>}
  </button>
);

export const Sidebar: React.FC<SidebarProps> = ({ activeView, onNavigate }) => {
  const activeProviderFilter = useLibraryStore((s) => s.activeProviderFilter);
  const activeTab = useLibraryStore((s) => s.activeTab);
  const setProviderFilter = useLibraryStore((s) => s.setProviderFilter);
  const setTab = useLibraryStore((s) => s.setTab);
  const isScanning = useLibraryStore((s) => s.isScanning);
  const scanProgress = useLibraryStore((s) => s.scanProgress);
  const scanSummary = useLibraryStore((s) => s.scanSummary);
  const error = useLibraryStore((s) => s.error);
  const config = useLibraryStore((s) => s.config);
  const pickAndScan = useLibraryStore((s) => s.pickAndScan);
  const playlists = useLibraryStore((s) => s.playlists);

  const queueLength = usePlayerStore((s) => s.queue.length);
  const toggleQueue = usePlayerStore((s) => s.toggleQueue);

  return (
    <aside className="w-60 glass-nav flex flex-col gap-4 p-3 shrink-0 justify-between min-h-0">
      <div className="space-y-6 overflow-y-auto min-h-0 py-1">
        <div>
          <h3 className="text-[10px] font-semibold text-zinc-500 uppercase tracking-[0.14em] mb-2 px-3">
            Sources
          </h3>
          <div className="space-y-1">
            {SOURCES.map((source) => {
              const Icon = source.icon;
              const isActive = activeView === 'library' && activeProviderFilter === source.id;
              return (
                <button
                  key={source.id}
                  onClick={() => {
                    setProviderFilter(source.id);
                    onNavigate('library');
                  }}
                  className={`relative w-full flex items-center gap-3 px-3 py-2 rounded-lg text-xs transition-colors ${
                    isActive
                      ? 'accent-soft text-white font-semibold'
                      : 'text-zinc-400 hover:text-zinc-200 hover:bg-white/5 font-medium'
                  }`}
                >
                  {isActive && (
                    <span className="absolute left-0 top-1/2 -translate-y-1/2 h-4 w-0.5 rounded-full accent-bg" />
                  )}
                  <Icon className={`w-4 h-4 ${source.color ?? 'text-zinc-300'}`} />
                  <span>{source.label}</span>
                </button>
              );
            })}
          </div>
        </div>

        <div>
          <h3 className="text-[10px] font-semibold text-zinc-500 uppercase tracking-[0.14em] mb-2 px-3">
            Library
          </h3>
          <div className="space-y-1">
            <LibraryLink
              icon={Music2}
              label="Tracks"
              isActive={activeView === 'library' && activeTab === 'tracks'}
              onClick={() => {
                setTab('tracks');
                onNavigate('library');
              }}
            />
            <LibraryLink
              icon={Disc3}
              label="Albums"
              isActive={activeView === 'library' && activeTab === 'albums'}
              onClick={() => {
                setTab('albums');
                onNavigate('library');
              }}
            />
            <LibraryLink
              icon={Compass}
              label="Artists"
              isActive={activeView === 'library' && activeTab === 'artists'}
              onClick={() => {
                setTab('artists');
                onNavigate('library');
              }}
            />
            <LibraryLink
              icon={ListMusic}
              label="Playlists"
              badge={playlists.length > 0 ? String(playlists.length) : undefined}
              isActive={activeView === 'playlists'}
              onClick={() => onNavigate('playlists')}
            />
            <LibraryLink
              icon={ListMusic}
              label="Queue"
              badge={queueLength > 0 ? String(queueLength) : undefined}
              isActive={false}
              onClick={() => toggleQueue(true)}
            />
            <LibraryLink
              icon={Settings}
              label="Settings"
              isActive={activeView === 'settings'}
              onClick={() => onNavigate('settings')}
            />
          </div>
        </div>

        {config.musicFolders.length > 0 && (
          <div>
            <h3 className="text-[10px] font-semibold text-zinc-500 uppercase tracking-[0.14em] mb-2 px-3">
              Folders
            </h3>
            <ul className="space-y-1">
              {config.musicFolders.map((folder) => (
                <li
                  key={folder}
                  title={folder}
                  className="px-3 text-[10px] font-mono text-zinc-500 truncate"
                >
                  {folder.split('/').filter(Boolean).pop() ?? folder}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>

      <div className="pt-4 border-t border-white/5 space-y-2">
        {isScanning && (
          <div className="px-1">
            <div className="flex items-center justify-between text-[10px] font-mono text-zinc-400 mb-1">
              <span>scanning</span>
              <span>
                {scanProgress.scanned}
                {scanProgress.total ? `/${scanProgress.total}` : ''}
              </span>
            </div>
            <div className="h-0.5 bg-white/10 rounded-full overflow-hidden">
              <div
                className="h-full accent-bg transition-all"
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

        {!isScanning && scanSummary && (
          <p className="px-1 text-[10px] font-mono text-zinc-500">
            +{pluralize(scanSummary.indexed, 'track')}
            {scanSummary.removed > 0 ? `, -${scanSummary.removed}` : ''}
          </p>
        )}

        {error && !isScanning && (
          <p className="px-1 text-[10px] text-red-300 line-clamp-2" title={error}>
            {error}
          </p>
        )}

        <button
          onClick={() => void pickAndScan()}
          disabled={isScanning}
          className="w-full flex items-center justify-center space-x-2 px-3 py-2 rounded-lg text-xs font-medium border border-white/10 text-zinc-300 hover:text-white hover:bg-white/5 transition-all disabled:opacity-50"
        >
          <FolderSync className="w-3.5 h-3.5 accent-text" />
          <span>{isScanning ? 'Scanning…' : 'Scan Music Folder'}</span>
        </button>
      </div>
    </aside>
  );
};
