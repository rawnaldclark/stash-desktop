/**
 * Reactive views of the APIs for components (runes-friendly), plus the UI's own state (panel, toast, menu).
 * Components read `S.v` (PlayerState), `L.likes` etc.; they never touch audio or storage.
 */
import { fromStore, get } from 'svelte/store';
import { library, player, sources as sourcesApi } from '../app';
import { LIB_REF, songIndex } from '../identity';
import type { Counts, Descriptor, HistoryEntry, LibraryItem, Playlist, SourceStatus } from '../api';
import type { PlayerState } from '../api';

/**
 * The player state, with the position split from the rest. A fresh state object is published about four times a second while a
 * song plays (only `positionMs` differs); if every reader of `S.v` re-ran on each, a few ms of main thread a second would go to
 * re-deriving things that did not change. `rest` is replaced only when something other than the position changed; `pos` is its own signal.
 */
let rest = $state.raw<PlayerState>(get(player.state));
let pos = $state(rest.positionMs);
function sameButPosition(a: PlayerState, b: PlayerState): boolean {
	for (const k of Object.keys(b) as (keyof PlayerState)[]) if (k !== 'positionMs' && a[k] !== b[k]) return false;
	return Object.keys(a).length === Object.keys(b).length;
}
player.state.subscribe((v) => {
	pos = v.positionMs;
	if (!sameButPosition(rest, v)) rest = v;
});
const view = new Proxy({} as PlayerState, {
	get: (_t, k) => (k === 'positionMs' ? pos : rest[k as keyof PlayerState]),
	has: (_t, k) => k in rest,
	ownKeys: () => Reflect.ownKeys(rest),
	getOwnPropertyDescriptor: (_t, k) => ({ value: k === 'positionMs' ? pos : rest[k as keyof PlayerState], enumerable: true, configurable: true })
});
export const S = {
	get v(): PlayerState {
		return view;
	}
};

const likes = fromStore<LibraryItem[]>(library.likes);
const playlists = fromStore<Playlist[]>(library.playlists);
const history = fromStore<HistoryEntry[]>(library.history);
const counts = fromStore<Counts | null>(library.counts);
const loaded = fromStore<boolean>(library.loaded);
const changed = fromStore<number>(library.changed);
const sources = fromStore<SourceStatus[]>(sourcesApi.status);

class Lib {
	get likes() {
		return likes.current;
	}
	get playlists() {
		return playlists.current;
	}
	get history() {
		return history.current;
	}
	/** How much the library holds; null until the core has answered. */
	get counts() {
		return counts.current;
	}
	/** The likes, the playlists and the history have been read once. */
	get loaded() {
		return loaded.current;
	}
	/** Goes up when the library changed under the lists (an import, a scan). */
	get changed() {
		return changed.current;
	}
	/** The listener's add-ons, in the order they are asked. */
	get sources() {
		return sources.current;
	}
	/** The liked rows of the library: a song of the library is liked by its row (the same song can be there as two files). */
	likedRows = $derived(new Set(likes.current.map((i) => i.refs[LIB_REF])));
	/** The likes by `sameSong`, for a song that is no row of the library (an add-on's track): liked when the same song is. */
	likedIndex = $derived(songIndex(likes.current.map((i) => [i, true] as const)));
	isLiked(d: Descriptor & { key?: string; refs?: Record<string, string> }) {
		const row = d.refs?.[LIB_REF];
		return row !== undefined ? this.likedRows.has(row) : this.likedIndex.has(d);
	}
	toggleLike(item: LibraryItem) {
		return this.isLiked(item) ? library.unlike(item) : library.like(item);
	}
}
export const L = new Lib();

/** `group`: the menu is for a set of songs (a Home album card): queue, playlist and like act on all of them, `item` (the first) names the artist and album. */
export type MenuState = { x: number; y: number; item: LibraryItem; group?: LibraryItem[]; queueIndex?: number; queueLength?: number; playlistId?: string; playlistIndex?: number; playlistLength?: number; /** On Library › Downloads: the song is saved, or on its way. */ download?: 'saved' | 'waiting'; anchor?: HTMLElement | null };

const LS = {
	get(k: string): string | null {
		try {
			return localStorage.getItem(k);
		} catch {
			return null;
		}
	},
	set(k: string, v: string) {
		try {
			localStorage.setItem(k, v);
		} catch {
			/* private mode: fine */
		}
	},
	del(k: string) {
		try {
			localStorage.removeItem(k);
		} catch {
			/* private mode: fine */
		}
	}
};
export { LS };

