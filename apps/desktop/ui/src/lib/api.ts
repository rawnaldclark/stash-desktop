/**
 * The UI <-> engine boundary, from the Stash web player. The UI never touches audio, storage or the network directly; it talks to
 * these interfaces. On the desktop `app.ts` implements them over the core's local API (`core.ts`): the core owns the library, the
 * queue and the sound.
 */
import type { Readable } from 'svelte/store';
import type { SearchResult } from './sources/search';
import type { Album, Artist, Artwork, Descriptor, Lyrics, LyricLine, LyricWord, Track } from './sources/contract';
import type { LibraryItem } from './identity';
import { NO_EQ, type AddonChange, type AddonInfo, type CoreEq, type Counts, type EqPatch, type Replaygain, type Signal, type SmartRules, type Station, type Tier } from './core';

export type { Readable, SearchResult, Counts, Album, Artist, Artwork, Descriptor, Lyrics, LyricLine, LyricWord, Track, LibraryItem, Signal, Station, Tier, CoreEq, EqPatch };

/** A queue entry: a library item plus an id for this particular slot (the same song can be queued twice). */
export type QueueItem = LibraryItem & { qid: string };

export type RepeatMode = 'off' | 'all' | 'one';

/** How a list came to be playing ("Your likes", a playlist's name, "Search"...): shown as "Playing from ..." in Up next. */
export type PlayOptions = {
	from?: string;
	/** The library's songs that play with no connection only (Library › Songs, "On this computer"): `offset` counts among them. */
	here?: boolean;
	/** The library's row of the song at `offset`: the list may have moved since it was read, and the core finds it. */
	id?: number;
};

export interface PlayerState {
	current: QueueItem | null;
	/** The place of the song playing in the whole queue; -1 when nothing is loaded. */
	index: number;
	/**
	 * The part of the queue the window holds: the core's queue can be the whole library, so only the songs from `queueStart` on are
	 * read (the one playing and what follows it). `queue[i]` is the song at place `queueStart + i`.
	 */
	queue: QueueItem[];
	/** The place in the whole queue of `queue[0]`. */
	queueStart: number;
	/** How many songs the whole queue has. */
	queueLength: number;
	playing: boolean;
	/** An add-on is being asked where the song plays from. */
	buffering: boolean;
	positionMs: number;
	durationMs: number;
	volume: number;
	shuffle: boolean;
	repeat: RepeatMode;
	/** A short, user-facing message for a toast; null when none. */
	error: string | null;
	/** The library row of the song the message is about, when it is one. */
	errorId?: number | null;
	/** How many times a song could not be played: each is told, the same message twice too. */
	errors?: number;
	/** What the queue is playing from (Up next's "Playing from ..."); null when unknown. */
	from: string | null;
	/** False when the core has no audio engine; `reason` says why. */
	available: boolean;
	reason: string | null;
	/** What the engine reports about the sound between the file and the sound card; null when nothing is loaded. */
	signal: Signal | null;
	/** The bit depth of the file playing, when the library knows it (the engine reports only what it decodes to). */
	sourceBitDepth: number | null;
	/** What the add-on said the stream is, for an add-on's stream. */
	tier: Tier | null;
	/** The add-on the stream comes from, by id. */
	servedBy: string | null;
	/** Stash has the sound card to itself (Settings › Sound). */
	exclusive: boolean;
	/** How loudness is evened out between songs (Settings › Sound). */
	replaygain: Replaygain;
	/** The equalizer of the output in use (Settings › Sound). */
	eq: CoreEq;
	/** The station growing the queue (a radio, or autoplay), when one is. */
	station: Station | null;
}

