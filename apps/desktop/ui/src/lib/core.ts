/**
 * The window's only way to the library, the player and the add-ons: the desktop core's local HTTP API, typed. Served by the core
 * itself, the API is on this page's own origin; the desktop shell hands the core's address in before the page runs
 * (`window.__STASH_CORE__`); `?core=` names it by hand (the dev server).
 *
 * `app.ts` puts the web player's UI boundary on top of this. The desktop's own screens (the whole-library list, albums, artists,
 * music folders, import and export) read their pages from here directly.
 */

type Shell = { __STASH_CORE__?: string; __STASH_BENCH__?: boolean };
const shell: Shell = typeof window === 'undefined' ? {} : (window as unknown as Shell);
const params = new URLSearchParams(typeof location === 'undefined' ? '' : location.search);

/** The core's address ('' when the core serves this page). */
export const coreUrl = shell.__STASH_CORE__ ?? params.get('core') ?? '';

/** The page was asked to measure itself: `?bench`, or the shell said so. */
export const measuring = shell.__STASH_BENCH__ === true || params.has('bench');

/** A request the core (or an add-on behind it) refused, in words for the listener. */
export class Refused extends Error {
	/** The HTTP status the core answered with. */
	readonly status: number;
	constructor(message: string, status: number) {
		super(message);
		this.name = 'Refused';
		this.status = status;
	}
}

/** What went wrong, in words for the listener: the core's own words, or that it is not answering. */
export const said = (e: unknown): string => (e instanceof Refused ? e.message : 'The Stash core is not answering.');

async function answer<T>(path: string, response: Response): Promise<T> {
	if (response.ok) return (await response.json()) as T;
	const body = (await response.json().catch(() => null)) as { error?: { message?: string } } | null;
	throw new Refused(body?.error?.message || `${path}: ${response.status}`, response.status);
}

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
	return answer<T>(path, await fetch(`${coreUrl}${path}`, { signal }));
}

async function post<T>(path: string, body: unknown): Promise<T> {
	const sent = { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) };
	return answer<T>(path, await fetch(`${coreUrl}${path}`, sent));
}

/** A picture the core names (`art`): an address of its own (`/api/…`) is asked of the core; anything else is used as it is. */
export const artUrl = (art: string | null | undefined): string | undefined => (!art ? undefined : art.startsWith('/') ? `${coreUrl}${art}` : art);

// ---- The library

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
	origin: 'file' | 'addon' | 'described';
	sort_key: string;
	liked: boolean;
	art: string | null;
	album_id: number | null;
	artist_id: number;
	added_at_ms: number;
	isrc: string | null;
};
export type LikedRow = TrackRow & { liked_at_ms: number };
export type PlayedRow = TrackRow & { played_at_ms: number };

export type Counts = { tracks: number; albums: number; artists: number; liked: number; playlists: number };

export type PlaylistRow = { id: number; name: string; tracks: number; followed: boolean; external_id: string | null; created_at_ms: number; updated_at_ms: number };

export type AlbumRow = { id: number; title: string; artist: string; artist_id: number; year: number | null; tracks: number; art: string | null };
export type ArtistRow = { id: number; name: string; tracks: number; albums: number; art: string | null };

type Rows<T> = { rows: T[] };
const rows = <T>(path: string, signal?: AbortSignal) => get<Rows<T>>(path, signal).then((r) => r.rows);

export const counts = () => get<Counts>('/api/counts');
/** The whole library in title order, a page at a time (`limit` at most 500). */
export const tracksAt = (offset: number, limit: number, signal?: AbortSignal) => rows<TrackRow>(`/api/tracks?offset=${offset}&limit=${limit}`, signal);
export const searchTracks = (query: string, limit: number, signal?: AbortSignal) => rows<TrackRow>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal);
/** What the library has for the text: songs (at most `limit`), and the albums and artists it names (a dozen of each). */
export const searchLibrary = (query: string, limit: number, signal?: AbortSignal) =>
	get<{ rows: TrackRow[]; albums: AlbumRow[]; artists: ArtistRow[] }>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal);
/** The liked songs, the newest like first. */
export const likedAt = (offset: number, limit: number, signal?: AbortSignal) => rows<LikedRow>(`/api/liked?offset=${offset}&limit=${limit}`, signal);
/** What was played, the newest first. */
export const historyAt = (offset: number, limit: number, signal?: AbortSignal) => rows<PlayedRow>(`/api/history?offset=${offset}&limit=${limit}`, signal);
export const playlists = () => rows<PlaylistRow>('/api/playlists');
export const playlistTracksAt = (id: number, offset: number, limit: number, signal?: AbortSignal) =>
	rows<TrackRow>(`/api/playlists/${id}/tracks?offset=${offset}&limit=${limit}`, signal);
