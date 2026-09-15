/** Shared display helpers. No React, no Tauri imports — the bridge owns those. */
import { toAssetUrl } from './tauriBridge';

/** `3:45`, or `1:02:07` for anything past an hour. */
export const formatDuration = (ms: number): string => {
  if (!Number.isFinite(ms) || ms <= 0) return '0:00';
  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
};

/** `1 hr 12 min` — for album and folder totals, where seconds are noise. */
export const formatLongDuration = (ms: number): string => {
  const totalMinutes = Math.round(ms / 60000);
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) return `${minutes} min`;
  return minutes === 0 ? `${hours} hr` : `${hours} hr ${minutes} min`;
};

export const pluralize = (count: number, singular: string, plural = `${singular}s`): string =>
  `${count.toLocaleString()} ${count === 1 ? singular : plural}`;

/** Tauri rejects with a plain string; `Error` covers everything else. */
export const errorMessage = (error: unknown): string =>
  typeof error === 'string' ? error : error instanceof Error ? error.message : String(error);

/**
 * Local artwork is stored as an absolute filesystem path and must go through the asset protocol;
 * remote artwork is already a URL. Returns `''` when there is nothing renderable, which callers
 * show as an empty artwork tile rather than a broken image.
 */
export const artworkSrc = (url?: string | null): string => {
  if (!url) return '';
  if (/^(https?:|data:|blob:|asset:|tauri:)/i.test(url)) return url;
  return toAssetUrl(url);
};
