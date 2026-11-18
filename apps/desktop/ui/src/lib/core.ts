/**
 * The window's only way to the library, the player and the add-ons: the desktop core's local HTTP API, typed. Served by the core
 * itself, the API is on this page's own origin; the desktop shell hands the core's address in before the page runs
 * (`window.__STASH_CORE__`); `?core=` names it by hand (the dev server).
 *
 * `app.ts` puts the web player's UI boundary on top of this. The desktop's own screens (the whole-library list, albums, artists,
 * music folders, import and export) read their pages from here directly.
 */

type Shell = { __STASH_CORE__?: string; __STASH_BENCH__?: boolean; __STASH_KEY__?: string };
const shell: Shell = typeof window === 'undefined' ? {} : (window as unknown as Shell);
const params = new URLSearchParams(typeof location === 'undefined' ? '' : location.search);

/** The core's address ('' when the core serves this page). */
export const coreUrl = shell.__STASH_CORE__ ?? params.get('core') ?? '';
/** The key the desktop window was given for this run: from the window's origin, the core answers only with it. */
const KEY = shell.__STASH_KEY__ ?? '';
/** The address of `path` on the core, with this run's key when the window has one. */
export function at(path: string): string {
	const url = `${coreUrl}${path}`;
	return KEY ? `${url}${url.includes('?') ? '&' : '?'}k=${encodeURIComponent(KEY)}` : url;
}

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

/** A GET of the core's (the window key added, a refusal thrown as `Refused`). */
export async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
	return answer<T>(path, await fetch(at(path), { signal }));
}

/** A POST of JSON to the core's (the window key added, a refusal thrown as `Refused`). */
export async function post<T>(path: string, body: unknown): Promise<T> {
	const sent = { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) };
	return answer<T>(path, await fetch(at(path), sent));
}

/**
 * A picture the core names (`art`): an address of its own (`/api/…`) is asked of the core, and so is a cover on a
 * server (an add-on's): the core fetches it, held to where that add-on may send Stash. The window never fetches one
 * itself: a cover's server could redirect it to a device on the listener's network.
 */
export const artUrl = (art: string | null | undefined): string | undefined => {
	if (!art) return undefined;
	if (art.startsWith('/')) return at(art);
	if (/^https?:\/\//i.test(art)) return at(`/api/art/remote?u=${encodeURIComponent(art)}`);
	return undefined;
};

/**
 * The cover's own address, from one `artUrl` made for a cover on a server: what the core is told when a song is handed
 * back to it (it keeps the address the add-on gave, never the window's way to it). Undefined for any other picture.
 */
export const rawArt = (url: string | null | undefined): string | undefined => {
	if (!url) return undefined;
	try {
		const parsed = new URL(url, 'http://core.invalid');
		if (parsed.pathname === '/api/art/remote') return parsed.searchParams.get('u') ?? undefined;
	} catch {
		/* not an address */
	}
	return undefined;
};

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
	/** It plays with no connection: a file that is here (the listener's own, or a song saved), or a song kept as it was played. */
	here: boolean;
	/** The listener chose not to play it: this song, or its artist's songs. */
	blocked?: 'song' | 'artist' | null;
};
export type LikedRow = TrackRow & { liked_at_ms: number };
export type PlayedRow = TrackRow & { played_at_ms: number };

export type Counts = {
	tracks: number;
	albums: number;
	artists: number;
	liked: number;
	playlists: number;
	/** The liked songs are kept on this computer (`/api/counts` says so; a change's answer leaves it out). */
	liked_kept?: boolean;
};

export type PlaylistRow = {
	id: number;
	name: string;
	tracks: number;
	followed: boolean;
	external_id: string | null;
	created_at_ms: number;
	updated_at_ms: number;
	/** A smart playlist's rules: it fills itself. */
	rules?: SmartRules;
	/** Kept on this computer: its songs are saved, and so are songs that come onto it later. */
	download?: boolean;
	/** The folder it is in. */
	folder?: number;
	/** The list of a source's it follows ("youtube:VL…"), and when that was last read. */
	follows?: string;
	follows_checked_ms?: number;
};

/** A playlist folder (the core's `folders.rs`). */
export type FolderRow = { id: number; name: string; position: number; collapsed: boolean };

/** Smart playlists (the core's `smart.rs`): rules over what the library knows of a song. */
export type SmartField = 'title' | 'artist' | 'album' | 'year' | 'length' | 'plays' | 'played_often' | 'last_played' | 'added' | 'liked' | 'quality' | 'here';
export type SmartOp = 'contains' | 'not_contains' | 'is' | 'is_not' | 'starts_with' | 'at_least' | 'below' | 'within' | 'not_within';
export type SmartOrder = 'added' | 'most_played' | 'recently_played' | 'least_recently_played' | 'title' | 'artist' | 'album' | 'year' | 'random';
/** `value`: text, a quality ("lossy", "lossless", "hires"), a whole number (a year, seconds, plays, days), or nothing. */
export type SmartRule = { field: SmartField; op: SmartOp; value?: string | number | null; days?: number };
export type SmartRules = { any: boolean; rules: SmartRule[]; order?: SmartOrder; limit?: number };

export type AlbumRow = { id: number; title: string; artist: string; artist_id: number; year: number | null; tracks: number; art: string | null };
export type ArtistRow = { id: number; name: string; tracks: number; albums: number; art: string | null };

type Rows<T> = { rows: T[] };
const rows = <T>(path: string, signal?: AbortSignal) => get<Rows<T>>(path, signal).then((r) => r.rows);

