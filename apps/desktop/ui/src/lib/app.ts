/**
 * The one place the UI gets its APIs from:  import { player, library } from '$lib/app'.
 *
 * On the desktop these are the web player's UI boundary (`api.ts`) put on top of the core's local API (`core.ts`). The core owns the
 * library, the queue and the sound; this module asks it, maps what it says to what the UI shows, and tells it what the listener did.
 * Nothing here plays audio or stores anything.
 */
import { get, writable, type Readable } from 'svelte/store';
import * as core from './core';
import { Refused, said } from './core';
import {
	INITIAL_PLAYER_STATE,
	type Album,
	type AppAPI,
	type Artist,
	type BrowseAPI,
	type Counts,
	type HistoryEntry,
	type LibraryAPI,
	type LibraryItem,
	type LyricsAPI,
	type PlayerAPI,
	type PlayerState,
	type Playlist,
	type QueueItem,
	type SearchAPI,
	type SourcesAPI,
	type SourceStatus,
	type Track
} from './api';
import { descriptorKey, fold, LIB_REF, LIBRARY_SOURCE, sameSong } from './identity';
import type { Descriptor } from './sources/contract';
import { emptySearch, mergeTop, type SearchAlbum, type SearchArtist, type SearchResult, type SearchSection, type TopPick } from './sources/search';

const ro = <T>(s: Readable<T>): Readable<T> => ({ subscribe: s.subscribe });

// ------------------------------------------------------------------------------------------------ what went wrong, in words

const problemStore = writable<{ text: string; n: number } | null>(null);
let problemCount = 0;
/** Something the listener asked for was refused or could not be done: the words to show (the shell toasts them). */
export const problems: Readable<{ text: string; n: number } | null> = ro(problemStore);
function report(e: unknown): void {
	const text = said(e);
	problemStore.set({ text: /[.!?]$/.test(text) ? text : `${text}.`, n: ++problemCount });
}

// ------------------------------------------------------------------------------------------------ songs

const descriptorOf = (title: string, artist: string, album: string | null | undefined, durationMs: number | null | undefined, isrc?: string | null): Descriptor => {
	const d: Descriptor = { title, artist };
	if (album) d.album = album;
	if (durationMs) d.durationMs = durationMs;
	if (isrc) d.isrc = isrc;
	return d;
};

/**
 * Songs of the library this window has seen, by row id: the player and the queue name a song only by its row, title and artist, so
 * its album and artist pages and its bit depth come from here when the window has met the song before. Bounded (the oldest go first).
 */
const SEEN_MAX = 20_000;
const seen = new Map<number, { item: LibraryItem; bitDepth: number | null }>();
function remember(id: number, item: LibraryItem, bitDepth: number | null): void {
	seen.delete(id);
	seen.set(id, { item, bitDepth });
	if (seen.size > SEEN_MAX) seen.delete(seen.keys().next().value as number);
}

const fromRow = new WeakMap<core.TrackRow, LibraryItem>();

/** A row of the library as the UI's song: `refs.lib` is its row id; its album and artist pages are the library's own. */
export function itemOfRow(r: core.TrackRow): LibraryItem {
	const had = fromRow.get(r);
	if (had) return had;
	const d = descriptorOf(r.title, r.artist, r.album, r.duration_ms, r.isrc);
	const item: LibraryItem = { ...d, key: descriptorKey(d), refs: { [LIB_REF]: String(r.id) }, addedAt: r.added_at_ms ?? 0 };
	const art = core.artUrl(r.art);
	if (art) item.artwork = [{ url: art }];
	if (r.album_id !== null && r.album_id !== undefined) item.albumRefs = { [LIBRARY_SOURCE]: String(r.album_id) };
	if (r.artist_id !== null && r.artist_id !== undefined) item.artistRefs = { [LIBRARY_SOURCE]: String(r.artist_id) };
	fromRow.set(r, item);
	remember(r.id, item, r.bit_depth ?? null);
	return item;
}

/** A track an add-on found: `refs[<add-on id>]` is the add-on's id for it. No album or artist page (add-ons have none here). */
function itemOfFound(addon: string, t: core.FoundTrack): LibraryItem {
	const d = descriptorOf(t.title, t.artist, t.album, t.duration_ms);
	const item: LibraryItem = { ...d, key: descriptorKey(d), refs: { [addon]: t.id }, addedAt: 0 };
	const art = core.artUrl(t.art);
	if (art) item.artwork = [{ url: art }];
	return item;
}

