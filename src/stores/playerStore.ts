import { create } from 'zustand';
import { tauriBridge } from '../services/tauriBridge';
import { errorMessage as message } from '../services/format';
import type {
  EngineTrack,
  MediaKeyAction,
  PlaybackStatus,
  RepeatMode,
  RomanizationMode,
  TrackRecord,
} from '../types';

/** Structurally identical to the unlisten function the bridge hands back. */
type Unsubscribe = () => void;

const identityOrder = (length: number): number[] => Array.from({ length }, (_, i) => i);

/** Fisher-Yates over everything but the track we are about to play, which stays first. */
const shuffledOrder = (length: number, first: number): number[] => {
  const rest = identityOrder(length).filter((i) => i !== first);
  for (let i = rest.length - 1; i > 0; i -= 1) {
    const j = Math.floor(Math.random() * (i + 1));
    [rest[i], rest[j]] = [rest[j], rest[i]];
  }
  return [first, ...rest];
};

let noticeTimer: ReturnType<typeof setTimeout> | null = null;
const showNotice = (text: string, ms = 7000): void => {
  if (noticeTimer) clearTimeout(noticeTimer);
  usePlayerStore.setState({ notice: text });
  noticeTimer = setTimeout(() => usePlayerStore.setState({ notice: null }), ms);
};

/**
 * Builds the payload the Rust engine expects.
 */
