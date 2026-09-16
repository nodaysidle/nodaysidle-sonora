import React, { useEffect, useState } from 'react';
import {
  AlertCircle,
  Check,
  Copy,
  FolderPlus,
  Info,
  Palette,
  Plug,
  Unplug,
  X,
} from 'lucide-react';
import { useLibraryStore } from '../../stores/libraryStore';
import { usePlayerStore } from '../../stores/playerStore';
import { useThemeStore, THEME_PRESET_META } from '../../stores/themeStore';
import { isTauriEnvironment, tauriBridge } from '../../services/tauriBridge';
import { pluralize } from '../../services/format';
import type { ThemePreset } from '../../types';

const Section: React.FC<{ title: string; icon: typeof Plug; children: React.ReactNode }> = ({
  title,
  icon: Icon,
  children,
}) => (
  <section className="glass-panel rounded-xl p-5 space-y-4">
    <h2 className="flex items-center space-x-2 text-xs font-semibold text-zinc-200 uppercase tracking-wider">
      <Icon className="w-3.5 h-3.5 accent-text" />
      <span>{title}</span>
    </h2>
    {children}
  </section>
);

export const SettingsView: React.FC = () => {
  const config = useLibraryStore((s) => s.config);
  const spotifyConnected = useLibraryStore((s) => s.spotifyConnected);
  const isSpotifyConnecting = useLibraryStore((s) => s.isSpotifyConnecting);
  const error = useLibraryStore((s) => s.error);
  const loadConfig = useLibraryStore((s) => s.loadConfig);
  const refreshSpotifyStatus = useLibraryStore((s) => s.refreshSpotifyStatus);
  const connectSpotify = useLibraryStore((s) => s.connectSpotify);
  const disconnectSpotify = useLibraryStore((s) => s.disconnectSpotify);
  const saveMusicFolders = useLibraryStore((s) => s.saveMusicFolders);
  const saveSpotifyClientId = useLibraryStore((s) => s.saveSpotifyClientId);
  const pickAndScan = useLibraryStore((s) => s.pickAndScan);
  const clearError = useLibraryStore((s) => s.clearError);

  const isNormalizing = usePlayerStore((s) => s.isNormalizing);
  const toggleNormalization = usePlayerStore((s) => s.toggleNormalization);
  const volume = usePlayerStore((s) => s.volume);

  const activeTheme = useThemeStore((s) => s.activeTheme);
  const setTheme = useThemeStore((s) => s.setTheme);

  const [settingsPath, setSettingsPath] = useState<string | null>(null);
  const [trackCount, setTrackCount] = useState<number | null>(null);
  const [clientIdDraft, setClientIdDraft] = useState('');
  const [clientIdSaved, setClientIdSaved] = useState(false);
  const [redirectCopied, setRedirectCopied] = useState(false);
  const [authUrl, setAuthUrl] = useState<string | null>(null);
  const [copiedAuth, setCopiedAuth] = useState(false);

  // Mirrors whatever the backend currently holds, including a value edited outside the app.
  useEffect(() => {
    setClientIdDraft(config.spotifyClientId ?? '');
  }, [config.spotifyClientId]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void tauriBridge.onSpotifyAuthUrl((url) => {
      setAuthUrl(url);
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  useEffect(() => {
    void loadConfig();
    void refreshSpotifyStatus();
    void tauriBridge
      .countTracks('all')
      .then(setTrackCount)
      .catch(() => setTrackCount(0));
    void tauriBridge.settingsFilePath().then(setSettingsPath);
  }, [loadConfig, refreshSpotifyStatus]);

  const handleConnect = async () => {
    setAuthUrl(null);
    if (clientIdDraft.trim() && clientIdDraft.trim() !== (config.spotifyClientId ?? '')) {
      await saveSpotifyClientId(clientIdDraft.trim());
      setClientIdSaved(true);
      setTimeout(() => setClientIdSaved(false), 2500);
    }
    await connectSpotify();
  };

  return (
    <div className="flex-1 overflow-y-auto bg-zinc-950/40">
      <div className="max-w-3xl mx-auto p-6 space-y-5">
        <header className="flex items-baseline justify-between">
          <h1 className="text-sm font-semibold text-zinc-100 uppercase tracking-wider">Settings</h1>
          <span className="text-[11px] font-mono text-zinc-500">
            {trackCount === null ? '—' : pluralize(trackCount, 'track')} indexed
          </span>
        </header>

        {error && (
          <div className="flex items-start space-x-2 px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/25 text-[11px] text-red-200">
            <AlertCircle className="w-3.5 h-3.5 shrink-0 mt-px text-red-400" />
            <span className="flex-1 leading-snug">{error}</span>
            <button onClick={clearError} className="text-red-300 hover:text-white" aria-label="Dismiss">
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
        )}

        <Section title="Spotify" icon={Plug}>
          <div className="flex items-center justify-between">
            <div className="flex items-center space-x-2 text-xs">
              <span
                className={`w-2 h-2 rounded-full ${spotifyConnected ? 'bg-emerald-400' : 'bg-zinc-600'}`}
              />
              <span className="text-zinc-300">
                {spotifyConnected ? 'Connected to Spotify' : 'Not connected'}
              </span>
            </div>
            <div className="flex items-center space-x-2">
              <button
                onClick={() => void handleConnect()}
                disabled={isSpotifyConnecting}
                className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg text-[11px] font-semibold accent-bg text-black hover:scale-105 transition-transform disabled:opacity-50 disabled:hover:scale-100"
              >
                <Plug className="w-3.5 h-3.5" />
                <span>{isSpotifyConnecting ? 'Connecting…' : spotifyConnected ? 'Reconnect' : 'Connect'}</span>
              </button>
              {spotifyConnected && (
                <button
                  onClick={() => void disconnectSpotify()}
                  className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg text-[11px] text-zinc-300 border border-white/10 hover:bg-white/5 transition-colors"
                >
                  <Unplug className="w-3.5 h-3.5" />
                  <span>Disconnect</span>
                </button>
              )}
            </div>
          </div>

          {authUrl && isSpotifyConnecting && (
            <div className="p-3 rounded-lg bg-emerald-500/10 border border-emerald-500/25 space-y-2">
              <div className="flex items-center justify-between text-xs text-emerald-300 font-medium">
                <span>Spotify Authorization Request</span>
                <span className="text-[10px] text-emerald-400/80 font-mono">Port 8899</span>
              </div>
              <p className="text-[11px] text-zinc-300">
                If your browser didn't open automatically, click below to authorize in Spotify:
              </p>
              <div className="flex items-center space-x-2">
                <button
                  type="button"
                  onClick={() => void tauriBridge.openUrl(authUrl)}
                  className="px-3 py-1.5 rounded-lg bg-emerald-400 hover:bg-emerald-300 text-black font-semibold text-[11px] transition-colors"
                >
                  Open in Browser
                </button>
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard.writeText(authUrl);
                    setCopiedAuth(true);
                    setTimeout(() => setCopiedAuth(false), 2000);
                  }}
                  className="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-zinc-200 text-[11px] transition-colors"
                >
                  {copiedAuth ? 'Copied Link!' : 'Copy Link'}
                </button>
              </div>
            </div>
          )}

          <div className="space-y-1.5">
            <label
              htmlFor="spotify-client-id"
              className="text-[11px] font-semibold text-zinc-500 uppercase tracking-wider"
            >
              Client ID
            </label>
            <div className="flex items-center space-x-2">
              <input
                id="spotify-client-id"
                type="text"
                value={clientIdDraft}
                onChange={(event) => {
                  setClientIdDraft(event.target.value);
                  setClientIdSaved(false);
                }}
                placeholder="Paste your Spotify app's Client ID"
                spellCheck={false}
                className="flex-1 px-3 py-2 rounded-lg bg-white/5 border border-white/10 text-xs font-mono text-zinc-300 placeholder-zinc-600 focus:outline-none accent-focus"
              />
              <button
                onClick={() => {
                  void saveSpotifyClientId(clientIdDraft).then(() => {
                    setClientIdSaved(true);
                    setTimeout(() => setClientIdSaved(false), 2500);
                  });
                }}
                disabled={
                  !isTauriEnvironment() ||
                  clientIdDraft.trim() === (config.spotifyClientId ?? '')
                }
                title={isTauriEnvironment() ? undefined : 'Needs the Sonora desktop app'}
                className="px-3 py-2 rounded-lg text-[11px] font-semibold accent-bg text-black hover:scale-105 transition-transform disabled:opacity-40 disabled:hover:scale-100 shrink-0"
              >
                {clientIdSaved ? 'Saved' : 'Save'}
              </button>
            </div>
            <div className="p-3 rounded-lg bg-white/5 border border-white/10 space-y-2 mt-2">
              <div className="flex items-center justify-between">
                <span className="text-[11px] text-zinc-400 font-medium">Spotify App Redirect URI</span>
                <button
                  type="button"
                  onClick={() => {
                    void navigator.clipboard
                      .writeText('http://127.0.0.1:8899/callback')
                      .then(() => {
                        setRedirectCopied(true);
                        setTimeout(() => setRedirectCopied(false), 2000);
                      })
                      .catch(() => setRedirectCopied(false));
                  }}
                  aria-label="Copy Spotify redirect URI"
                  className="inline-flex items-center space-x-1 px-2 py-1 rounded bg-white/10 hover:bg-white/15 text-[10px] text-zinc-200 transition-colors"
                >
                  {redirectCopied ? <Check className="w-3 h-3 text-lime-400" /> : <Copy className="w-3 h-3" />}
                  <span>{redirectCopied ? 'Copied!' : 'Copy URI'}</span>
                </button>
              </div>
              <code className="block text-[11px] font-mono text-lime-400/90 bg-black/40 px-2 py-1 rounded select-all">
                http://127.0.0.1:8899/callback
              </code>
              <p className="text-[10px] text-zinc-500 leading-normal">
                1. Open <a href="https://developer.spotify.com/dashboard" target="_blank" rel="noreferrer" className="text-zinc-300 underline">developer.spotify.com/dashboard</a> and create an app.<br/>
                2. In App Settings, paste the Redirect URI above and save.<br/>
                3. Copy your Client ID into the field above and click Connect.
              </p>
            </div>
            <p className="flex items-start space-x-1.5 text-[11px] text-zinc-500 leading-relaxed mt-2">
              <Info className="w-3.5 h-3.5 shrink-0 mt-px text-zinc-600" />
              <span>
                Stored in{' '}
                <code className="font-mono text-zinc-400">
                  {settingsPath ?? '<app data>/settings.json'}
                </code>
                . Tokens stay in the OS keyring.
              </span>
            </p>
            <p className="text-[11px] text-zinc-500 leading-relaxed">
              Spotify tracks are controlled through your active Spotify Connect device. Sonora does
              not download or decode Spotify audio, and local gapless playback does not apply to it.
            </p>
          </div>
        </Section>

        <Section title="Playback" icon={Check}>
          <div className="flex items-center justify-between">
            <div>
              <p className="text-xs text-zinc-200">EBU R128 loudness normalization</p>
              <p className="text-[11px] text-zinc-500">
                Applies per-track replay gain towards -14 LUFS in the Rust audio pipeline.
              </p>
            </div>
            <button
              onClick={toggleNormalization}
              role="switch"
              aria-checked={isNormalizing}
              aria-label="Toggle EBU R128 loudness normalization"
              className={`relative w-10 h-5 rounded-full transition-colors shrink-0 ${
                isNormalizing ? 'accent-bg' : 'bg-white/15'
              }`}
            >
              <span
                className={`absolute top-0.5 w-4 h-4 rounded-full bg-black/80 transition-all ${
                  isNormalizing ? 'left-5' : 'left-0.5'
                }`}
              />
            </button>
          </div>

          <div className="flex items-center justify-between text-[11px] text-zinc-500">
            <span>Output volume</span>
            <span className="font-mono">{Math.round(volume * 100)}%</span>
          </div>

          <p className="text-[11px] text-zinc-500 leading-relaxed">
            Local and YouTube Music tracks are gapless only when the next decoder is successfully
            pre-buffered. Spotify transport stays on your Spotify Connect device.
          </p>
        </Section>

        <Section title="Music Folders" icon={FolderPlus}>
          {config.musicFolders.length === 0 ? (
            <p className="text-[11px] text-zinc-500">
              No folders yet. Adding one indexes its audio files in the background and remembers the
              path in the config.
            </p>
          ) : (
            <ul className="space-y-1.5">
              {config.musicFolders.map((folder) => (
                <li
                  key={folder}
                  className="flex items-center justify-between px-3 py-2 rounded-lg bg-white/5 text-[11px] font-mono text-zinc-300"
                >
                  <span className="truncate" title={folder}>
                    {folder}
                  </span>
                  <button
                    onClick={() =>
                      void saveMusicFolders(config.musicFolders.filter((item) => item !== folder))
                    }
                    className="ml-3 p-1 rounded text-zinc-500 hover:text-red-400 transition-colors shrink-0"
                    title="Remove folder from the config (files stay on disk)"
                    aria-label={`Remove ${folder}`}
                  >
                    <X className="w-3.5 h-3.5" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <button
            onClick={() => void pickAndScan()}
            className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg text-[11px] text-zinc-300 border border-white/10 hover:bg-white/5 transition-colors"
          >
            <FolderPlus className="w-3.5 h-3.5 accent-text" />
            <span>Add &amp; scan folder</span>
          </button>
        </Section>

        <Section title="Theme" icon={Palette}>
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
            {(Object.keys(THEME_PRESET_META) as ThemePreset[]).map((preset) => {
              const meta = THEME_PRESET_META[preset];
              const isActive = activeTheme === preset;
              return (
                <button
                  key={preset}
                  onClick={() => setTheme(preset)}
                  aria-pressed={isActive}
                  className={`flex items-center space-x-2.5 px-3 py-2 rounded-lg border text-left transition-all ${
                    isActive
                      ? 'border-white/25 bg-white/10'
                      : 'border-white/10 hover:bg-white/5'
                  }`}
                >
                  <span
                    className="w-4 h-4 rounded-full border border-white/20 shrink-0"
                    style={{ backgroundColor: meta.accent }}
                  />
                  <span className="text-[11px] text-zinc-200 truncate">{meta.label}</span>
                </button>
              );
            })}
          </div>
          <p className="text-[11px] text-zinc-500 leading-relaxed">
            Presets rewrite the <code className="font-mono text-zinc-400">--accent</code> and surface
            custom properties on the document root. Dynamic mode samples the playing album art.
          </p>
        </Section>

        <p className="text-[10px] font-mono text-zinc-600 pb-2">
          {isTauriEnvironment()
            ? 'Connected to the Sonora Rust backend.'
            : 'Running in a browser: commands are inert and the library stays empty until you run the desktop app.'}
        </p>
      </div>
    </div>
  );
};