type Queued = Pick<core.QueueRow, 'id' | 'title' | 'artist' | 'album' | 'art' | 'duration_ms' | 'addon' | 'qid'>;

/** A song as the player or the queue names it. */
function itemOfQueued(q: Queued): LibraryItem {
	if (q.id !== null) {
		const known = seen.get(q.id);
		if (known) return known.item;
	}
	const d = descriptorOf(q.title, q.artist, q.album, q.duration_ms);
	const refs: Record<string, string> = {};
	if (q.id !== null) refs[LIB_REF] = String(q.id);
	if (q.addon && q.addon.addon !== LIB_REF) refs[q.addon.addon] = q.addon.track;
	const item: LibraryItem = { ...d, key: descriptorKey(d), refs, addedAt: 0 };
	const art = core.artUrl(q.art);
	if (art) item.artwork = [{ url: art }];
	return item;
}

/** The library's row of a song, when it is one. */
const libIdOf = (item: Pick<LibraryItem, 'refs'>): number | null => {
	const v = item.refs?.[LIB_REF];
	return v !== undefined && /^\d+$/.test(v) ? Number(v) : null;
};

/** The add-on (and its id for the track) of a song that is no row of the library: its first ref. */
function addonRefOf(item: LibraryItem): { addon: string; track: core.KeptTrack } | null {
	const e = Object.entries(item.refs ?? {}).find(([k]) => k !== LIB_REF);
	if (!e) return null;
	return { addon: e[0], track: { id: e[1], title: item.title, artist: item.artist, album: item.album ?? null, duration_ms: item.durationMs ?? null } };
}

/** How the player is told to play a song: by its row, else as its add-on's track. Null for a song that is neither. */
function entryOf(item: LibraryItem): core.PlayEntry | null {
	const id = libIdOf(item);
	if (id !== null) return { id };
	const a = addonRefOf(item);
	if (!a) return null;
	const art = item.artwork?.[0]?.url;
	return { addon: a.addon, track: art ? { ...a.track, art } : a.track };
}

const trackOfRow = (r: core.TrackRow): Track => {
	itemOfRow(r); // remembered, so the player knows the song's pages and bit depth
	const t: Track = { id: String(r.id), title: r.title, artist: r.artist };
	if (r.album) t.album = r.album;
	if (r.album_id !== null && r.album_id !== undefined) t.albumId = String(r.album_id);
	if (r.artist_id !== null && r.artist_id !== undefined) t.artistId = String(r.artist_id);
	if (r.duration_ms) t.durationMs = r.duration_ms;
	if (r.isrc) t.isrc = r.isrc;
	const art = core.artUrl(r.art);
	if (art) t.artwork = [{ url: art }];
	return t;
};

const PAGE = 500;
/** Every row of a list the core hands out in pages. */
async function allPages<T>(page: (offset: number, limit: number) => Promise<T[]>, max = Infinity): Promise<T[]> {
	const out: T[] = [];
	for (let offset = 0; offset < max; offset += PAGE) {
		const rows = await page(offset, PAGE);
		out.push(...rows);
		if (rows.length < PAGE) break;
	}
	return out;
}

// ------------------------------------------------------------------------------------------------ the player

/** How many songs after the one playing the window holds (the core's queue can be the whole library). */
const QUEUE_AHEAD = 200;
/** The window is read again when fewer than this many of the songs it holds are still to come (and the queue has more). */
const QUEUE_LOW = 50;

let heard: core.CorePlayer = core.IDLE;
let heardAt = 0;
let state: PlayerState = { ...INITIAL_PLAYER_STATE };
const playerState = writable<PlayerState>(state);

/** The part of the queue the window holds, and what it was read for. */
let held: { version: number; len: number; start: number; items: QueueItem[] } = { version: -1, len: -1, start: 0, items: [] };
let queueAsk = 0;

const identity = (t: Queued) => (t.id !== null ? `l${t.id}` : t.addon ? `a${t.addon.addon}:${t.addon.track}` : `t${t.title}|${t.artist}`);
/** The id of a place in the queue: the core's own when it gives one (it stays with the song when the queue is reordered), else the place and the song. */
const qidOf = (place: number, t: Queued) => (t.qid !== undefined && t.qid !== null ? `q${t.qid}` : `${place}:${identity(t)}`);