export const counts = () => get<Counts>('/api/counts');
/** The whole library in title order, a page at a time (`limit` at most 500). */
export const tracksAt = (offset: number, limit: number, signal?: AbortSignal) => rows<TrackRow>(`/api/tracks?offset=${offset}&limit=${limit}`, signal);
/** A page of the songs that play with no connection, in title order (Library › Songs, "On this computer"). */
export const tracksHereAt = (offset: number, limit: number, signal?: AbortSignal) => rows<TrackRow>(`/api/tracks?here=1&offset=${offset}&limit=${limit}`, signal);
/** How many songs play with no connection. */
export const hereCount = async () => (await get<{ count: number }>('/api/tracks/here')).count;
/** `here`: among the songs that play with no connection only (the core filters, so `limit` counts those). */
export const searchTracks = (query: string, limit: number, signal?: AbortSignal, here = false) =>
	rows<TrackRow>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}${here ? '&here=1' : ''}`, signal);
/** What the library has for the text: songs (at most `limit`), and the albums and artists it names (a dozen of each). */
export const searchLibrary = (query: string, limit: number, signal?: AbortSignal) =>
	get<{ rows: TrackRow[]; albums: AlbumRow[]; artists: ArtistRow[] }>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal);
/** The liked songs, the newest like first. */
export const likedAt = (offset: number, limit: number, signal?: AbortSignal) => rows<LikedRow>(`/api/liked?offset=${offset}&limit=${limit}`, signal);
/** What was played, the newest first. */
export const historyAt = (offset: number, limit: number, signal?: AbortSignal) => rows<PlayedRow>(`/api/history?offset=${offset}&limit=${limit}`, signal);
/** A play of a song the library does not have: the source that played it, its name for the song, the song as shown. */
export type HeardRow = { source: string; track: string; title: string; artist: string; album: string | null; duration_ms: number | null; art: string | null; played_at_ms: number };
export const heardAt = (offset: number, limit: number) => rows<HeardRow>(`/api/history/heard?offset=${offset}&limit=${limit}`);
export const playlists = () => rows<PlaylistRow>('/api/playlists');
export const playlistTracksAt = (id: number, offset: number, limit: number, signal?: AbortSignal) =>
	rows<TrackRow>(`/api/playlists/${id}/tracks?offset=${offset}&limit=${limit}`, signal);
export const albumsAt = (offset: number, limit: number, signal?: AbortSignal) => rows<AlbumRow>(`/api/albums?offset=${offset}&limit=${limit}`, signal);
export const album = (id: number) => get<{ album: AlbumRow; rows: TrackRow[] }>(`/api/albums/${id}`);
export const artistsAt = (offset: number, limit: number, signal?: AbortSignal) => rows<ArtistRow>(`/api/artists?offset=${offset}&limit=${limit}`, signal);
/** `with`: the artist's songs with others ("A & B", "B feat. A"), credited apart. */
export const artist = (id: number) => get<{ artist: ArtistRow; rows: TrackRow[]; albums: AlbumRow[]; with?: TrackRow[] }>(`/api/artists/${id}`);
/** One artist a credit names: `before`, `name` and `after` give the credit back as written; `id` is their artist here,
 * when the library knows them by themselves. */
export type CreditPart = { before: string; name: string; after?: string; id?: number };
/** The artists each credit names (none for a credit that names one). */
export const credits = (names: string[]) => post<{ credits: Record<string, CreditPart[]> }>('/api/credits', { credits: names });

/** The core's answer to a change the listener made: the playlists and the counts as they now are. */
/** `edits`: the core's count of changes to the likes and the playlists, this one included. */
export type Changed = { playlists: PlaylistRow[]; counts: Counts; folders?: FolderRow[]; id?: number; liked?: boolean; added?: number; done?: boolean; edits?: number };

/** Likes a song of the library, or takes the like back. */
export const likeTrack = (id: number, liked: boolean) => post<Changed>('/api/tracks/like', { id, liked });
/** Makes a track an add-on found a song of the library; the answer's `id` is its row. */
export const keepFound = (addon: string, track: KeptTrack) => post<Changed>('/api/tracks/keep', { addon, track });
/** Which of a source's songs (by what it calls them) the library has: each one's row, or null. */
export const knownRows = (addon: string, ids: string[]) => post<{ rows: (number | null)[] }>('/api/tracks/known', { addon, ids });
/** Takes songs out of the library (a saved one's file goes too). A song in a playlist, and a file of the music folders, stay: the answer counts them. */
export const removeSongs = (ids: number[]) => post<Changed & { removed: number; in_playlists: number; in_folders: number }>('/api/library/remove', { ids });
export const createPlaylist = (name: string) => post<Changed>('/api/playlists', { name });
/** Follows the YouTube Music playlist `id` ("VL…"), or reads it again: the answer's `id` is the playlist that follows it. */
export const followTube = (id: string) => post<Changed & { changed?: boolean }>('/api/youtube/follow', { id });
/** An artist the listener follows: the source whose page it is ("youtube") and its name for them ("UC…"), and when
 * their page was last read (null: not yet). */
export type FollowedArtist = { source: string; id: string; name: string; art: string | null; followed_at_ms: number; checked_ms: number | null };
/** A record by a followed artist that came out since they were followed, and when it was found. */
export type NewRelease = {
	source: string;
	id: string;
	title: string;
	kind: 'album' | 'single';
	year: number | null;
	art: string | null;
	artist: string;
	artist_id: string;
	new_at_ms: number;
};
/** The answer to following an artist, or stopping: the artists followed as they now are. */
export type FollowedChanged = { followed: FollowedArtist[]; edits?: number };
/** Follows an artist (`art`: their picture as the source gave it); their page is read at once for this year's records. */
export const followArtist = (source: string, id: string, name: string, art?: string | null) =>
	post<FollowedChanged>('/api/artists/follow', { source, id, name, art: art ?? undefined });
export const unfollowArtist = (source: string, id: string) => post<FollowedChanged>('/api/artists/unfollow', { source, id });
export const followedArtists = () => rows<FollowedArtist>('/api/artists/followed');
/** Records by followed artists found in the last two months, the newest first. */
export const releases = () => rows<NewRelease>('/api/releases');
/** What a playlist file brought in: the new playlist, and what came of its entries (files the library had, files it
 * added, songs found by their words, and entries left out). */
export type Brought = { id: number; name: string; songs: number; files: number; added: number; by_words: number; left_out: number };
/** The system's own dialog, for a playlist file (.m3u, .m3u8); null when none was chosen. */
export const pickPlaylistFile = () => post<{ path: string | null }>('/api/playlists/file/pick', {});
export const playlistFromFile = (path: string) => post<Changed & { brought: Brought }>('/api/playlists/file', { path });
/** A playlist (`null`: the liked songs) as an .m3u8 file: its name, and how many songs went in and were left out. */
export async function playlistFile(id: number | null): Promise<{ blob: Blob; name: string; songs: number; leftOut: number }> {
	const path = id === null ? '/api/liked/file' : `/api/playlists/${id}/file`;
	const response = await fetch(at(path));
	if (!response.ok) throw new Refused(`${path}: ${response.status}`, response.status);
	const text = await response.text();
	const named = /filename\*=UTF-8''([^;]+)/i.exec(response.headers.get('content-disposition') ?? '');
	return {
		blob: new Blob([text], { type: 'audio/x-mpegurl' }),
		name: named ? decodeURIComponent(named[1]) : 'Playlist.m3u8',
		songs: Number(response.headers.get('x-songs') ?? 0),
		leftOut: Number(response.headers.get('x-left-out') ?? 0)
	};
}
export const folders = () => rows<FolderRow>('/api/folders');
/** With `playlist`, that playlist goes in the new folder. */
export const createFolder = (name: string, playlist?: number) => post<Changed>('/api/folders', { name, playlist });
export const renameFolder = (id: number, name: string) => post<Changed>(`/api/folders/${id}/rename`, { name });
export const deleteFolder = (id: number) => post<Changed>(`/api/folders/${id}/delete`, {});
export const foldFolder = (id: number, collapsed: boolean) => post<Changed>(`/api/folders/${id}/fold`, { collapsed });
/** Into a folder, or (null) out of any. */
export const setPlaylistFolder = (id: number, folder: number | null) => post<Changed>(`/api/playlists/${id}/folder`, { folder });
/** Makes a smart playlist: it fills itself by `rules`. */
export const createSmartPlaylist = (name: string, rules: SmartRules) => post<Changed>('/api/playlists', { name, rules });
/** Gives a smart playlist new rules. */
export const setPlaylistRules = (id: number, rules: SmartRules) => post<Changed>(`/api/playlists/${id}/rules`, { rules });
/** How many songs `rules` pick now, and the first few. Rules that make no sense are refused in the core's words. */
export const previewRules = (rules: SmartRules) => post<{ count: number; rows: TrackRow[] }>('/api/playlists/preview', { rules });

// ---- Songs and artists the listener chose not to play

/** A song (every copy of its words) or an artist the listener chose not to play. */
export type Blocked = { kind: 'song' | 'artist'; key: string; title: string; artist: string; at_ms: number };
export type BlockedAnswer = { rows: Blocked[]; songs?: { id: number; blocked: 'song' | 'artist' | null }[]; edits?: number };
export const blocks = () => get<BlockedAnswer>('/api/blocks').then((a) => a.rows);
/** To play one again, `key` (as the list gives it) may stand for its words. */
/** `songs`: several songs at once (kind "song"), each by its words. */
export const setBlocked = (b: { kind: 'song' | 'artist'; title?: string; artist?: string; blocked: boolean; key?: string; songs?: { title: string; artist: string }[] }) =>
	post<BlockedAnswer>('/api/blocks', b);

// ---- Lists kept on this computer (a playlist, or the liked songs)

/** Whether a list is kept on this computer, and how far: its songs, how many play with no connection, how many wait to be saved, and how many could not be. */
export type ListDownload = {
	on: boolean;
	songs: number;
	here: number;
	waiting: number;
	failed: number;
	/** Songs the listener took off this computer: the list leaves them be. */
	declined: number;
	/** The core's count of changes, when this answer made one. */
	edits?: number;
};
const keptPath = (list: string) => (list === 'liked' ? '/api/liked/download' : `/api/playlists/${encodeURIComponent(list)}/download`);
export const listDownload = (list: string) => get<ListDownload>(keptPath(list));
export const setListDownload = (list: string, on: boolean) => post<ListDownload>(keptPath(list), { on });

// ---- Shortcuts that work in any app (the desktop app holds them; in a browser there are none)

export type Shortcuts = { available: boolean; on: boolean; actions: string[]; keys: Record<string, string>; trouble: Record<string, string> };
export const shortcuts = () => get<Shortcuts>('/api/settings/shortcuts');
/** `paused`: lets go of the keys while the listener picks new ones (keys held system-wide never reach the window). */
export const setShortcuts = (change: { on?: boolean; keys?: Record<string, string>; paused?: boolean }) => post<Shortcuts>('/api/settings/shortcuts', change);

// ---- Offline: Stash plays only what is on this computer, and asks no add-on

export const offline = () => get<{ offline: boolean }>('/api/offline').then((r) => r.offline);
export const setOffline = (on: boolean) => post<{ offline: boolean }>('/api/offline', { offline: on }).then((r) => r.offline);

// ---- Scrobbling (the core's `scrobble.rs`, `listenbrainz.rs` and `scrobbling.rs`)

/** A service the listener's plays go to: ListenBrainz, or a server that speaks its language. Never its token. */
export type ScrobbleTarget = {
	id: number;
	kind: string;
	url: string;
	/** The name the service knows the listener by. */
	user: string | null;
	enabled: boolean;
	/** It refused the token: nothing is sent until it is given again (plays wait meanwhile). */
	refused: boolean;
	added_at_ms: number;
	sent: number;
	sent_at_ms: number | null;
	/** Plays it said were wrong, let go. */
	dropped: number;
	waiting: number;
	trouble: { message: string; at_ms: number } | null;
};
type Targets = { targets: ScrobbleTarget[] };
export const scrobbling = () => get<Targets>('/api/scrobbling');
/** Adds a service by the listener's token (checked with the service first); `url` for another server than ListenBrainz.
 *  Giving a service's token again makes it go on. */
export const addScrobbling = (token: string, url?: string) => post<Targets & { id: number }>('/api/scrobbling', url ? { token, url } : { token });
export const setScrobblingEnabled = (id: number, enabled: boolean) => post<Targets>(`/api/scrobbling/${id}/enabled`, { enabled });
export const removeScrobbling = (id: number) => post<Targets>(`/api/scrobbling/${id}/remove`, {});
/** Asks a service again now, though it was left a while after it could not take plays. */
export const scrobbleAgain = (id: number) => post<Targets>(`/api/scrobbling/${id}/again`, {});
export const renamePlaylist = (id: number, name: string) => post<Changed>(`/api/playlists/${id}/rename`, { name });
export const deletePlaylist = (id: number) => post<Changed>(`/api/playlists/${id}/delete`, {});
export const addToPlaylist = (id: number, ids: number[]) => post<Changed>(`/api/playlists/${id}/add`, { ids });
/** Takes the song at place `index` (0 is the top) out of a playlist. */
export const removeFromPlaylist = (id: number, index: number) => post<Changed>(`/api/playlists/${id}/remove`, { index });
/** Several songs out at once, each by its place and the song there: only while every place still holds it (`done` false otherwise, and nothing goes). */
export const removeFromPlaylistAt = (id: number, places: { index: number; id: number }[]) => post<Changed>(`/api/playlists/${id}/remove`, { places });
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
		/** Songs chosen not to be played there that were not here yet. */
		blocks_added?: number;
	};
	counts: Counts;
};

/** Hands the core a library file from the Android app or the web player. The file goes as it is: the core reads it. */
export async function importLibrary(file: Blob): Promise<Imported> {
	const sent = { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: file };
	return answer<Imported>('/api/import', await fetch(at('/api/import'), sent));
}

/** The library as the file the Android app and the web player import, and how much is in it. */
export async function exportLibrary(): Promise<{ blob: Blob; likes: number; lists: number; plays: number }> {
	const response = await fetch(at('/api/export'));
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
	/** Where it is, noted when it was added: it may send Stash no nearer. Null when it cannot be used. */
	reach: 'internet' | 'home' | 'here' | null;
	resources: string[];
	enabled: boolean;
	/** Why it cannot be used, when it cannot. */
	problem: string | null;
	/** Part of Stash itself (YouTube Music): asked after the listener's add-ons; switched off, never removed. */
	builtIn?: boolean;
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

/** One way an add-on's new version differs from the one added. `more`: it gives the add-on more than was agreed to. */
export type AddonChange = { words: string; more: boolean };
/** An add-on's link asked for a new version: what changed (nothing: it is up to date), and the version it gives. */
export type AddonCheck = { addons: AddonInfo[]; changes: AddonChange[]; version: string; token: string | null };
/** Asks the add-on's link for a new version. Nothing the listener would care about is taken until `updateAddon`. */
export const checkAddon = (id: string) => post<AddonCheck>('/api/addons/check', { id });
/** Takes the new version that was offered, as it was shown. */
export const updateAddon = (id: string, token: string) => post<Listed>('/api/addons/update', { id, token }).then((r) => r.addons);

/** What an add-on gave since a time (streams and downloads), when it last gave a song, and its last trouble. */
export type AddonUse = { streams: number; downloads: number; found_at_ms: number | null; trouble: { at_ms: number; message: string } | null };
/** Each add-on's use since `since` (ms; the window's own midnight for "today"). */
export const addonUse = (since: number) => get<{ addons: Record<string, AddonUse> }>(`/api/addons/use?since=${since}`).then((r) => r.addons);

// ---- YouTube Music's own pages: what a search finds beside the songs, and an album's, an artist's and a playlist's page

/** The built-in source's id among the listener's sources. */
export const YOUTUBE = 'youtube';
export type TubeSong = { id: string; title: string; artist: string; album: string | null; duration_ms: number | null; art: string | null; explicit: boolean; artist_id: string | null; album_id: string | null };
export type TubeAlbum = { id: string; title: string; artist: string; artist_id: string | null; year: number | null; single: boolean; art: string | null };
export type TubeArtist = { id: string; name: string; art: string | null };
export type TubePlaylist = { id: string; title: string; author: string; art: string | null };
export type TubeFound = {
	top: { kind: 'artist'; artist: TubeArtist } | { kind: 'album'; album: TubeAlbum } | { kind: 'song'; song: TubeSong } | null;
	albums: TubeAlbum[];
	artists: TubeArtist[];
	playlists: TubePlaylist[];
};
export type TubeAlbumPage = TubeAlbum & { songs: TubeSong[] };
export type TubeArtistPage = TubeArtist & { songs: TubeSong[]; albums: TubeAlbum[]; singles: TubeAlbum[]; related: TubeArtist[]; albums_more: boolean; singles_more: boolean; songs_all: string | null };
/** All of an artist's albums or singles; `whole` false when YouTube Music gave only part of the list. */
export type TubeDiscography = { id: string; name: string; albums: TubeAlbum[]; whole: boolean };
export type TubePlaylistPage = TubePlaylist & { songs: TubeSong[]; more: boolean };
const tube = (what: string, id: string) => `/api/youtube/${what}?id=${encodeURIComponent(id)}`;
export const tubeSearch = (query: string, signal?: AbortSignal) => get<TubeFound>(`/api/youtube/search?q=${encodeURIComponent(query)}`, signal);
export const tubeAlbum = (id: string) => get<TubeAlbumPage>(tube('album', id));
export const tubeArtist = (id: string) => get<TubeArtistPage>(tube('artist', id));
export const tubePlaylist = (id: string) => get<TubePlaylistPage>(tube('playlist', id));
export const tubeDiscography = (id: string, kind: 'albums' | 'singles') => get<TubeDiscography>(`${tube('discography', id)}&kind=${kind}`);

// ---- a Mornye extension's catalogue (a metadata provider: Qobuz, Tidal, Deezer, ...): its artists, albums, playlists

/** The window's source id of a Mornye extension (`mornye.<key>`), and back. */
export const MORNYE = 'mornye.';
export const mornyeKey = (sourceId: string): string | null => (sourceId.startsWith(MORNYE) ? sourceId.slice(MORNYE.length) : null);
export type CatAlbum = { id: string; title: string; artist: string; artist_id: string | null; year: number | null; single: boolean; tracks: number | null; art: string | null };
export type CatArtist = { id: string; name: string; art: string | null };
export type CatFound = { artists: CatArtist[]; albums: CatAlbum[] };
export type CatArtistPage = CatArtist & { albums: CatAlbum[]; singles: CatAlbum[]; songs: FoundTrack[] };
export type CatAlbumPage = CatAlbum & { songs: FoundTrack[] };
const cat = (what: string, key: string, id: string) => `/api/mornye/${what}?key=${encodeURIComponent(key)}&id=${encodeURIComponent(id)}`;
export const catFind = (key: string, q: string, signal?: AbortSignal) => get<CatFound>(`/api/mornye/find?key=${encodeURIComponent(key)}&q=${encodeURIComponent(q)}`, signal);
export const catArtist = (key: string, id: string) => get<CatArtistPage>(cat('artist', key, id));
export const catAlbum = (key: string, id: string) => get<CatAlbumPage>(cat('album', key, id));

// ---- YouTube Music for a listener who signed in: their account, their library, and the home page

/** A card of the home page or of the library: a song plays; the rest open a page. */
export type TubeCard = { kind: 'song'; song: TubeSong } | { kind: 'album'; album: TubeAlbum } | { kind: 'artist'; artist: TubeArtist } | { kind: 'playlist'; playlist: TubePlaylist };
export type TubeShelf = { title: string; over: string | null; cards: TubeCard[] };
/** The account as the window is told it (never the login itself). `window`: this Stash can open Google's sign-in. */
export type TubeAccount = {
	signed_in: boolean;
	/** YouTube no longer takes the login: sign in again. */
	spent: boolean;
	/** A sign-in window is open. */
	signing: boolean;
	window: boolean;
	/** Why the last sign-in was not kept. */
	trouble: string | null;
	account: { name: string; handle: string | null; photo: string | null } | null;
};
export type TubeSaved = 'playlists' | 'albums' | 'artists';
export const tubeHome = () => get<{ shelves: TubeShelf[] }>('/api/youtube/home').then((r) => r.shelves);
export const tubeLibrary = (kind: TubeSaved) => get<{ cards: TubeCard[] }>(`/api/youtube/library?kind=${kind}`).then((r) => r.cards);
export const tubeAccount = () => get<TubeAccount>('/api/youtube/account');
/** Opens Google's sign-in in a window of its own; the account says when it is done (`signing` false). */
export const tubeSignInWindow = () => post<TubeAccount>('/api/youtube/account/window', {});
/** Signs in with cookies copied from a browser (a `Cookie` header or a cookies.txt file). */
export const tubeSignInWith = (cookies: string) => post<TubeAccount>('/api/youtube/account', { cookies });
export const tubeSignOut = () => post<TubeAccount>('/api/youtube/account/out', {});

/** yt-dlp, for the songs YouTube's apps will not play to Stash: Stash's own copy (`own`) or the system's. */
export type TubeFallback = {
	on: boolean;
	found: { own: boolean; version: string | null; runtime: string | null } | null;
	fetching: { what: string; got: number; total: number | null } | null;
	trouble: string | null;
	can_fetch: boolean;
	/** A fetch would bring Deno too (there is no JavaScript runtime yt-dlp can use). */
	with_deno: boolean;
};
export const tubeFallback = () => get<TubeFallback>('/api/youtube/ytdlp');
export const setTubeFallback = (on: boolean) => post<TubeFallback>('/api/youtube/ytdlp', { on });
export const fetchTubeFallback = () => post<TubeFallback>('/api/youtube/ytdlp/install', {});

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
	/** The samples are made louder or softer on the way: the volume is not at 100, or ReplayGain evens the song out. */
	scaled: boolean;
	/** An equalizer changes the samples on the way: the engine's filter list holds Stash's equalizer, and it has not failed. */
	equalized: boolean;
};

/** A tone choice of the equalizer: less, as it is, or more. */
export type Tilt = 'less' | 'as-is' | 'more';

/** The equalizer of the output in use, as the core says it (`eq` in /api/player). Each output keeps its own. */
export type CoreEq = {
	/** The output in use: the one the engine said it opened, else the one chosen. */
	output: Device;
	/** The output was not chosen by hand: it is the system's choice right now. */
	system_choice: boolean;
	on: boolean;
	bass: Tilt;
	treble: Tilt;
	/** The correction read from a file (Equalizer APO's format): its name, the file's own name, how many filters it has, how
	 * many lines of the file were switched off there or passed over, and what was read, in the core's sentence ("Read 10
	 * filters and a preamp of −6.4 dB from Sennheiser HD 650 ParametricEQ.txt."). */
	correction: { name: string; file: string; filters: number; switched_off: number; passed_over: number; summary: string } | null;
	/** How much softer Stash plays, in dB, to leave room for the boosts (0 for cuts only). */
	softer_db: number;
	/** What the equalizer does, in words ("Sennheiser HD 650, more bass"); null when it changes nothing. */
	describe: string | null;
	/** It was turned off while the sound card is open at its format: the card is opened again at the song's own at the next pause. */
	reopen_at_pause: boolean;
	/** Why it was turned off by itself, when it was, in the core's whole sentence: "Your correction “{name}” could not be read
	 * again: {reason}", or "Your equalizer could not be used and was turned off: {the engine's reason}". */
	trouble: string | null;
};

/** A change to the equalizer of the output in use: any of the switch, the two tones, and (`correction: null`) taking the
 * correction off. A correction only comes from a file (`eqFromFile`). */
export type EqPatch = { on?: boolean; bass?: Tilt; treble?: Tilt; correction?: null };

/** No equalizer: off, with nothing set. */
export const NO_EQ: CoreEq = {
	output: { name: 'auto', description: '' },
	system_choice: false,
	on: false,
	bass: 'as-is',
	treble: 'as-is',
	correction: null,
	softer_db: 0,
	describe: null,
	reopen_at_pause: false,
	trouble: null
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
	/** The core's count of changes to the likes and the playlists: when it moves, another window or a shortcut changed them. */
	edits?: number;
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
	/** How loudness is evened out between songs (ReplayGain). */
	replaygain: Replaygain;
	/** The equalizer of the output in use. */
	eq: CoreEq;
	shuffle: boolean;
	repeat: 'off' | 'all' | 'one';
	/** What the queue is playing from, in the words it was started with. */
	from: string | null;
	signal: Signal | null;
	/** The last song that could not be played, and why. */
	error: string | null;
	/** That song's row in the library, when it is one. */
	error_id: number | null;
	/** How many times a song could not be played: each is told, the same one twice too. */
	errors: number;
	/** Nothing plays and the player rests on `track` (status "paused"): the queue ended there, or it was brought back from last time. Play starts it. */
	resting: boolean;
	/** Goes up only when the whole queue is replaced (a new queue, a stop). */
	queue_id?: number;
	/** The station growing the queue (a radio, or autoplay), when one is. */
	station?: Station | null;
};

/** A station growing the queue, as the core says it (`station` in /api/player). */
export type Station = {
	kind: 'radio' | 'autoplay';
	/** "Teardrop radio", "Massive Attack radio". */
	label: string;
	/** What it is like: a song, or an artist. */
	seed: { title?: string; artist: string } | null;
	/** Who chose its songs: Last.fm, or (when Last.fm could not help) the library. */
	via: 'lastfm' | 'library';
	/** Why the library chose them, when it did. */
	why: string | null;
	/** Its first songs are still being found. */
	finding: boolean;
	/** How many songs it added: none, with nothing after the song playing and no more being found, means it found none. */
	added: number;
	/** The core's id of the first of its songs still to come (Up next draws its line there). */
	from_qid: number | null;
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
	replaygain: 'off',
	eq: NO_EQ,
	shuffle: false,
	repeat: 'off',
	from: null,
	signal: null,
	error: null,
	error_id: null,
	errors: 0,
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
export const playLibrary = (offset: number, from?: string, here = false, id?: number) =>
	told('/api/player/play', { offset, ...(from ? { from } : {}), ...(here ? { here: true } : {}), ...(id !== undefined ? { id } : {}) });
export const setPaused = (paused: boolean) => told('/api/player/pause', { paused });
export const nextTrack = () => told('/api/player/next', {});
export const previousTrack = () => told('/api/player/previous', {});
export const seekTo = (seconds: number) => told('/api/player/seek', { seconds });
/** Plays the queue from `index` on. */
export const jumpTo = (index: number) => told('/api/player/jump', { index });
export const setVolume = (percent: number) => told('/api/player/volume', { percent });
// ---- Downloads: songs saved to this computer

export type DownloadState = 'queued' | 'saving' | 'saved' | 'failed';
export type DownloadRow = TrackRow & { state: DownloadState; error: string | null; bytes: number | null; tier: Tier | null; addon: string | null; saved_at_ms: number | null };
export type DownloadCounts = { queued: number; saving: number; saved: number; failed: number };
/** The list (the one coming in first, then those waiting, those not saved, the saved ones newest first), how far the one coming in is, and where songs go. */
export type Downloads = {
	rows: DownloadRow[];
	counts: DownloadCounts;
	now: { id: number; bytes: number; of: number | null } | null;
	/** Saving waits after trouble with an add-on (its allowance is spent, it could not be reached): why, and until when. */
	held?: { why: string; until_ms: number } | null;
	folder: string;
	queued?: number;
};
export const downloads = (offset: number, limit: number) => get<Downloads>(`/api/downloads?offset=${offset}&limit=${limit}`);
/** Puts songs on the list: songs of the library, all the liked songs, or a playlist's. `queued` says how many were not on it or saved already. */
export const saveSongs = (what: { ids: number[] } | { liked: true } | { playlist: number }) => post<Downloads>('/api/downloads', what);
/** Takes songs off the list; a saved one's file is deleted and it plays through the add-ons again. */
export const unsaveSongs = (ids: number[]) => post<Downloads>('/api/downloads/remove', { ids });
/** Songs that were not saved, back on the list (all of them without `ids`). */
export const retrySaving = (ids: number[] = []) => post<Downloads>('/api/downloads/retry', { ids });
/** Where new songs are saved: a folder, or one picked in the system's dialog. */
export const setDownloadsFolder = (where: { folder: string } | { pick: true }) => post<Downloads>('/api/downloads/folder', where);

// ---- Not found: songs no add-on could find, fixed by hand

export type UnfoundRow = TrackRow & { first_at_ms: number; last_at_ms: number };
/** The list (newest miss first), how many are on it, and every song known not to play (on it or set aside). */
export type Unfound = { rows: UnfoundRow[]; open: number; marked: number[]; saving?: boolean };
/** The add-ons' closest results for one song, and the words searched. */
export type Candidates = { query: string; groups: AddonGroup[] };
export const unfound = (offset: number, limit: number) => get<Unfound>(`/api/unfound?offset=${offset}&limit=${limit}`);
export const candidates = (id: number, words?: string, signal?: AbortSignal) =>
	get<Candidates>(`/api/unfound/${id}/candidates${words ? `?q=${encodeURIComponent(words)}` : ''}`, signal);
/** The listener's pick: kept as that add-on's name for the song. `saving`: a save of it that failed goes again. */
export const fixSong = (id: number, addon: string, track: string) => post<Unfound>('/api/unfound/fix', { id, addon, track });
/** "None of these" (or, with `aside` false, back on the list). */
export const setAside = (ids: number[], aside = true) => post<Unfound>('/api/unfound/aside', { ids, aside });

// ---- Lyrics

/** A song's lyrics and where they came from (".lrc file", "the file's tags", an add-on's name, "LRCLIB"); `lyrics` null for none. */
export type FoundLyrics = { lyrics: import('./sources/contract').Lyrics | null; via: string | null };
/** The lyrics of a library row (`id`), or of any song by what it is (with the add-on and its id for it, when one serves it). */
export function lyrics(song: { id: number } | { title: string; artist: string; album?: string; durationMs?: number; isrc?: string; addon?: string; track?: string }, signal?: AbortSignal) {
	const q = new URLSearchParams();
	if ('id' in song) q.set('id', String(song.id));
	else {
		q.set('title', song.title);
		q.set('artist', song.artist);
		if (song.album) q.set('album', song.album);
		if (song.durationMs) q.set('duration_ms', String(Math.round(song.durationMs)));
		if (song.isrc) q.set('isrc', song.isrc);
		if (song.addon && song.track) {
			q.set('addon', song.addon);
			q.set('track', song.track);
		}
	}
	return get<FoundLyrics>(`/api/lyrics?${q}`, signal);
}
/** Whether songs like the last ones are added when the queue ends. */
export const autoplaySettings = () => get<{ on: boolean }>('/api/settings/autoplay');
export const setAutoplay = (on: boolean) => post<{ on: boolean }>('/api/settings/autoplay', { on });
/** Whether this build has similar music from Last.fm (radio), and whether it is on. */
export type SimilarSettings = { available: boolean; on: boolean };
export const similarSettings = () => get<SimilarSettings>('/api/settings/similar');
/** Off, what Last.fm said is deleted (`forgot`: how much). */
export const setSimilar = (on: boolean) => post<SimilarSettings & { forgot?: number }>('/api/settings/similar', { on });
/** A radio from a song (it plays now), from the song playing (it plays on, and what follows is replaced), or from an artist. */
export type RadioStart = { song: PlayEntry } | { playing: true } | { artist: string };
export const startRadio = (body: RadioStart) => post<{ station: Station | null }>('/api/radio/start', body);
/** Stops the station: its songs still to come are taken out. */
export const stopRadio = () => post<{ removed: number }>('/api/radio/stop', {});
/** Opens the station's seed on Last.fm: the window's shell opens it (`opened`), else the page is given the address. */
export const openLastfm = () => post<{ opened?: boolean; url?: string }>('/api/radio/lastfm', {});
/** Whether closing the window keeps Stash playing in the tray. */
export const windowSettings = () => get<{ tray: boolean }>('/api/settings/window');
/** Whether there is a mini player to switch to (only inside the desktop window), and the switch between it and the full window. */
export const windowInfo = () => get<{ mini: boolean }>('/api/window');
export const showView = (view: 'mini' | 'full') => post<{ view: string }>('/api/window', { view });
export const setWindowSettings = (tray: boolean) => post<{ tray: boolean }>('/api/settings/window', { tray });
/** Whether lyrics are looked up on LRCLIB when no add-on has them. */
export const lyricsSettings = () => get<{ lrclib: boolean }>('/api/settings/lyrics');
export const setLyricsSettings = (lrclib: boolean) => post<{ lrclib: boolean }>('/api/settings/lyrics', { lrclib });

// ---- Songs kept as they were played from an add-on

/** How many songs are kept, the room they take, and the room they may take (0: none are kept). */
export type PlayedRoom = { songs: number; bytes: number; cap_mb: number };
export const playedRoom = () => get<PlayedRoom>('/api/played');
/** The room copies may take; the songs played longest ago go when it is smaller than what they take. */
export const setPlayedRoom = (cap_mb: number) => post<PlayedRoom>('/api/played', { cap_mb });
export const clearPlayed = () => post<PlayedRoom>('/api/played/clear', {});

/** A sound output of the system: `name` is the engine's ("auto" for the system's own choice), `description` the system's. */
export type Device = { name: string; description: string };
/** The outputs, the one in use, and the one chosen last when it is not connected now. */
export type Devices = { devices: Device[]; device: string; missing: Device | null };
export const devices = () => get<Devices>('/api/player/devices');
/** Sends the sound to another output; it holds until changed. */
export const setDevice = (name: string) => post<Devices>('/api/player/device', { name });
/** Takes the sound card for Stash alone, or gives it back. */
export const setExclusive = (exclusive: boolean) => told('/api/player/exclusive', { exclusive });
/** Off; every song equally loud ("track"); or the album's own differences kept ("album"). */
export type Replaygain = 'off' | 'track' | 'album';
export const setReplaygain = (mode: Replaygain) => told('/api/player/replaygain', { mode });
/** Changes the equalizer of the output in use. A tone other than "as-is" turns it on, unless `on` says otherwise. */
export const setEq = (patch: EqPatch) => told('/api/player/eq', patch);
/** The system's own dialog, for an equalizer file (Equalizer APO's format); null when none was chosen. */
export const pickEqFile = () => post<{ path: string | null }>('/api/player/eq/pick', {});
/** Reads an equalizer file and puts its correction on the output in use, switched on. A file Stash cannot play exactly as it
 * says is refused, in the reader's words, and nothing changes. */
export const eqFromFile = (path: string) => told('/api/player/eq/file', { path });
export const setShuffle = (on: boolean) => told('/api/player/shuffle', { on });
export const setRepeat = (mode: 'off' | 'all' | 'one') => told('/api/player/repeat', { mode });
/** Adds songs to the queue: at its end, or (`next`) right after the one playing. */
export const enqueue = (items: PlayEntry[], next: boolean) => told('/api/player/enqueue', { items, next });
export const removeFromQueue = (index: number) => told('/api/player/remove', { index });
export const moveInQueue = (from: number, to: number) => told('/api/player/move', { from, to });

/** A track of the queue, with its place in it. */
export type QueueRow = { place: number; id: number | null; title: string; artist: string; album: string | null; art: string | null; duration_ms: number | null; addon: { addon: string; track: string } | null; qid?: number; /** It plays with no connection (offline, the others are passed over). */ here?: boolean };
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
	await fetch(at('/api/bench-result'), { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(result) });
}

// ---- Mornye extensions (SpotiFLAC-format packages, run by the core)

export type MornyeAccess = { kind: 'verification' | 'login' | 'settings' | 'none' | string; verified: boolean; missing: string[]; ready: boolean };
/** A field of an extension's settings, as its manifest declares it. */
export type MornyeField = {
	key: string;
	type?: string;
	label?: string;
	description?: string;
	help?: string;
	default?: unknown;
	required?: boolean;
	secret?: boolean;
	options?: (string | { value: string; label?: string })[];
	action?: string;
	placeholder?: string;
};
export type MornyeQuality = { id: string; label?: string; name?: string; kind?: string; description?: string };
export type MornyeInstalled = {
	key: string;
	name: string;
	display_name: string;
	version: string;
	description: string;
	enabled: boolean;
	playable: boolean;
	methods: string[];
	repository: string | null;
	installed_at: number;
	settings: MornyeField[] | null;
	quality_options: MornyeQuality[] | null;
	global_actions: { action: string; label?: string }[] | null;
	access: MornyeAccess;
};
export type MornyeRepository = { id: string; url: string; name: string | null; official: boolean; refreshed_at: number | null; error: string | null; count: number };
export type MornyeEntry = { repository: string; id: string; version: string; display_name: string; description: string; compatible: boolean; installed: string | null; update: boolean };
export type Mornye = {
	installed: MornyeInstalled[];
	repositories: MornyeRepository[];
	catalogue: MornyeEntry[];
	refreshing: boolean;
	verifying: string | null;
	/** The core can open a verification page in a window of its own. */
	window: boolean;
	/** FFmpeg, for songs some services send encrypted (Amazon Music's) or in pieces (SoundCloud's). */
	ffmpeg: { found: boolean; can_fetch: boolean; fetching: { got: number; total: number | null } | null; trouble: string | null };
};
export type MornyeStatus = {
	authenticated: boolean;
	verification_required?: boolean;
	open_auth_url?: string;
	access: MornyeAccess;
	verifying: boolean;
	/** What came of the last verification in the window: "ok", or why not. */
	result: string | null;
};
/** What an action answered: words, a page to open, or a form to fill in. */
export type MornyeActed = {
	message?: string;
	success?: boolean;
	error?: string;
	open_auth_url?: string;
	window?: boolean;
	action_form?: { version: number; title?: string; description?: string; submit_action: string; fields: MornyeField[] };
	byoa_form?: MornyeActed['action_form'];
	formToken?: string;
};
export const mornye = () => get<Mornye>('/api/mornye');
export const mornyeAddRepository = (url: string) => post<Mornye>('/api/mornye/repositories', { url });
export const mornyeRemoveRepository = (id: string) => post<Mornye>('/api/mornye/repositories/remove', { id });
export const mornyeRefresh = (id?: string) => post<Mornye>('/api/mornye/refresh', { id: id ?? '' });
export const mornyeInstall = (repository: string, id: string) => post<{ installed: MornyeInstalled }>('/api/mornye/install', { repository, id });
export async function mornyeInstallFile(file: Blob): Promise<{ installed: MornyeInstalled }> {
	const sent = { method: 'POST', headers: { 'Content-Type': 'application/octet-stream' }, body: file };
	return answer<{ installed: MornyeInstalled }>('/api/mornye/install-file', await fetch(at('/api/mornye/install-file'), sent));
}
export const mornyeFetchFfmpeg = () => post<Mornye>('/api/mornye/ffmpeg', {});
export const mornyeRemove = (key: string) => post<Mornye>('/api/mornye/remove', { key });
export const mornyeSetEnabled = (key: string, on: boolean) => post<Mornye>('/api/mornye/enabled', { key, on });
export const mornyeSettings = (key: string) =>
	get<{ settings: Record<string, unknown>; schema: MornyeField[] | null; quality_options: MornyeQuality[] | null; global_actions: { action: string; label?: string }[] | null }>(`/api/mornye/settings?key=${encodeURIComponent(key)}`);
export const mornyeSaveSettings = (key: string, values: Record<string, unknown>) => post<{ settings: Record<string, unknown> }>('/api/mornye/settings', { key, values });
export const mornyeStatus = (key: string) => get<MornyeStatus>(`/api/mornye/access?key=${encodeURIComponent(key)}`);
export const mornyeVerify = (key: string) => post<{ ready: boolean; window?: boolean; url?: string }>('/api/mornye/verify', { key });
export const mornyeCallback = (key: string, url: string) => post<unknown>('/api/mornye/callback', { key, url });
export const mornyeLogout = (key: string) => post<{ ok: boolean }>('/api/mornye/logout', { key });
export const mornyeAction = (key: string, action: string, input?: Record<string, unknown>, token?: string) =>
	post<MornyeActed>('/api/mornye/action', { key, action, input: input ?? null, token: token ?? null });