export interface PlayerAPI {
	state: Readable<PlayerState>;
	/** Precise position in ms for the LYRICS animator (not throttled like state.positionMs). */
	now(): number;
	/** Precise playback position in ms for the progress bar and for seeking relative to it. */
	position(): number;
	/** Replaces the queue and plays from `startIndex`. */
	playItems(items: LibraryItem[], startIndex?: number, opts?: PlayOptions): Promise<void>;
	/** Replaces the queue with the whole library in title order and plays from row `offset` (the list is too long to hand over song by song). */
	playLibrary(offset: number, opts?: PlayOptions): Promise<void>;
	play(): void;
	pause(): void;
	toggle(): void;
	next(): void;
	prev(): void;
	seek(ms: number): void;
	/** Play the song at queue position `i` (keeps the queue; a hard cut). */
	jumpTo(i: number): void;
	enqueue(items: LibraryItem[], opts?: { next?: boolean }): void;
	removeAt(i: number): void;
	move(from: number, to: number): void;
	setShuffle(on: boolean): void;
	setRepeat(m: RepeatMode): void;
	setVolume(v: number): void;
	/** Takes the sound card for Stash alone, or gives it back. */
	setExclusive(on: boolean): void;
	setReplaygain(mode: Replaygain): void;
	/** Reads the player's state again now (after something changed it outside these commands). */
	refresh(): void;
	/** Changes the equalizer of the output in use: one request for each tap. */
	setEq(patch: EqPatch): void;
	/** Asks for an equalizer file (the system's own dialog) and puts its correction on the output in use, switched on. False
	 * when none was chosen; rejects with the core's words when the file is refused. */
	eqFromFile(): Promise<boolean>;
	/** Every song of the queue, in order (read from the core a page at a time): what "Save as playlist" keeps. */
	wholeQueue(): Promise<LibraryItem[]>;
	/** Starts a radio: from a song (the song playing plays on, and what follows it is replaced; any other plays now), or
	 * from an artist. Songs like it follow, as they play. */
	startRadio(from: LibraryItem | { artist: string }): Promise<void>;
	/** Whether a radio can start from the song (a library row or an add-on's track). */
	canRadio(item: LibraryItem): boolean;
	/** Stops the station growing the queue: its songs still to come are taken out. True once stopped; a refusal is said,
	 * and answers false. */
	stopRadio(): Promise<boolean>;
	/** Opens the station's seed on Last.fm. */
	openLastfm(): Promise<void>;
}

export interface SearchAPI {
	/**
	 * Per-source sections ("Your library", then each add-on) plus a merged "Top results"; `failed` lists add-ons that did not answer.
	 * The library answers at once and an add-on can take seconds: `onLibrary` is handed the library's part as soon as it is there
	 * (only when add-ons are still being asked).
	 */
	search(q: string, opts?: { signal?: AbortSignal; onLibrary?: (r: SearchResult) => void }): Promise<SearchResult>;
}

/** Album and artist pages. On the desktop only the library has them (`sourceId` 'library'). */
export interface BrowseAPI {
	album(sourceId: string, id: string): Promise<Album>;
	artist(sourceId: string, id: string): Promise<Artist>;
}

export type Playlist = {
	/** The core's numeric id, as a string. */
	id: string;
	name: string;
	items: LibraryItem[];
	createdAt: number;
	updatedAt: number;
	/** A shared mix this playlist follows: its songs are not edited here. */
	followed?: boolean;
	/** A smart playlist's rules: it fills itself, and its songs are not edited by hand. */
	rules?: SmartRules;
	/** Kept on this computer (its songs are saved, and so are songs added later). */
	download?: boolean;
	/** The folder it is in (`PlaylistFolder`). */
	folder?: number;
};

/** A folder of playlists: this computer's own grouping (the phone and the web player have none). */
export type PlaylistFolder = { id: number; name: string; collapsed: boolean };

/** A play that counted. `playedAt` is when the play STARTED (epoch ms). */
export type HistoryEntry = { item: LibraryItem; playedAt: number };