/** The volume the listener is setting: the core's own number follows a moment later, and an older answer must not move the slider back. */
let volumeHeld: { v: number; until: number } | null = null;

function position(): number {
	if (heard.status !== 'playing') return heard.position_ms;
	const p = heard.position_ms + (performance.now() - heardAt);
	return heard.duration_ms > 0 ? Math.min(heard.duration_ms, p) : p;
}

function publish(next: PlayerState): void {
	state = next;
	playerState.set(next);
}

const same = (a: unknown, b: unknown) => a === b || JSON.stringify(a) === JSON.stringify(b);

/** Takes in what the core says the player is doing. Things that did not change keep their objects, so the UI redraws only what did. */
function take(a: core.CorePlayer): void {
	const before = state.current?.qid;
	// A failure the core still remembers from before this window opened is not news: it is said when it changes.
	if (old === undefined) old = a.error;
	if (a.error !== old) old = null;
	heard = a;
	heardAt = performance.now();
	const index = a.track && a.index !== null ? a.index : -1;
	let current: QueueItem | null = null;
	if (a.track) {
		const qid = qidOf(index, a.track);
		current = state.current?.qid === qid ? state.current : (held.items[index - held.start]?.qid === qid ? held.items[index - held.start] : { ...itemOfQueued(a.track), qid });
	}
	const volume = volumeHeld && performance.now() < volumeHeld.until ? volumeHeld.v : a.volume / 100;
	publish({
		...state,
		current,
		index,
		queueLength: a.queue_len,
		playing: a.status === 'playing',
		buffering: a.status === 'loading',
		positionMs: position(),
		durationMs: a.duration_ms || a.track?.duration_ms || 0,
		volume,
		shuffle: a.shuffle,
		repeat: a.repeat,
		error: old !== null && a.error === old ? null : a.error,
		from: a.from,
		available: a.available,
		reason: a.reason ?? null,
		signal: same(state.signal, a.signal) ? state.signal : a.signal,
		sourceBitDepth: a.track?.id !== null && a.track?.id !== undefined ? (seen.get(a.track.id)?.bitDepth ?? null) : null,
		tier: a.tier,
		servedBy: a.served_by,
		exclusive: a.exclusive
	});
	readQueue(a);
	// A stream from an add-on the window has not heard of (added from somewhere else): the list is read again.
	if (a.served_by && a.served_by !== unknownAddon && !get(addonStore).some((x) => x.id === a.served_by)) {
		unknownAddon = a.served_by;
		void loadAddons();
	}
	// The core writes a play down when it counts: History follows a little after the song changed.
	if (current?.qid !== before) historySoon();
}
let unknownAddon: string | null = null;
/** The failure the core already had when the window first heard from it (undefined: not heard yet; null: none, or it has changed since). */
let old: string | null | undefined;

/** Reads the queue from the song playing on, when it changed or the part the window holds is running out. */
function readQueue(a: core.CorePlayer): void {
	const index = a.index ?? 0;
	const end = held.start + held.items.length;
	const changed = a.queue_version !== held.version || a.queue_len !== held.len;
	const outside = index < held.start || index >= end;
	const low = end < a.queue_len && end - index <= QUEUE_LOW;
	if (!changed && !(a.queue_len > 0 && (outside || low))) return;
	const mine = ++queueAsk;
	// Marked as read for now, so the answers that arrive meanwhile do not ask again.
	held = { ...held, version: a.queue_version, len: a.queue_len };
	if (a.queue_len === 0) {
		held = { version: a.queue_version, len: 0, start: 0, items: [] };
		publish({ ...state, queue: held.items, queueStart: 0 });
		return;
	}
	core
		.queueAt(index, QUEUE_AHEAD + 1)
		.then((rows) => {
			if (mine !== queueAsk) return;
			held = { version: a.queue_version, len: a.queue_len, start: index, items: rows.map((r) => ({ ...itemOfQueued(r), qid: qidOf(r.place, r) })) };
			publish({ ...state, queue: held.items, queueStart: held.start });
		})
		.catch(() => {
			if (mine === queueAsk) held = { ...held, version: -1 }; // asked again with the next answer
		});
}

