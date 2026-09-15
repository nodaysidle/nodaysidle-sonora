import { create } from 'zustand';
import { ThemePreset } from '../types';
import { artworkSrc } from '../services/format';

interface ThemeVars {
  accent: string;
  background: string;
  surface: string;
  surfaceHover: string;
  border: string;
}

/**
 * Each preset rewrites the CSS custom properties declared in `index.css`, which every glass surface
 * and accent helper reads from, so a preset change is a live re-skin with no re-render.
 */
const PRESETS: Record<Exclude<ThemePreset, 'dynamic'>, ThemeVars> = {
  cybervolt: {
    accent: '#c8ff00',
    background: 'rgba(10, 10, 15, 0.85)',
    surface: 'rgba(22, 22, 34, 0.7)',
    surfaceHover: 'rgba(35, 35, 52, 0.8)',
    border: 'rgba(255, 255, 255, 0.08)',
  },
  obsidian: {
    accent: '#d4d4d8',
    background: 'rgba(12, 12, 14, 0.88)',
    surface: 'rgba(24, 24, 27, 0.72)',
    surfaceHover: 'rgba(39, 39, 42, 0.82)',
    border: 'rgba(255, 255, 255, 0.06)',
  },
  midnight: {
    accent: '#7dd3fc',
    background: 'rgba(8, 12, 24, 0.86)',
    surface: 'rgba(18, 26, 45, 0.72)',
    surfaceHover: 'rgba(30, 41, 66, 0.82)',
    border: 'rgba(125, 211, 252, 0.14)',
  },
  'rose-pine': {
    accent: '#ebbcba',
    background: 'rgba(25, 15, 22, 0.86)',
    surface: 'rgba(38, 25, 34, 0.72)',
    surfaceHover: 'rgba(55, 37, 48, 0.82)',
    border: 'rgba(235, 188, 186, 0.14)',
  },
  nord: {
    accent: '#88c0d0',
    background: 'rgba(13, 19, 26, 0.86)',
    surface: 'rgba(23, 32, 43, 0.72)',
    surfaceHover: 'rgba(36, 48, 62, 0.82)',
    border: 'rgba(136, 192, 208, 0.14)',
  },
  amoled: {
    accent: '#c8ff00',
    background: 'rgba(0, 0, 0, 0.94)',
    surface: 'rgba(8, 8, 8, 0.86)',
    surfaceHover: 'rgba(20, 20, 20, 0.9)',
    border: 'rgba(255, 255, 255, 0.09)',
  },
};

const DEFAULT_VARS = PRESETS.cybervolt;

/** Swatch metadata for the settings picker; `dynamic` previews with the live accent. */
export const THEME_PRESET_META: Record<ThemePreset, { label: string; accent: string }> = {
  cybervolt: { label: 'Cybervolt', accent: PRESETS.cybervolt.accent },
  obsidian: { label: 'Obsidian', accent: PRESETS.obsidian.accent },
  midnight: { label: 'Midnight', accent: PRESETS.midnight.accent },
  'rose-pine': { label: 'Rosé Pine', accent: PRESETS['rose-pine'].accent },
  nord: { label: 'Nord', accent: PRESETS.nord.accent },
  amoled: { label: 'AMOLED', accent: PRESETS.amoled.accent },
  dynamic: { label: 'Dynamic (album art)', accent: DEFAULT_VARS.accent },
};

const applyVars = (vars: ThemeVars) => {
  if (typeof document === 'undefined') return;
  const root = document.documentElement.style;
  root.setProperty('--accent', vars.accent);
  root.setProperty('--background', vars.background);
  root.setProperty('--surface', vars.surface);
  root.setProperty('--surface-hover', vars.surfaceHover);
  root.setProperty('--border', vars.border);
};