export interface LibraryAPI {
	likes: Readable<LibraryItem[]>;
	playlists: Readable<Playlist[]>;
	history: Readable<HistoryEntry[]>;
	/** How much the library holds; null until the core has answered. */
	counts: Readable<Counts | null>;
	/** The likes, the playlists and the history have been read once. */
	loaded: Readable<boolean>;
	/** Goes up when the library changed under the lists (an import, a scan): a list that reads its own pages reads them again. */
	changed: Readable<number>;
	like(item: LibraryItem): Promise<void>;
	unlike(item: LibraryItem): Promise<void>;
	createPlaylist(name: string): Promise<Playlist>;
	/** A playlist that fills itself by `rules`. */
	createSmartPlaylist(name: string, rules: SmartRules): Promise<Playlist>;
	setPlaylistRules(id: string, rules: SmartRules): Promise<void>;
	renamePlaylist(id: string, name: string): Promise<void>;
	deletePlaylist(id: string): Promise<void>;
	addToPlaylist(id: string, items: LibraryItem[]): Promise<void>;
	removeFromPlaylist(id: string, index: number): Promise<void>;
	/** Moves the song at `from` to position `to` (both in range; otherwise ignored). */
	moveInPlaylist(id: string, from: number, to: number): Promise<void>;
	/** Takes several songs out of a playlist at once, each by its place and the song there. False when the playlist
	 * changed since they were picked: then nothing is taken out. */
	removeFromPlaylistAt(id: string, picks: { index: number; item: LibraryItem }[]): Promise<boolean>;
	/** The listener's playlist folders, in their order. */
	folders: Readable<PlaylistFolder[]>;
	/** A new folder, after the others (with `playlist`, that playlist goes in it). Answers its id. */
	createFolder(name: string, playlist?: string): Promise<number>;
	renameFolder(id: number, name: string): Promise<void>;
	/** Deletes a folder: its playlists stay, out of any folder. */
	deleteFolder(id: number): Promise<void>;
	/** Folds a folder shut (its playlists are not listed under it in the sidebar), or opens it. */
	foldFolder(id: number, collapsed: boolean): Promise<void>;
	/** Puts a playlist in a folder, or (null) takes it out of any. */
	moveToFolder(playlist: string, folder: number | null): Promise<void>;
	/** Reads the counts, the likes, the playlists and the history again (an import or a scan changed the library under them). */
	reload(): Promise<void>;
}

export interface LyricsAPI {
	forTrack(item: QueueItem): Promise<Lyrics | null>;
}

/** An add-on the listener added, as Settings lists it. Its link is never part of it. */
export type SourceStatus = AddonInfo;

/** The listener's add-ons: where music beyond their own files comes from. The order is theirs: the first is asked first. */
export interface SourcesAPI {
	status: Readable<SourceStatus[]>;
	/** Reads the list again. */
	reload(): Promise<void>;
	/** Adds the add-on at `link` (the address of its manifest). Rejects with the core's words when it is refused. */
	add(link: string): Promise<void>;
	remove(id: string): Promise<void>;
	setEnabled(id: string, on: boolean): Promise<void>;
	/** The add-ons' ids in the order they are asked. */
	order(ids: string[]): Promise<void>;
	/** Asks the add-on's link for a new version: what changed (none: it is up to date) and the version it gives. */
	check(id: string): Promise<{ changes: AddonChange[]; version: string; token: string | null }>;
	/** Takes the new version `check` offered: `token` is the one it came with, so what is taken is what was shown. */
	update(id: string, token: string): Promise<void>;
}

export interface AppAPI {
	player: PlayerAPI;
	search: SearchAPI;
	browse: BrowseAPI;
	library: LibraryAPI;
	lyrics: LyricsAPI;
	sources: SourcesAPI;
}

export const INITIAL_PLAYER_STATE: PlayerState = {
	current: null,
	index: -1,
	queue: [],
	queueStart: 0,
	queueLength: 0,
	playing: false,
	buffering: false,
	positionMs: 0,
	durationMs: 0,
	volume: 1,
	shuffle: false,
	repeat: 'off',
	error: null,
	from: null,
	available: true,
	reason: null,
	signal: null,
	sourceBitDepth: null,
	tier: null,
	servedBy: null,
	exclusive: false,
	replaygain: 'off',
	eq: NO_EQ,
	station: null
};