let sent = 0;
let applied = 0;
/** Answers to requests sent before the listener's last command are not shown: they describe the player as it was. */
let barrier = 0;
async function hear(call: () => Promise<core.CorePlayer>): Promise<void> {
	const mine = ++sent;
	const a = await call();
	if (mine <= barrier || mine < applied) return;
	applied = mine;
	take(a);
}

let pollTimer: ReturnType<typeof setTimeout> | undefined;
let polling = false;
async function poll(): Promise<void> {
	clearTimeout(pollTimer);
	await hear(() => core.player()).catch(() => {});
	if (!polling) return;
	// Often while a stream is being asked for, twice a second while something plays or waits, now and then otherwise.
	const pause = heard.status === 'loading' ? 250 : heard.status !== 'idle' ? 500 : 1500;
	clearTimeout(pollTimer);
	pollTimer = setTimeout(() => void poll(), pause);
}
function pollSoon(): void {
	if (!polling) return;
	clearTimeout(pollTimer);
	pollTimer = setTimeout(() => void poll(), 150);
}

/** Commands reach the core in the order the listener gave them ("shuffle, then play"). */
let chain: Promise<void> = Promise.resolve();
function command(call: () => Promise<core.CorePlayer>): Promise<void> {
	barrier = sent;
	chain = chain
		.then(() => {
			barrier = sent;
			return hear(call);
		})
		.then(pollSoon, (e) => {
			if (e instanceof Refused) report(e);
		});
	return chain;
}

const entriesOf = (items: LibraryItem[]) => items.map(entryOf).filter((e): e is core.PlayEntry => e !== null);

const player: PlayerAPI = {
	state: ro(playerState),
	now: position,
	position,
	playItems(items, startIndex = 0, opts) {
		// A song that is neither a row of the library nor an add-on's track cannot be played: the start moves with what is left out.
		const start = items.slice(0, startIndex).filter((i) => entryOf(i) !== null).length;
		const entries = entriesOf(items);
		if (!entries.length) return Promise.resolve();
		return command(() => core.playEntries(entries, Math.min(start, entries.length - 1), opts?.from));
	},
	playLibrary: (offset, opts) => command(() => core.playLibrary(offset, opts?.from)),
	play: () => void command(() => core.setPaused(false)),
	pause: () => void command(() => core.setPaused(true)),
	toggle: () => {
		if (state.current) void command(() => core.setPaused(heard.status === 'playing'));
	},
	next: () => void command(() => core.nextTrack()),
	prev: () => void command(() => core.previousTrack()),
	seek(ms) {
		const to = Math.max(0, ms);
		// The bar moves at once; the core's answer follows.
		heard = { ...heard, position_ms: to };
		heardAt = performance.now();
		publish({ ...state, positionMs: to });
		void command(() => core.seekTo(to / 1000));
	},
	jumpTo: (i) => void command(() => core.jumpTo(i)),
	enqueue(items, opts) {
		const entries = entriesOf(items);
		if (entries.length) void command(() => core.enqueue(entries, opts?.next === true));
	},
	removeAt: (i) => void command(() => core.removeFromQueue(i)),
	move: (from, to) => void command(() => core.moveInQueue(from, to)),
	setShuffle: (on) => void command(() => core.setShuffle(on)),
	setRepeat: (m) => void command(() => core.setRepeat(m)),
	setVolume(v) {
		const to = Math.max(0, Math.min(1, v));
		volumeHeld = { v: to, until: performance.now() + 800 };
		publish({ ...state, volume: to });
		void command(() => core.setVolume(Math.round(to * 100)));
	},
	setExclusive: (on) => void command(() => core.setExclusive(on)),
	wholeQueue: async () => (await allPages((o, l) => core.queueAt(o, l))).map(itemOfQueued)
};

// ------------------------------------------------------------------------------------------------ the library

const likes = writable<LibraryItem[]>([]);
const playlists = writable<Playlist[]>([]);
const history = writable<HistoryEntry[]>([]);
const counts = writable<Counts | null>(null);
const loaded = writable(false);
const changed = writable(0);

const HISTORY_MAX = 500;

async function loadLikes(): Promise<void> {
	likes.set((await allPages((o, l) => core.likedAt(o, l))).map(itemOfRow));
}
async function loadHistory(): Promise<void> {
	history.set((await core.historyAt(0, HISTORY_MAX)).map((r) => ({ item: itemOfRow(r), playedAt: r.played_at_ms })));
}
let historyTimer: ReturnType<typeof setTimeout> | undefined;
function historySoon(): void {
	if (!polling) return;
	clearTimeout(historyTimer);
	historyTimer = setTimeout(() => void loadHistory().catch(() => {}), 4000);
}