const engineTrackFor = async (track: TrackRecord): Promise<EngineTrack> => {
  if (track.provider === 'ytmusic') {
    const videoId = track.id.replace(/^ytmusic:\/\/track\//, '');
    const streamUrl = await tauriBridge.ytmusicResolveStream(videoId);
    return {
      id: track.id,
      title: track.title,
      artist: track.artist,
      filePath: null,
      streamUrl,
      durationMs: track.durationMs,
      loudnessLufs: track.loudnessLufs,
    };
  }
  if (track.provider === 'spotify') {
    const streamUrl = await tauriBridge.spotifyResolveStream(
      track.title,
      track.artist,
      track.durationMs,
    );
    return {
      id: track.id,
      title: track.title,
      artist: track.artist,
      filePath: null,
      streamUrl,
      durationMs: track.durationMs,
      loudnessLufs: track.loudnessLufs,
    };
  }
  return {
    id: track.id,
    title: track.title,
    artist: track.artist,
    filePath: track.filePath ?? null,
    streamUrl: null,
    durationMs: track.durationMs,
    loudnessLufs: track.loudnessLufs,
  };
};

/**
 * How Spotify tracks play: on the user's Spotify app over Connect, or through the YouTube matcher
 * when no Spotify device answers. Every non-gapless Spotify start retries Connect first.
 */
let spotifyRoute: 'connect' | 'youtube' = 'connect';

/** True for Spotify tracks that Spotify Connect, not Sonora's engine, is playing. */
const isSpotifyTrack = (track: TrackRecord | null | undefined): boolean =>
  track?.provider === 'spotify' && spotifyRoute === 'connect';

/** Monotonic token so a slow resolve can never load a track the user has already skipped past. */
let loadSequence = 0;

const upcomingQueueIndex = (state: PlayerStore): number => {
  if (state.order.length === 0) return -1;
  // Repeat-one pre-rolls the same track: that is what makes the loop gapless.
  if (state.repeatMode === 'track') return state.order[state.cursor] ?? -1;
  const next = state.cursor + 1;
  if (next < state.order.length) return state.order[next];
  if (state.repeatMode === 'queue') return state.order[0];
  return -1;
};

/**
 * Keeps the engine's pre-roll slot in sync with the queue. Runs on every queue, cursor, shuffle and
 * repeat change — not just on track change — because the engine can only splice gaplessly when it
 * already holds the next track.
 */
const syncPreRoll = async (): Promise<void> => {
  const state = usePlayerStore.getState();
  const upcoming = upcomingQueueIndex(state);
  const upNext = upcoming >= 0 ? state.queue[upcoming] ?? null : null;
  const localSeam = !isSpotifyTrack(state.currentTrack) && !isSpotifyTrack(upNext);
  usePlayerStore.setState({ nextTrack: upNext, isGapless: false });

  if (!localSeam) {
    void tauriBridge.setNextTrack(null).catch(() => {});
    return;
  }

  let engineTrack: EngineTrack | null = null;
  if (upNext) {
    try {
      engineTrack = await engineTrackFor(upNext);
    } catch {
      engineTrack = null;
    }
  }
  // A newer queue edit superseded this computation while the stream was resolving.
  const current = usePlayerStore.getState();
  if (
    current.queue !== state.queue ||
    current.order !== state.order ||
    current.cursor !== state.cursor ||
    current.currentTrack?.id !== state.currentTrack?.id
  ) {
    return;
  }
  try {
    await tauriBridge.setNextTrack(engineTrack);
  } catch {
    /* The engine is not accepting commands yet; the next queue change retries. */
  }
};

const startAtCursor = async (autoPlay: boolean): Promise<void> => {
  const state = usePlayerStore.getState();
  const track = state.queue[state.order[state.cursor]];
  if (!track) return;

  const sequence = ++loadSequence;
  usePlayerStore.setState({ isLoading: true });
  try {
    let viaConnect = false;
    if (track.provider === 'spotify') {
      try {
        await tauriBridge.spotifyPlay(track.id);
        viaConnect = true;
      } catch (error) {
        if (sequence !== loadSequence) return;
        showNotice(`Spotify app unavailable (${message(error)}). Playing “${track.title}” via YouTube.`);
      }
      spotifyRoute = viaConnect ? 'connect' : 'youtube';
    }
    if (viaConnect) {
      void tauriBridge.spotifySetVolume(state.volume).catch(() => {});
    } else {
      const engineTrack = await engineTrackFor(track);
      if (sequence !== loadSequence) return;
      await tauriBridge.loadTrack(engineTrack, autoPlay);
      if (sequence !== loadSequence) return;
      await tauriBridge.setVolume(usePlayerStore.getState().volume);
    }
    if (sequence !== loadSequence) return;
    const currentStatus = usePlayerStore.getState().status;
    const shouldPlay = currentStatus !== 'paused' && autoPlay;
    usePlayerStore.setState({
      status: shouldPlay ? 'playing' : 'paused',
      isLoading: false,
      isGapless: false,
      spotifyDeviceName: viaConnect ? usePlayerStore.getState().spotifyDeviceName ?? 'Spotify' : undefined,
    });
    if (!shouldPlay && autoPlay) {
      void (viaConnect ? tauriBridge.spotifyPause() : tauriBridge.pause()).catch(() => {});
    }
  } catch (error) {
    if (sequence !== loadSequence) return;
    showNotice(`Could not play “${track.title}”: ${message(error)}`);
    usePlayerStore.setState({ status: 'stopped', isLoading: false });
    return;
  }
  void syncPreRoll();
};

const handleTrackEnded = (endedTrackId: string, gapless: boolean): void => {
  const state = usePlayerStore.getState();
  if (state.order.length === 0) return;
  // A stale or duplicated event would otherwise advance the queue twice.
  if (state.currentTrack && state.currentTrack.id !== endedTrackId) return;

  if (state.repeatMode === 'track') {
    usePlayerStore.setState({ positionMs: 0, status: 'playing' });
    if (gapless) void syncPreRoll();
    else void startAtCursor(true);
    return;
  }

  const next = state.cursor + 1;
  const position = next < state.order.length ? next : state.repeatMode === 'queue' ? 0 : -1;
  if (position < 0) {
    if (isSpotifyTrack(state.currentTrack)) void tauriBridge.spotifyStop().catch(() => {});
    usePlayerStore.setState({ status: 'stopped', positionMs: 0 });
    return;
  }

  const track = state.queue[state.order[position]];
  if (!track) return;
  usePlayerStore.setState({
    cursor: position,
    currentTrack: track,
    positionMs: 0,
    durationMs: track.durationMs,
    status: 'playing',
  });
  if (gapless) void syncPreRoll();
  else void startAtCursor(true);
};

const handleMediaKey = (action: MediaKeyAction): void => {
  const state = usePlayerStore.getState();
  if (action === 'play_pause') void state.playPause();
  else if (action === 'next') state.next();
  else state.previous();
};

interface PlayerStore {
  /** Mixed provider entries. Spotify rows play on the Spotify app over Connect, else via YouTube. */
  queue: TrackRecord[];
  /** Play order as indices into `queue`; the identity permutation when shuffle is off. */
  order: number[];
  cursor: number;
  currentTrack: TrackRecord | null;
  nextTrack: TrackRecord | null;
  shuffle: boolean;
  repeatMode: RepeatMode;

  status: PlaybackStatus;
  isLoading: boolean;
  positionMs: number;
  durationMs: number;
  volume: number;
  isMuted: boolean;
  volumeBeforeMute: number;
  isNormalizing: boolean;
  isGapless: boolean;
  spotifyDeviceName?: string;
  isLyricsOpen: boolean;
  isQueueOpen: boolean;
  romanizationMode: RomanizationMode;
  notice: string | null;

  playFromList: (tracks: TrackRecord[], startIndex?: number) => void;
  playPause: () => void;
  next: () => void;
  previous: () => void;
  /** The only `playback_seek` the engine sees: the scrubber calls it once, on release. */
  seek: (positionMs: number) => void;
  setVolume: (volume: number) => void;
  toggleMute: () => void;
  toggleShuffle: () => void;
  cycleRepeat: () => void;
  toggleNormalization: () => void;
  toggleLyrics: (open?: boolean) => void;
  toggleQueue: (open?: boolean) => void;
  setRomanizationMode: (mode: RomanizationMode) => void;
  jumpTo: (queueIndex: number) => void;
  removeFromQueue: (queueIndex: number) => void;
  moveInQueue: (orderPosition: number, direction: -1 | 1) => void;
  setNotice: (notice: string | null) => void;
  /** Subscribes to engine events; returns the teardown function. Safe to call twice. */
  bindEngineEvents: () => Unsubscribe;
}

export const usePlayerStore = create<PlayerStore>((set, get) => ({
  queue: [],
  order: [],
  cursor: 0,
  currentTrack: null,
  nextTrack: null,
  shuffle: false,
  repeatMode: 'off',

  status: 'stopped',
  isLoading: false,
  positionMs: 0,
  durationMs: 0,
  volume: 0.85,
  isMuted: false,
  volumeBeforeMute: 0.85,
  isNormalizing: true,
  isGapless: false,
  spotifyDeviceName: undefined,
  isLyricsOpen: false,
  isQueueOpen: false,
  romanizationMode: 'dual',
  notice: null,

  playFromList: (tracks, startIndex = 0) => {
    const clicked = tracks[startIndex];
    if (!clicked) return;
    const start = Math.max(
      0,
      tracks.findIndex((track) => track.id === clicked.id),
    );
    const order = get().shuffle ? shuffledOrder(tracks.length, start) : identityOrder(tracks.length);
    const currentTrack = tracks[start];

    set({
      queue: tracks,
      order,
      cursor: order.indexOf(start),
      currentTrack,
      positionMs: 0,
      durationMs: currentTrack.durationMs,
      status: 'playing',
      isLoading: true,
      isGapless: false,
      spotifyDeviceName: undefined,
    });
    void startAtCursor(true);
  },

  playPause: () => {
    const { currentTrack, status, isLoading } = get();
    if (!currentTrack) return;

    if (isLoading) {
      if (status === 'playing') {
        set({ status: 'paused' });
        void (isSpotifyTrack(currentTrack) ? tauriBridge.spotifyPause() : tauriBridge.pause()).catch(
          (error) => showNotice(message(error)),
        );
      } else {
        set({ status: 'playing' });
        void (isSpotifyTrack(currentTrack) ? tauriBridge.spotifyResume() : tauriBridge.play()).catch(
          (error) => showNotice(message(error)),
        );
      }
      return;
    }

    if (status === 'playing') {
      set({ status: 'paused' });
      void (isSpotifyTrack(currentTrack) ? tauriBridge.spotifyPause() : tauriBridge.pause()).catch(
        (error) => showNotice(message(error)),
      );
      return;
    }
    if (status === 'paused') {
      set({ status: 'playing' });
      void (isSpotifyTrack(currentTrack) ? tauriBridge.spotifyResume() : tauriBridge.play()).catch(
        (error) => showNotice(message(error)),
      );
      return;
    }
    // The engine stopped (queue drained, or nothing loaded yet): reload from the top.
    void startAtCursor(true);
  },

  next: () => {
    const state = get();
    if (state.order.length === 0) return;
    const position =
      state.cursor + 1 < state.order.length
        ? state.cursor + 1
        : state.repeatMode === 'queue'
          ? 0
          : -1;
    if (position < 0) {
      void (isSpotifyTrack(state.currentTrack) ? tauriBridge.spotifyStop() : tauriBridge.stop()).catch(
        (error) => showNotice(message(error)),
      );
      set({ status: 'stopped', positionMs: 0, nextTrack: null, isGapless: false });
      return;
    }
    const track = state.queue[state.order[position]];
    if (!track) return;
    set({
      cursor: position,
      currentTrack: track,
      positionMs: 0,
      durationMs: track.durationMs,
      status: 'playing',
      isLoading: true,
      isGapless: false,
    });
    void startAtCursor(true);
  },

  previous: () => {
    const state = get();
    if (!state.currentTrack) return;
    // Past the three-second mark "previous" restarts the track, like every other player.
    if (state.positionMs > 3000 || state.cursor === 0) {
      set({ positionMs: 0 });
      void (isSpotifyTrack(state.currentTrack) ? tauriBridge.spotifySeek(0) : tauriBridge.seek(0)).catch(
        (error) => showNotice(message(error)),
      );
      return;
    }
    const position = state.cursor - 1;
    const track = state.queue[state.order[position]];
    if (!track) return;
    set({
      cursor: position,
      currentTrack: track,
      positionMs: 0,
      durationMs: track.durationMs,
      status: 'playing',
      isLoading: true,
      isGapless: false,
    });
    void startAtCursor(true);
  },

  seek: (positionMs) => {
    set({ positionMs });
    const target = Math.max(0, Math.round(positionMs));
    void (isSpotifyTrack(get().currentTrack) ? tauriBridge.spotifySeek(target) : tauriBridge.seek(target)).catch(
      (error) => showNotice(message(error)),
    );
  },

  setVolume: (volume) => {
    const clamped = Math.max(0, Math.min(1, volume));
    const state = get();
    const beforeMute = clamped > 0 ? clamped : state.volumeBeforeMute || 0.85;
    set({ volume: clamped, isMuted: clamped === 0, volumeBeforeMute: beforeMute });
    const update = isSpotifyTrack(state.currentTrack)
      ? tauriBridge.spotifySetVolume(clamped)
      : tauriBridge.setVolume(clamped);
    void update.catch((error) => showNotice(message(error)));
  },

  toggleMute: () => {
    const state = get();
    get().setVolume(state.isMuted || state.volume === 0 ? state.volumeBeforeMute || 0.85 : 0);
  },

  toggleShuffle: () => {
    const state = get();
    const shuffle = !state.shuffle;
    if (state.order.length === 0) {
      set({ shuffle });
      return;
    }
    // Re-plan the order around the track that is playing so it never changes mid-song.
    const currentIndex = state.order[state.cursor];
    const order = shuffle
      ? shuffledOrder(state.queue.length, currentIndex)
      : identityOrder(state.queue.length);
    set({ shuffle, order, cursor: Math.max(0, order.indexOf(currentIndex)) });
    void syncPreRoll();
  },

  cycleRepeat: () => {
    const next: RepeatMode =
      get().repeatMode === 'off' ? 'track' : get().repeatMode === 'track' ? 'queue' : 'off';
    set({ repeatMode: next });
    void syncPreRoll();
  },

  toggleNormalization: () => {
    if (isSpotifyTrack(get().currentTrack)) {
      showNotice('Spotify loudness is controlled by the Spotify app.');
      return;
    }
    const enabled = !get().isNormalizing;
    set({ isNormalizing: enabled });
    void tauriBridge
      .toggleNormalization(enabled)
      .then((confirmed) => set({ isNormalizing: confirmed }))
      .catch((error) => {
        set({ isNormalizing: !enabled });
        showNotice(`Normalization unavailable: ${message(error)}`);
      });
  },

  toggleLyrics: (open) => set((state) => ({ isLyricsOpen: open ?? !state.isLyricsOpen })),
  toggleQueue: (open) => set((state) => ({ isQueueOpen: open ?? !state.isQueueOpen })),
  setRomanizationMode: (romanizationMode) => set({ romanizationMode }),
  setNotice: (notice) => set({ notice }),

  jumpTo: (queueIndex) => {
    const state = get();
    const position = state.order.indexOf(queueIndex);
    const track = state.queue[queueIndex];
    if (position < 0 || !track) return;
    set({
      cursor: position,
      currentTrack: track,
      positionMs: 0,
      durationMs: track.durationMs,
      status: 'playing',
      isLoading: true,
      isGapless: false,
    });
    void startAtCursor(true);
  },

  removeFromQueue: (queueIndex) => {
    const state = get();
    const currentIndex = state.order[state.cursor];
    if (queueIndex === currentIndex) {
      showNotice('The playing track cannot be removed from the queue.');
      return;
    }
    const queue = state.queue.filter((_, index) => index !== queueIndex);
    const order = state.order
      .filter((index) => index !== queueIndex)
      .map((index) => (index > queueIndex ? index - 1 : index));
    const shifted = currentIndex > queueIndex ? currentIndex - 1 : currentIndex;
    set({ queue, order, cursor: Math.max(0, order.indexOf(shifted)) });
    void syncPreRoll();
  },

  moveInQueue: (orderPosition, direction) => {
    const state = get();
    const target = orderPosition + direction;
    if (target < 0 || target >= state.order.length) return;
    // Swapping the playing entry would silently change what is playing; the UI disables it too.
    if (orderPosition === state.cursor || target === state.cursor) return;
    const order = [...state.order];
    [order[orderPosition], order[target]] = [order[target], order[orderPosition]];
    set({ order });
    void syncPreRoll();
  },

  bindEngineEvents: () => {
    if (started) return unsubscribe ?? (() => {});

    started = true;
    const generation = ++subscriptionGeneration;
    const attach = async (pending: Promise<Unsubscribe>) => {
      const off = await pending;
      if (generation === subscriptionGeneration) subscriptions.push(off);
      else off();
    };

    void attach(
      tauriBridge.onPlaybackProgress((progress) => {
        if (isSpotifyTrack(get().currentTrack)) return;
        set({
          positionMs: progress.positionMs,
          durationMs: progress.durationMs > 0 ? progress.durationMs : get().durationMs,
        });
      }),
    );
    void attach(
      tauriBridge.onPlaybackStatus((engine) => {
        if (isSpotifyTrack(get().currentTrack)) return;
        if (get().isLoading) return;
        set({
          status: engine.status,
          positionMs: engine.positionMs,
          durationMs: engine.durationMs > 0 ? engine.durationMs : get().durationMs,
          isNormalizing: engine.isNormalizing,
          isGapless: engine.isGapless,
        });
      }),
    );
    void attach(
      tauriBridge.onSpotifyPlaybackState((spotify) => {
        const current = get().currentTrack;
        if (!isSpotifyTrack(current) || (spotify.track && spotify.track.id !== current?.id)) return;
        if (get().isLoading && !spotify.isPlaying) return;
        set({
          status: spotify.isPlaying ? 'playing' : 'paused',
          isLoading: false,
          positionMs: spotify.progressMs,
          durationMs: spotify.durationMs > 0 ? spotify.durationMs : get().durationMs,
          spotifyDeviceName: spotify.deviceName ?? 'Spotify',
        });
      }),
    );
    void attach(tauriBridge.onTrackEnded(({ trackId, gapless }) => handleTrackEnded(trackId, gapless)));
    void attach(tauriBridge.onMediaKey(({ action }) => handleMediaKey(action)));

    // A freshly started engine sits at its own defaults, so push what the UI is already showing.
    void tauriBridge.setVolume(get().volume).catch(() => {});
    void tauriBridge.toggleNormalization(get().isNormalizing).catch(() => {});

    unsubscribe = () => {
      started = false;
      // Bump the generation so listeners still being registered tear themselves down.
      subscriptionGeneration += 1;
      subscriptions.forEach((off) => off());
      subscriptions = [];
      unsubscribe = null;
    };
    return unsubscribe;
  },
}));

let started = false;
let subscriptionGeneration = 0;
let subscriptions: Unsubscribe[] = [];
let unsubscribe: Unsubscribe | null = null;
