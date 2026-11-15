// The window's only way to the library, the player and the add-ons: the core's local API. Served by the core
// itself, the API is on this page's own origin; the desktop shell hands the core's address in before the page runs
// (`window.__STASH_CORE__`).

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

/** A request the core (or an add-on behind it) refused, in words for the listener. */
export class Refused extends Error {}

async function answer<T>(path: string, response: Response): Promise<T> {
  if (response.ok) return (await response.json()) as T;
  const body = (await response.json().catch(() => null)) as { error?: { message?: string } } | null;
  throw new Refused(body?.error?.message || `${path}: ${response.status}`);
}

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  return answer<T>(path, await fetch(`${core}${path}`, { signal }));
}

async function post<T>(path: string, body: unknown): Promise<T> {
  const sent = { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  return answer<T>(path, await fetch(`${core}${path}`, sent));
}

export const counts = () => get<Counts>("/api/counts");

export const tracksAt = (offset: number, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/tracks?offset=${offset}&limit=${limit}`, signal).then((r) => r.rows);

export const search = (query: string, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal).then((r) => r.rows);

// ---- Add-ons

export type Tier = "low" | "high" | "lossless" | "hires";

/** An add-on the listener added. Its link is kept by the core and not shown: a link can carry a key. */
export type AddonInfo = {
  id: string;
  name: string;
  version: string | null;
  description: string | null;
  /** The server it is on. */
  host: string | null;
  resources: string[];
  enabled: boolean;
  /** Why it cannot be used, when it cannot. */
  problem: string | null;
};

/** A track an add-on found. */
export type FoundTrack = {
  id: string;
  title: string;
  artist: string;
  album: string | null;
  duration_ms: number | null;
  max_tier: Tier | null;
  explicit: boolean | null;
};

export type AddonGroup = {
  addon: { id: string; name: string };
  tracks: FoundTrack[];
  error: { code: string; message: string; retry_after: number | null } | null;
};

type Listed = { addons: AddonInfo[] };

export const addons = () => get<Listed>("/api/addons").then((r) => r.addons);
/** Adds the add-on at `link` (the address of its manifest). */
export const addAddon = (link: string) => post<Listed>("/api/addons", { link }).then((r) => r.addons);
export const removeAddon = (id: string) => post<Listed>("/api/addons/remove", { id }).then((r) => r.addons);
export const setAddonEnabled = (id: string, enabled: boolean) =>
  post<Listed>("/api/addons/enabled", { id, enabled }).then((r) => r.addons);
export const orderAddons = (ids: string[]) => post<Listed>("/api/addons/order", { ids }).then((r) => r.addons);

/** Asks one add-on for tracks. */
export const searchAddon = (id: string, query: string, limit: number, signal?: AbortSignal) =>
  get<{ groups: AddonGroup[] }>(
    `/api/addons/search?q=${encodeURIComponent(query)}&addon=${encodeURIComponent(id)}&limit=${limit}`,
    signal,
  ).then((r) => r.groups[0] ?? null);

// ---- The player

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
  /** "loading": an add-on is being asked where the track plays from. */
  status: "idle" | "loading" | "playing" | "paused";
  index: number | null;
  queue_len: number;
  track: {
    /** The library's row, for a track of the library. */
    id: number | null;
    title: string;
    artist: string;
    album: string | null;
    /** Set for a track an add-on serves. */
    addon: { addon: string; track: string } | null;
  } | null;
  position_ms: number;
  duration_ms: number;
  /** What the add-on said the stream is. */
  tier: Tier | null;
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
  tier: null,
  signal: null,
  error: null,
};

/** The core's answer with every field present (a core without a player sends only a few). */
const whole = (answer: Partial<PlayerState>): PlayerState => ({ ...IDLE, ...answer });
const told = (path: string, body: unknown) => post<Partial<PlayerState>>(path, body).then(whole);

export const player = () => get<Partial<PlayerState>>("/api/player").then(whole);
/** Plays the library's rows from `offset` on, in the list's order. */
export const playFrom = (offset: number) => told("/api/player/play", { offset });
/** Plays these library tracks, in this order. */
export const playTracks = (ids: number[]) => told("/api/player/play", { ids });
/** Plays these tracks an add-on found, in this order. */
export const playFound = (addon: string, tracks: FoundTrack[]) =>
  told("/api/player/play", { addon, tracks: tracks.map(({ id, title, artist, album }) => ({ id, title, artist, album })) });
export const setPaused = (paused: boolean) => told("/api/player/pause", { paused });
export const nextTrack = () => told("/api/player/next", {});
export const previousTrack = () => told("/api/player/previous", {});
export const seekTo = (seconds: number) => told("/api/player/seek", { seconds });

export async function postMeasurement(result: unknown): Promise<void> {
  await fetch(`${core}/api/bench-result`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(result) });
}