/** The playlists as the core lists them; the songs of one are read again only when it changed (or `force` names it). One at a time. */
let playlistChain: Promise<void> = Promise.resolve();
function syncPlaylists(rows: core.PlaylistRow[], force: number[] = []): Promise<void> {
	playlistChain = playlistChain
		.then(async () => {
			const have = new Map(get(playlists).map((p) => [p.id, p]));
			const next = await Promise.all(
				rows.map(async (r): Promise<Playlist> => {
					const id = String(r.id);
					const old = have.get(id);
					const fresh = !old || force.includes(r.id) || old.items.length !== r.tracks || old.updatedAt !== r.updated_at_ms;
					const items = fresh ? (await allPages((o, l) => core.playlistTracksAt(r.id, o, l))).map(itemOfRow) : old.items;
					const p: Playlist = { id, name: r.name, items, createdAt: r.created_at_ms, updatedAt: r.updated_at_ms };
					if (r.followed) p.followed = true;
					return p;
				})
			);
			playlists.set(next);
		})
		.catch(() => {});
	return playlistChain;
}

/** Takes in the core's answer to a change: the counts and the playlists as they now are. */
function took(c: core.Changed, force: number[] = []): Promise<void> {
	counts.set(c.counts);
	return syncPlaylists(c.playlists, force);
}

async function loadLibrary(): Promise<void> {
	await Promise.all([
		core.counts().then((c) => counts.set(c)),
		loadLikes(),
		core.playlists().then((rows) => syncPlaylists(rows, rows.map((r) => r.id))),
		loadHistory()
	]);
}

/** The library's row of a song; an add-on's track is made a song of the library first. */
async function rowOf(item: LibraryItem): Promise<number> {
	const id = libIdOf(item);
	if (id !== null) return id;
	const a = addonRefOf(item);
	if (!a) throw new Refused('That song cannot be kept in your library', 400);
	const kept = await core.keepFound(a.addon, a.track);
	counts.set(kept.counts);
	if (kept.id === undefined) throw new Refused('That song was not kept in your library', 500);
	return kept.id;
}

const pid = (id: string) => Number(id);
/** A change shown ahead of the core's answer was refused: the playlist is read again as the core has it. */
const resync = (id: number): Promise<void> =>
	core
		.playlists()
		.then((rows) => syncPlaylists(rows, [id]))
		.catch(() => {});

/** Runs a change; a refusal is said (and the change's promise still rejects, for a caller that waits on it). */
async function change<T>(run: () => Promise<T>): Promise<T> {
	try {
		return await run();
	} catch (e) {
		report(e);
		throw e;
	}
}
/** A change nothing waits on: a refusal is said and that is all. */
const quiet = (p: Promise<unknown>): Promise<void> => p.then(() => {}, () => {});