export const albumsAt = (offset: number, limit: number, signal?: AbortSignal) => rows<AlbumRow>(`/api/albums?offset=${offset}&limit=${limit}`, signal);
export const album = (id: number) => get<{ album: AlbumRow; rows: TrackRow[] }>(`/api/albums/${id}`);
export const artistsAt = (offset: number, limit: number, signal?: AbortSignal) => rows<ArtistRow>(`/api/artists?offset=${offset}&limit=${limit}`, signal);
export const artist = (id: number) => get<{ artist: ArtistRow; rows: TrackRow[]; albums: AlbumRow[] }>(`/api/artists/${id}`);

/** The core's answer to a change the listener made: the playlists and the counts as they now are. */
export type Changed = { playlists: PlaylistRow[]; counts: Counts; id?: number; liked?: boolean; added?: number; done?: boolean };

/** Likes a song of the library, or takes the like back. */
export const likeTrack = (id: number, liked: boolean) => post<Changed>('/api/tracks/like', { id, liked });
/** Makes a track an add-on found a song of the library; the answer's `id` is its row. */
export const keepFound = (addon: string, track: KeptTrack) => post<Changed>('/api/tracks/keep', { addon, track });
export const createPlaylist = (name: string) => post<Changed>('/api/playlists', { name });
export const renamePlaylist = (id: number, name: string) => post<Changed>(`/api/playlists/${id}/rename`, { name });
export const deletePlaylist = (id: number) => post<Changed>(`/api/playlists/${id}/delete`, {});
export const addToPlaylist = (id: number, ids: number[]) => post<Changed>(`/api/playlists/${id}/add`, { ids });
/** Takes the song at place `index` (0 is the top) out of a playlist. */
export const removeFromPlaylist = (id: number, index: number) => post<Changed>(`/api/playlists/${id}/remove`, { index });
export const moveInPlaylist = (id: number, from: number, to: number) => post<Changed>(`/api/playlists/${id}/move`, { from, to });

// ---- Bringing a library in from another Stash, and writing this one out for them

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
	const sent = { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: file };
	return answer<Imported>('/api/import', await fetch(`${coreUrl}/api/import`, sent));
}

/** The library as the file the Android app and the web player import, and how much is in it. */
export async function exportLibrary(): Promise<{ blob: Blob; likes: number; lists: number; plays: number }> {
	const response = await fetch(`${coreUrl}/api/export`);
	if (!response.ok) throw new Refused(`/api/export: ${response.status}`, response.status);
	const text = await response.text();
	const file = JSON.parse(text) as { likes?: unknown[]; playlists?: unknown[]; history?: unknown[] };
	return {
		blob: new Blob([text], { type: 'application/json' }),
		likes: file.likes?.length ?? 0,
		lists: file.playlists?.length ?? 0,
		plays: file.history?.length ?? 0
	};
}

// ---- Add-ons

export type Tier = 'low' | 'high' | 'lossless' | 'hires';

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
	art: string | null;
};

/** An add-on's track as the core is told it (to keep it, to play it). */
export type KeptTrack = { id: string; title: string; artist: string; album?: string | null; duration_ms?: number | null; art?: string | null };

export type AddonGroup = {
	addon: { id: string; name: string };
	tracks: FoundTrack[];
	error: { code: string; message: string; retry_after: number | null } | null;
};

type Listed = { addons: AddonInfo[] };

export const addons = () => get<Listed>('/api/addons').then((r) => r.addons);
/** Adds the add-on at `link` (the address of its manifest). */
export const addAddon = (link: string) => post<Listed>('/api/addons', { link }).then((r) => r.addons);
export const removeAddon = (id: string) => post<Listed>('/api/addons/remove', { id }).then((r) => r.addons);
export const setAddonEnabled = (id: string, enabled: boolean) => post<Listed>('/api/addons/enabled', { id, enabled }).then((r) => r.addons);
export const orderAddons = (ids: string[]) => post<Listed>('/api/addons/order', { ids }).then((r) => r.addons);

/** Asks one add-on for tracks. */
export const searchAddon = (id: string, query: string, limit: number, signal?: AbortSignal) =>
	get<{ groups: AddonGroup[] }>(`/api/addons/search?q=${encodeURIComponent(query)}&addon=${encodeURIComponent(id)}&limit=${limit}`, signal).then((r) => r.groups[0] ?? null);

// ---- The player

export type Format = { sample_rate: number | null; format: string | null };

/** What the engine reports about the sound between the file and the sound card. */
export type Signal = {
	codec: string | null;
	decoded: Format;
	output: Format;
	driver: string | null;
	mode: 'exclusive' | 'shared' | null;
	resampled: boolean;
	converted: boolean;
};

export type CoreTrack = {
	/** The library's row, for a track of the library. */
	id: number | null;
	title: string;
	artist: string;
	album: string | null;
	art: string | null;
	duration_ms: number | null;
	/** Set for a track an add-on serves. */
	addon: { addon: string; track: string } | null;
	/** The core's own id of this place in the queue (it stays with the song when the queue is reordered), when the core gives one. */
	qid?: number;
};

