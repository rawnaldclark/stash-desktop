// The window's only way to the library: the core's local API. Served by the core itself, the API is on this page's
// own origin; the desktop shell hands the core's address in before the page runs (`window.__STASH_CORE__`).

export type TrackRow = {
  id: number;
  title: string;
  artist: string;
  album: string | null;
  duration_ms: number;
  codec: string | null;
  sample_rate: number | null;
  bit_depth: number | null;
  origin: "file" | "addon";
  sort_key: string;
};

export type Counts = { tracks: number; albums: number; artists: number };

type Shell = { __STASH_CORE__?: string; __STASH_BENCH__?: boolean };
const shell = window as unknown as Shell;
const params = new URLSearchParams(location.search);

const core = shell.__STASH_CORE__ ?? params.get("core") ?? "";

/** The page was asked to measure itself: `?bench`, or the shell said so. */
export const measuring = shell.__STASH_BENCH__ === true || params.has("bench");

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${core}${path}`, { signal });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const counts = () => get<Counts>("/api/counts");

export const tracksAt = (offset: number, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/tracks?offset=${offset}&limit=${limit}`, signal).then((r) => r.rows);

export const search = (query: string, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal).then((r) => r.rows);

export type Format = { sample_rate: number | null; format: string | null };

/** What the engine reports about the sound between the file and the sound card. */
export type Signal = {
  codec: string | null;
  decoded: Format;
  output: Format;
  driver: string | null;
  mode: string | null;
  resampled: boolean;
  converted: boolean;
};

export type PlayerState = {
  /** False when the core has no audio engine; `reason` says why. */
  available: boolean;
  reason?: string;
  status: "idle" | "playing" | "paused";
  index: number | null;
  queue_len: number;
  track: { id: number; title: string; artist: string; album: string | null } | null;
  position_ms: number;
  duration_ms: number;
  signal: Signal | null;
  error: string | null;
};

/** Nothing playing. */
export const IDLE: PlayerState = {
  available: true,
  status: "idle",
  index: null,
  queue_len: 0,
  track: null,
  position_ms: 0,
  duration_ms: 0,
  signal: null,
  error: null,
};

/** The core's answer with every field present (a core without a player sends only a few). */
const whole = (answer: Partial<PlayerState>): PlayerState => ({ ...IDLE, ...answer });

async function post<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(`${core}${path}`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const player = () => get<Partial<PlayerState>>("/api/player").then(whole);
/** Plays the library's rows from `offset` on, in the list's order. */
export const playFrom = (offset: number) => post<Partial<PlayerState>>("/api/player/play", { offset }).then(whole);
/** Plays these tracks, in this order. */
export const playTracks = (ids: number[]) => post<Partial<PlayerState>>("/api/player/play", { ids }).then(whole);
export const setPaused = (paused: boolean) => post<Partial<PlayerState>>("/api/player/pause", { paused }).then(whole);
export const nextTrack = () => post<Partial<PlayerState>>("/api/player/next", {}).then(whole);
export const previousTrack = () => post<Partial<PlayerState>>("/api/player/previous", {}).then(whole);
export const seekTo = (seconds: number) => post<Partial<PlayerState>>("/api/player/seek", { seconds }).then(whole);

export async function postMeasurement(result: unknown): Promise<void> {
  await fetch(`${core}/api/bench-result`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(result) });
}
