import React, { useEffect, useState } from 'react';
import { Shell } from './components/layout/Shell';
import { Sidebar } from './components/sidebar/Sidebar';
import { LibraryView } from './components/library/LibraryView';
import { PlaylistsView } from './components/library/PlaylistsView';
import { SettingsView } from './components/settings/SettingsView';
import { SyncedLyricsView } from './components/lyrics/SyncedLyricsView';
import { PlayerBar } from './components/player/PlayerBar';
import { QueueDrawer } from './components/player/QueueDrawer';
import { tauriBridge } from './services/tauriBridge';
import { usePlayerStore } from './stores/playerStore';
import { useLibraryStore } from './stores/libraryStore';
import { useThemeStore, applyActiveTheme } from './stores/themeStore';
import type { AppView } from './types';

export const App: React.FC = () => {
  const [activeView, setActiveView] = useState<AppView>('library');

  const isLyricsOpen = usePlayerStore((s) => s.isLyricsOpen);
  const bindEngineEvents = usePlayerStore((s) => s.bindEngineEvents);
  const currentArtwork = usePlayerStore((s) => s.currentTrack?.artworkUrl);

  const loadConfig = useLibraryStore((s) => s.loadConfig);
  const refreshSpotifyStatus = useLibraryStore((s) => s.refreshSpotifyStatus);
  const refreshLibrary = useLibraryStore((s) => s.refresh);

  const activeTheme = useThemeStore((s) => s.activeTheme);
  const syncFromArtwork = useThemeStore((s) => s.syncFromArtwork);

  // Startup: theme first so the first paint already carries the accent, then the engine events and
  // the config. `bindEngineEvents` is idempotent, which keeps React strict mode from doubling up.
  // The library itself loads from LibraryView, the view that actually renders it.
  useEffect(() => {
    applyActiveTheme();
    const unbind = bindEngineEvents();
    void loadConfig();
    void refreshSpotifyStatus();
    return unbind;
  }, [bindEngineEvents, loadConfig, refreshSpotifyStatus]);

  useEffect(() => {
    let unbind: (() => void) | undefined;
    let cancelled = false;
    void tauriBridge.onLibraryChanged(() => void refreshLibrary()).then((off) => {
      if (cancelled) off();
      else unbind = off;
    });
    return () => {
      cancelled = true;
      unbind?.();
    };
  }, [refreshLibrary]);

  // Dynamic themes re-tint whenever the artwork behind the current track changes.
  useEffect(() => {
    if (activeTheme === 'dynamic') void syncFromArtwork(currentArtwork);
  }, [activeTheme, currentArtwork, syncFromArtwork]);

  return (
    <Shell>
      <div className="flex-1 flex overflow-hidden relative">
        <Sidebar activeView={activeView} onNavigate={setActiveView} />
        <main className="flex-1 flex overflow-hidden relative">
          {isLyricsOpen ? (
            <SyncedLyricsView />
          ) : (
            <>
              {activeView === 'library' && <LibraryView />}
              {activeView === 'playlists' && <PlaylistsView onNavigate={setActiveView} />}
              {activeView === 'settings' && <SettingsView />}
            </>
          )}
        </main>
        <QueueDrawer />
      </div>
      <PlayerBar />
    </Shell>
  );
};

export default App;