const library: LibraryAPI = {
	likes: ro(likes),
	playlists: ro(playlists),
	history: ro(history),
	counts: ro(counts),
	loaded: ro(loaded),
	changed: ro(changed),
	like: (item) =>
		quiet(
			change(async () => {
				const id = await rowOf(item);
				const c = await core.likeTrack(id, true);
				const mine: LibraryItem = { ...item, refs: { ...item.refs, [LIB_REF]: String(id) } };
				likes.update((l) => [mine, ...l.filter((x) => libIdOf(x) !== id)]);
				await took(c);
				// The list here and the core's count disagree (liked from somewhere else meanwhile): the list is read again.
				if (get(likes).length !== c.counts.liked) await loadLikes();
			})
		),
	unlike: (item) =>
		quiet(
			change(async () => {
				const id = libIdOf(item) ?? libIdOf(get(likes).find((x) => sameSong(x, item)) ?? { refs: {} });
				if (id === null) return;
				const c = await core.likeTrack(id, false);
				likes.update((l) => l.filter((x) => libIdOf(x) !== id));
				await took(c);
				if (get(likes).length !== c.counts.liked) await loadLikes();
			})
		),
	createPlaylist: (name) =>
		change(async () => {
			const c = await core.createPlaylist(name.trim());
			await took(c);
			const made = get(playlists).find((p) => p.id === String(c.id));
			if (!made) throw new Refused('The playlist was not made', 500);
			return made;
		}),
	renamePlaylist: (id, name) => quiet(change(async () => took(await core.renamePlaylist(pid(id), name.trim())))),
	deletePlaylist: (id) => change(async () => took(await core.deletePlaylist(pid(id)))),
	addToPlaylist: (id, items) =>
		change(async () => {
			const ids: number[] = [];
			for (const it of items) ids.push(await rowOf(it));
			await took(await core.addToPlaylist(pid(id), ids), [pid(id)]);
		}),
	removeFromPlaylist: (id, index) =>
		quiet(
			change(async () => {
				playlists.update((l) => l.map((p) => (p.id === id ? { ...p, items: p.items.filter((_, i) => i !== index) } : p)));
				try {
					await took(await core.removeFromPlaylist(pid(id), index), [pid(id)]);
				} catch (e) {
					await resync(pid(id));
					throw e;
				}
			})
		),
	moveInPlaylist: (id, from, to) =>
		quiet(
			change(async () => {
				const p = get(playlists).find((x) => x.id === id);
				if (!p || from === to || from < 0 || to < 0 || from >= p.items.length || to >= p.items.length) return;
				// The row lands at once; the core's order follows.
				const items = p.items.slice();
				items.splice(to, 0, ...items.splice(from, 1));
				playlists.update((l) => l.map((x) => (x.id === id ? { ...x, items } : x)));
				try {
					await took(await core.moveInPlaylist(pid(id), from, to), [pid(id)]);
				} catch (e) {
					await resync(pid(id));
					throw e;
				}
			})
		),
	reload: async () => {
		await loadLibrary().catch(() => {});
		changed.update((n) => n + 1);
	}
};

// ------------------------------------------------------------------------------------------------ add-ons

const addonStore = writable<SourceStatus[]>([]);
async function loadAddons(): Promise<void> {
	addonStore.set(await core.addons());
}

const sources: SourcesAPI = {
	status: ro(addonStore),
	reload: () => loadAddons(),
	add: async (link) => addonStore.set(await core.addAddon(link)),
	remove: async (id) => addonStore.set(await core.removeAddon(id)),
	setEnabled: async (id, on) => addonStore.set(await core.setAddonEnabled(id, on)),
	order: async (ids) => addonStore.set(await core.orderAddons(ids))
};

// ------------------------------------------------------------------------------------------------ search

const LIBRARY_RESULTS = 50;
const ADDON_RESULTS = 25;
const TIERS: Record<string, string> = { low: 'Low', high: 'High', lossless: 'Lossless', hires: 'Hi-res' };

const search: SearchAPI = {
	async search(q, opts) {
		const query = q.trim();
		if (!query) return emptySearch();
		const here = await core.searchLibrary(query, LIBRARY_RESULTS, opts?.signal);
		const lib: SearchSection = { sourceId: LIB_REF, name: 'Your library', items: here.rows.map(itemOfRow) };
		const artists: SearchArtist[] = here.artists.map((a) => ({ sourceId: LIBRARY_SOURCE, id: String(a.id), name: a.name, ...(artOf(a.art) ? { artwork: artOf(a.art) } : {}) }));
		const albums: SearchAlbum[] = here.albums.map((a) => ({ sourceId: LIBRARY_SOURCE, id: String(a.id), title: a.title, artist: a.artist, ...(a.year ? { year: a.year } : {}), ...(artOf(a.art) ? { artwork: artOf(a.art) } : {}) }));
		// The card at the top is the artist or the album whose name is what was typed; else the first song.
		// ("lanterns" names The Lanterns: a leading article is not part of what is typed.)
		const bare = (s: string) => fold(s).replace(/^(?:the|a|an)\s+(?=\S)/, '');
		const typed = bare(query);
		const named = artists.find((a) => bare(a.name) === typed);
		const titled = albums.find((a) => bare(a.title) === typed);
		const topPick: TopPick = named ? { kind: 'artist', artist: named } : titled ? { kind: 'album', album: titled } : { kind: 'track' };
		const found = { artists, albums, topPick };
		// The add-ons a search asks: switched on, usable, and offering search. In the listener's order.
		const asked = get(addonStore).filter((a) => a.enabled && !a.problem && a.resources.includes('search'));
		if (!asked.length) return { top: mergeTop([lib]), ...found, bySource: [lib], failed: [], pending: [] };
		opts?.onLibrary?.({ top: mergeTop([lib]), ...found, bySource: [lib], failed: [], pending: asked.map((a) => a.name) });

		const failed: SearchResult['failed'] = [];
		const sections: (SearchSection | null)[] = await Promise.all(
			asked.map(async (a): Promise<SearchSection | null> => {
				try {
					const g = await core.searchAddon(a.id, query, ADDON_RESULTS, opts?.signal);
					if (!g || g.error) {
						failed.push({ sourceId: a.id, name: a.name, message: g?.error?.message ?? 'no answer' });
						return null;
					}
					const tags: Record<string, string> = {};
					for (const t of g.tracks) if (t.max_tier && TIERS[t.max_tier]) tags[t.id] = TIERS[t.max_tier];
					return { sourceId: a.id, name: a.name, items: g.tracks.map((t) => itemOfFound(a.id, t)), tags };
				} catch (e) {
					if (opts?.signal?.aborted) throw e;
					failed.push({ sourceId: a.id, name: a.name, message: said(e) });
					return null;
				}
			})
		);
		const bySource = [lib, ...sections.filter((s): s is SearchSection => s !== null)];
		// In the add-ons' own order, whichever answered first.
		failed.sort((x, y) => asked.findIndex((a) => a.id === x.sourceId) - asked.findIndex((a) => a.id === y.sourceId));
		return { top: mergeTop(bySource), ...found, bySource, failed, pending: [] };
	}
};