/**
 * Pulls a usable accent out of album art: the most colourful pixel wins, then its hue is pushed to
 * full saturation so the result reads on a near-black surface.
 *
 * ponytail: reads the canvas back, so a cross-origin image taints it and we fall back to the
 * preset accent. Serve artwork with CORS headers (or decode in Rust) if per-album tinting matters.
 */
const extractAccent = (src: string): Promise<string | null> =>
  new Promise((resolve) => {
    const image = new Image();
    image.crossOrigin = 'anonymous';
    image.onload = () => {
      try {
        const size = 32;
        const canvas = document.createElement('canvas');
        canvas.width = size;
        canvas.height = size;
        const ctx = canvas.getContext('2d');
        if (!ctx) return resolve(null);
        ctx.drawImage(image, 0, 0, size, size);
        const { data } = ctx.getImageData(0, 0, size, size);

        let bestScore = 0;
        let bestHue = -1;
        for (let i = 0; i < data.length; i += 4) {
          if (data[i + 3] < 128) continue;
          const r = data[i] / 255;
          const g = data[i + 1] / 255;
          const b = data[i + 2] / 255;
          const max = Math.max(r, g, b);
          const min = Math.min(r, g, b);
          const lightness = (max + min) / 2;
          if (lightness < 0.18 || lightness > 0.92) continue;
          const saturation = max === min ? 0 : (max - min) / (1 - Math.abs(2 * lightness - 1));
          const score = saturation * (1 - Math.abs(lightness - 0.5) * 1.4);
          if (score <= bestScore) continue;
          bestScore = score;
          if (max === r) bestHue = ((g - b) / (max - min) + 6) % 6;
          else if (max === g) bestHue = (b - r) / (max - min) + 2;
          else bestHue = (r - g) / (max - min) + 4;
        }
        if (bestHue < 0 || bestScore < 0.15) return resolve(null);
        resolve(`hsl(${Math.round(bestHue * 60)} 100% 62%)`);
      } catch {
        resolve(null);
      }
    };
    image.onerror = () => resolve(null);
    image.src = src;
  });

interface ThemeStore {
  activeTheme: ThemePreset;
  /** The accent currently applied to the DOM — static for presets, sampled art for `dynamic`. */
  accentColor: string;
  setTheme: (theme: ThemePreset) => void;
  setAccentColor: (color: string) => void;
  /** Re-tints the `dynamic` preset from the playing track's artwork. No-op for static presets. */
  syncFromArtwork: (artworkUrl?: string | null) => Promise<void>;
}

export const useThemeStore = create<ThemeStore>((set, get) => ({
  activeTheme: 'cybervolt',
  accentColor: DEFAULT_VARS.accent,

  setTheme: (activeTheme) => {
    const vars = activeTheme === 'dynamic' ? null : PRESETS[activeTheme];
    set({ activeTheme, accentColor: vars ? vars.accent : DEFAULT_VARS.accent });
    applyVars(vars ?? DEFAULT_VARS);
    if (activeTheme === 'dynamic') void get().syncFromArtwork();
  },

  setAccentColor: (accentColor) => {
    set({ accentColor });
    if (typeof document !== 'undefined') {
      document.documentElement.style.setProperty('--accent', accentColor);
    }
  },

  syncFromArtwork: async (artworkUrl) => {
    if (get().activeTheme !== 'dynamic') return;
    const src = artworkSrc(artworkUrl);
    if (!src) {
      set({ accentColor: DEFAULT_VARS.accent });
      applyVars(DEFAULT_VARS);
      return;
    }
    const accent = (await extractAccent(src)) ?? DEFAULT_VARS.accent;
    if (get().activeTheme !== 'dynamic') return;
    set({ accentColor: accent });
    applyVars({ ...DEFAULT_VARS, accent });
  },
}));

/** Applies the persisted default on startup so the DOM matches the store from the first paint. */
export const applyActiveTheme = () => {
  const { activeTheme } = useThemeStore.getState();
  applyVars(activeTheme === 'dynamic' ? DEFAULT_VARS : PRESETS[activeTheme]);
};