export type CorePlayer = {
	/** False when the core has no audio engine; `reason` says why. */
	available: boolean;
	reason?: string;
	/** "loading": an add-on is being asked where the track plays from. */
	status: 'idle' | 'loading' | 'playing' | 'paused';
	index: number | null;
	queue_len: number;
	/** Goes up whenever the queue's songs or their order change. */
	queue_version: number;
	track: CoreTrack | null;
	position_ms: number;
	duration_ms: number;
	/** What the add-on said the stream is. */
	tier: Tier | null;
	/** The id of the add-on the stream comes from. */
	served_by: string | null;
	/** 0 to 100. */
	volume: number;
	/** Stash has the sound card to itself. */
	exclusive: boolean;
	shuffle: boolean;
	repeat: 'off' | 'all' | 'one';
	/** What the queue is playing from, in the words it was started with. */
	from: string | null;
	signal: Signal | null;
	/** The last song that could not be played, and why. */
	error: string | null;
	/** Nothing plays and the player rests on `track` (status "paused"): the queue ended there, or it was brought back from last time. Play starts it. */
	resting: boolean;
};

/** Nothing playing. */
export const IDLE: CorePlayer = {
	available: true,
	status: 'idle',
	index: null,
	queue_len: 0,
	queue_version: 0,
	track: null,
	position_ms: 0,
	duration_ms: 0,
	tier: null,
	served_by: null,
	volume: 100,
	exclusive: false,
	shuffle: false,
	repeat: 'off',
	from: null,
	signal: null,
	error: null,
	resting: false
};

/** One entry of what is handed to the player: a song of the library by its row, or an add-on's track. */
export type PlayEntry = { id: number } | { addon: string; track: KeptTrack };

/** The core's answer with every field present (a core without a player sends only a few). */
const whole = (a: Partial<CorePlayer>): CorePlayer => ({ ...IDLE, ...a });
const told = (path: string, body: unknown) => post<Partial<CorePlayer>>(path, body).then(whole);

export const player = (signal?: AbortSignal) => get<Partial<CorePlayer>>('/api/player', signal).then(whole);
/** Replaces the queue with these songs, in this order, and plays from `start`. */
export const playEntries = (items: PlayEntry[], start: number, from?: string) => told('/api/player/play', { items, start, ...(from ? { from } : {}) });
/** Plays the whole library (title order) from row `offset` on. */
export const playLibrary = (offset: number, from?: string) => told('/api/player/play', { offset, ...(from ? { from } : {}) });
export const setPaused = (paused: boolean) => told('/api/player/pause', { paused });
export const nextTrack = () => told('/api/player/next', {});
export const previousTrack = () => told('/api/player/previous', {});
export const seekTo = (seconds: number) => told('/api/player/seek', { seconds });
/** Plays the queue from `index` on. */
export const jumpTo = (index: number) => told('/api/player/jump', { index });
export const setVolume = (percent: number) => told('/api/player/volume', { percent });
/** Takes the sound card for Stash alone, or gives it back. */
export const setExclusive = (exclusive: boolean) => told('/api/player/exclusive', { exclusive });
export const setShuffle = (on: boolean) => told('/api/player/shuffle', { on });
export const setRepeat = (mode: 'off' | 'all' | 'one') => told('/api/player/repeat', { mode });
/** Adds songs to the queue: at its end, or (`next`) right after the one playing. */
export const enqueue = (items: PlayEntry[], next: boolean) => told('/api/player/enqueue', { items, next });
export const removeFromQueue = (index: number) => told('/api/player/remove', { index });
export const moveInQueue = (from: number, to: number) => told('/api/player/move', { from, to });

/** A track of the queue, with its place in it. */
export type QueueRow = { place: number; id: number | null; title: string; artist: string; album: string | null; art: string | null; duration_ms: number | null; addon: { addon: string; track: string } | null; qid?: number };
/** A page of the queue (`limit` at most 500). */
export const queueAt = (offset: number, limit: number, signal?: AbortSignal) => rows<QueueRow>(`/api/player/queue?offset=${offset}&limit=${limit}`, signal);

// ---- Music folders

export type ScanReport = { files: number; added: number; changed: number; unchanged: number; unreadable: number; missing: number; seconds: number };
export type Scanning = {
	scan: { running: boolean; folder: string | null; report: ScanReport | null; error: string | null; finished: number };
	/** The music folders the listener added. */
	folders: string[];
};
export const scanning = () => get<Scanning>('/api/scan');
/** Asks for a folder with the system's own dialog; null when the listener closed it. */
export const pickFolder = () => post<{ folder: string | null }>('/api/scan/pick', {}).then((r) => r.folder);
/** Scans `folder` and remembers it; without one, scans the remembered folders again. */
export const scan = (folder?: string) => post<Scanning>('/api/scan', folder ? { folder } : {});

// ---- Measuring the window

/** When this script started, in ms since the page began to load (the first of the measurement's marks). */
export const scriptStartedMs = Math.round(performance.now());

export async function postMeasurement(result: unknown): Promise<void> {
	await fetch(`${coreUrl}/api/bench-result`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(result) });
}