// ------------------------------------------------------------------------------------------------ album and artist pages

const numbered = (sourceId: string, id: string, what: string): number => {
	if (sourceId !== LIBRARY_SOURCE || !/^\d+$/.test(id)) throw new Refused(`That ${what} is not in your library`, 404);
	return Number(id);
};
const artOf = (art: string | null) => {
	const url = core.artUrl(art);
	return url ? [{ url }] : undefined;
};

const browse: BrowseAPI = {
	async album(sourceId, id) {
		const a = await core.album(numbered(sourceId, id, 'album'));
		const out: Album = { id, title: a.album.title, artist: a.album.artist, tracks: a.rows.map(trackOfRow) };
		if (a.album.year) out.year = a.album.year;
		const art = artOf(a.album.art);
		if (art) out.artwork = art;
		return out;
	},
	async artist(sourceId, id) {
		const a = await core.artist(numbered(sourceId, id, 'artist'));
		const out: Artist = {
			id,
			name: a.artist.name,
			topTracks: a.rows.map(trackOfRow),
			albums: a.albums.map((al) => ({ id: String(al.id), title: al.title, ...(al.year ? { year: al.year } : {}), ...(artOf(al.art) ? { artwork: artOf(al.art) } : {}), trackCount: al.tracks }))
		};
		const art = artOf(a.artist.art);
		if (art) out.artwork = art;
		return out;
	}
};

// ------------------------------------------------------------------------------------------------ lyrics

/**
 * The core finds them: a file's own (.lrc beside it, or its tags), then the add-ons that have lyrics (the one serving the song first),
 * then LRCLIB unless switched off in Settings. What came from the network the core keeps, so a song is asked for once.
 */
const lyrics: LyricsAPI = {
	async forTrack(item) {
		const id = libIdOf(item);
		if (id !== null) return (await core.lyrics({ id })).lyrics;
		const ref = addonRefOf(item);
		const found = await core.lyrics({ title: item.title, artist: item.artist, album: item.album, durationMs: item.durationMs, isrc: item.isrc, addon: ref?.addon, track: ref?.track.id });
		return found.lyrics;
	}
};

// ------------------------------------------------------------------------------------------------ start

export const app: AppAPI = { player, search, browse, library, lyrics, sources };
export { player, browse, library, lyrics, sources };
export const searchApi = search;

/** Starts asking the core: the player (and keeps asking), the library's lists, the add-ons. Not while the window measures itself. */
function start(): void {
	polling = true;
	void poll();
	// While something plays the position runs on between the core's reports, so the clock and the lyrics move smoothly.
	setInterval(() => {
		if (heard.status === 'playing') publish({ ...state, positionMs: position() });
	}, 250);
	void loadAddons().catch(() => {});
	void loadLibrary()
		.catch(() => {})
		.then(() => loaded.set(true));
}

if (typeof window !== 'undefined' && !core.measuring) start();
