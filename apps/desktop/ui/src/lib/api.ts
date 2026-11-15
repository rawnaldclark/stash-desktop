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
  /** "described": a song known by its description (imported); it plays through the add-ons. */
  origin: "file" | "addon" | "described";
  sort_key: string;
  liked: boolean;
};

export type Counts = { tracks: number; albums: number; artists: number; liked: number; playlists: number };

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

// ---- The listener's lists, and bringing a library in from another Stash

export type PlaylistRow = { id: number; name: string; tracks: number; followed: boolean };

export const likedAt = (offset: number, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/liked?offset=${offset}&limit=${limit}`, signal).then((r) => r.rows);

export const playlists = () => get<{ rows: PlaylistRow[] }>("/api/playlists").then((r) => r.rows);

export const playlistTracksAt = (id: number, offset: number, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/playlists/${id}/tracks?offset=${offset}&limit=${limit}`, signal).then((r) => r.rows);

/** What an import changed. */
export type Imported = {
  source: string;
  /** Who wrote the file, in the file's own words. */
  generator: string | null;
  summary: {
    songs_added: number;
    songs_already: number;
    likes_added: number;
    likes_already: number;
    playlists_added: number;
    playlists_merged: number;
    playlist_songs_added: number;
    plays_added: number;
    plays_already: number;
  };
  counts: Counts;
};

/** Hands the core a library file from the Android app or the web player. The file goes as it is: the core reads it. */
export async function importLibrary(file: Blob): Promise<Imported> {
  const sent = { method: "POST", headers: { "Content-Type": "application/octet-stream" }, body: file };
  return answer<Imported>("/api/import", await fetch(`${core}/api/import`, sent));
}

/** The core's answer to a change the listener made: the playlists and the counts as they now are. */
export type Changed = { playlists: PlaylistRow[]; counts: Counts; id?: number; liked?: boolean; added?: number; done?: boolean };

/** Likes a song of the library, or takes the like back. */
export const likeTrack = (id: number, liked: boolean) => post<Changed>("/api/tracks/like", { id, liked });
/** Makes a track an add-on found a song of the library; the answer's `id` is its row. */
export const keepFound = (addon: string, track: FoundTrack) => post<Changed>("/api/tracks/keep", { addon, track });
export const createPlaylist = (name: string) => post<Changed>("/api/playlists", { name });
export const renamePlaylist = (id: number, name: string) => post<Changed>(`/api/playlists/${id}/rename`, { name });
export const deletePlaylist = (id: number) => post<Changed>(`/api/playlists/${id}/delete`, {});
export const addToPlaylist = (id: number, ids: number[]) => post<Changed>(`/api/playlists/${id}/add`, { ids });
/** Takes the song at place `index` (0 is the top) out of a playlist. */
export const removeFromPlaylist = (id: number, index: number) => post<Changed>(`/api/playlists/${id}/remove`, { index });
export const moveInPlaylist = (id: number, from: number, to: number) => post<Changed>(`/api/playlists/${id}/move`, { from, to });

/** The library as the file the Android app and the web player import, and how much is in it. */
export async function exportLibrary(): Promise<{ blob: Blob; likes: number; lists: number; plays: number }> {
  const response = await fetch(`${core}/api/export`);
  if (!response.ok) throw new Refused(`/api/export: ${response.status}`);
  const text = await response.text();
  const file = JSON.parse(text) as { likes?: unknown[]; playlists?: unknown[]; history?: unknown[] };
  return {
    blob: new Blob([text], { type: "application/json" }),
    likes: file.likes?.length ?? 0,
    lists: file.playlists?.length ?? 0,
    plays: file.history?.length ?? 0,
  };
}

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
  /** The id of the add-on the stream comes from. */
  served_by: string | null;
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
  served_by: null,
  signal: null,
  error: null,
};

/** The core's answer with every field present (a core without a player sends only a few). */
const whole = (answer: Partial<PlayerState>): PlayerState => ({ ...IDLE, ...answer });
const told = (path: string, body: unknown) => post<Partial<PlayerState>>(path, body).then(whole);

export const player = () => get<Partial<PlayerState>>("/api/player").then(whole);
/** Plays the library's rows from `offset` on, in the list's order. */
export const playFrom = (offset: number) => told("/api/player/play", { offset });
/** Plays the liked songs from `offset` on, the newest like first. */
export const playLiked = (offset: number) => told("/api/player/play", { liked: true, offset });
/** Plays a playlist from `offset` on. */
export const playPlaylist = (playlist: number, offset: number) => told("/api/player/play", { playlist, offset });
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