class UIState {
	/** Window size in px (set by the shell's ResizeObserver; the shell is the whole viewport). */
	w = $state(typeof window === 'undefined' ? 1200 : window.innerWidth);
	h = $state(typeof window === 'undefined' ? 800 : window.innerHeight);
	panelOpen = $state(true);
	/** The command palette (Ctrl+K) is open. */
	palette = $state(false);
	/** The smart playlist editor is open: on a new one (`id` null), or on the playlist `id`. */
	smart = $state<{ id: string | null } | null>(null);
	/** The desktop has no lyrics yet, so the panel opens on Up next unless the listener chose Lyrics. */
	panelTab = $state<'lyrics' | 'next'>(LS.get('stash.panelTab') === 'lyrics' ? 'lyrics' : 'next');
	/** On the Now playing route (the big lyrics). */
	np = $state(false);
	/** The Up next sheet (Now playing below 900 px). */
	sheet = $state(false);
	reduced = $state(false);
	/** The system's colour scheme (the layout keeps it current). */
	systemLight = $state(false);
	/** Settings > Appearance's theme, as app.html's head script put it on <html> (Settings changes both through theme.ts). */
	theme = $state<'system' | 'dark' | 'light'>((typeof document === 'undefined' ? undefined : (document.documentElement.dataset.theme as 'dark' | 'light')) ?? 'system');
	/** Light is showing (picked, or the system's when following it). */
	get light() {
		return this.theme === 'light' || (this.theme === 'system' && this.systemLight);
	}
	toastText = $state<string | null>(null);
	/** A button on the message, when there is something to do about it. */
	toastAction = $state<{ label: string; run: () => void } | null>(null);
	menu = $state<MenuState | null>(null);
	/** The text in the header search box (shared with the Search page). */
	q = $state('');
	private toastTimer: ReturnType<typeof setTimeout> | undefined;

	/** The desktop has two sizes: L, and M (the rail) for every window under 1100 px. */
	get size(): 'm' | 'l' {
		return this.w < 1100 ? 'm' : 'l';
	}
	get twoCol() {
		return this.w >= 900;
	}
	/** Up next opens as a bottom sheet instead of docking. */
	get sheetMode() {
		return this.np && this.w < 900;
	}
	toggleNext() {
		if (this.sheetMode) this.sheet = !this.sheet;
		else this.toggleTab('next');
	}
	setTab(t: 'lyrics' | 'next') {
		this.panelTab = t;
		this.panelOpen = true;
		LS.set('stash.panelTab', t);
	}
	toggleTab(t: 'lyrics' | 'next') {
		if (this.panelOpen && this.panelTab === t) this.panelOpen = false;
		else this.setTab(t);
	}
	toast(text: string, action: { label: string; run: () => void } | null = null) {
		this.toastText = text;
		this.toastAction = action;
		clearTimeout(this.toastTimer);
		// A message with something to do stays longer.
		this.toastTimer = setTimeout(() => {
			this.toastText = null;
			this.toastAction = null;
		}, action ? 7000 : 3800);
	}
	openMenu(m: MenuState) {
		this.menu = m;
	}
	closeMenu() {
		this.menu = null;
	}
}
export const ui = new UIState();

/**
 * Recent searches (UI-only convenience, kept in this window; at most 10, newest first): a typed query, and a song you played
 * from search.
 */
export type RecentEntry = { kind: 'query'; q: string } | { kind: 'song'; item: LibraryItem };

export const RECENT_MAX = 10;
const RECENT_KEY = 'stash.recent2';

export const recentId = (e: RecentEntry): string => (e.kind === 'query' ? `q:${e.q.toLowerCase()}` : `s:${e.item.key}`);

const str = (v: unknown, max = 300): v is string => typeof v === 'string' && v.length > 0 && v.length <= max;

function readRecent(): RecentEntry[] {
	try {
		const v = JSON.parse(LS.get(RECENT_KEY) ?? '[]');
		if (!Array.isArray(v)) return [];
		const out: RecentEntry[] = [];
		for (const e of v) {
			if (typeof e !== 'object' || e === null) continue;
			if (e.kind === 'query' && str(e.q)) out.push({ kind: 'query', q: e.q });
			else if (e.kind === 'song' && e.item && str(e.item.key) && str(e.item.title) && str(e.item.artist) && typeof e.item.refs === 'object' && e.item.refs) out.push({ kind: 'song', item: e.item });
		}
		return out.slice(0, RECENT_MAX);
	} catch {
		return [];
	}
}
export const loadRecent = (): RecentEntry[] => readRecent();

function writeRecent(list: RecentEntry[]) {
	LS.set(RECENT_KEY, JSON.stringify(list.slice(0, RECENT_MAX)));
}
function pushRecent(e: RecentEntry) {
	writeRecent([e, ...readRecent().filter((x) => recentId(x) !== recentId(e))]);
}
export function saveRecent(q: string) {
	const t = q.trim();
	if (!t) return;
	// Search runs as you type, so "ric" then "rick ast" then "rick astley" all answer. One typing session is one entry: a new query
	// that continues (or backs up) the newest entry replaces it instead of piling prefixes into the list.
	const list = readRecent();
	const head = list[0];
	const a = t.toLowerCase();
	const b = head?.kind === 'query' ? head.q.toLowerCase() : null;
	const rest = b !== null && b !== a && (a.startsWith(b) || b.startsWith(a)) ? list.slice(1) : list;
	writeRecent([{ kind: 'query', q: t }, ...rest.filter((x) => recentId(x) !== `q:${a}`)]);
}
export function saveRecentSong(item: LibraryItem) {
	// Keep it small: one picture, no history of where else it lives.
	pushRecent({ kind: 'song', item: { ...item, artwork: item.artwork?.slice(0, 1) } });
}
export function removeRecent(e: RecentEntry) {
	writeRecent(readRecent().filter((x) => recentId(x) !== recentId(e)));
}
export function clearRecent() {
	writeRecent([]);
}
