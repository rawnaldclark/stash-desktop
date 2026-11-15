/**
 * The UI <-> engine boundary, from the Stash web player. The UI never touches audio, storage or the network directly; it talks to
 * these interfaces. On the desktop `app.ts` implements them over the core's local API (`core.ts`): the core owns the library, the
 * queue and the sound.
 */
import type { Readable } from 'svelte/store';
import type { SearchResult } from './sources/search';
import type { Album, Artist, Artwork, Descriptor, Lyrics, LyricLine, LyricWord, Track } from './sources/contract';
import type { LibraryItem } from './identity';
import type { AddonInfo, Counts, Replaygain, Signal, Tier } from './core';

export type { Readable, SearchResult, Counts, Album, Artist, Artwork, Descriptor, Lyrics, LyricLine, LyricWord, Track, LibraryItem, Signal, Tier };

/** A queue entry: a library item plus an id for this particular slot (the same song can be queued twice). */
export type QueueItem = LibraryItem & { qid: string };

export type RepeatMode = 'off' | 'all' | 'one';

/** How a list came to be playing ("Your likes", a playlist's name, "Search"...): shown as "Playing from ..." in Up next. */
export type PlayOptions = { from?: string };

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
	/** Every song of the queue, in order (read from the core a page at a time): what "Save as playlist" keeps. */
	wholeQueue(): Promise<LibraryItem[]>;
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
};

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
	renamePlaylist(id: string, name: string): Promise<void>;
	deletePlaylist(id: string): Promise<void>;
	addToPlaylist(id: string, items: LibraryItem[]): Promise<void>;
	removeFromPlaylist(id: string, index: number): Promise<void>;
	/** Moves the song at `from` to position `to` (both in range; otherwise ignored). */
	moveInPlaylist(id: string, from: number, to: number): Promise<void>;
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
	replaygain: 'off'
};
