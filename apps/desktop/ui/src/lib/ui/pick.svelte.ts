/**
 * Songs picked in a list, to act on them together: Ctrl+click (Cmd+click on a Mac) picks a song or lets it go, Shift+click
 * picks every song from the last one clicked, Ctrl+A picks the whole list and Escape lets them go. A plain click still plays
 * the song. The song menu of a picked song, and the bar shown while songs are picked (`PickBar.svelte`), act on all of them.
 *
 * Songs are picked in one list at a time. A list says how to read its songs (`PickList`): a long one (Library › Songs) reads
 * them from the core, so a pick can run past the rows on screen.
 */
import type { LibraryItem } from '../api';
import { sameCopy } from '../identity';
import type { MenuState } from './state.svelte';
import { ui } from './state.svelte';

/** A list whose songs can be picked. */
export type PickList = {
	/** Names the list on its page: songs picked in one list are not mixed with another's. */
	readonly scope: string;
	/** How many songs the list has now. */
	count(): number;
	/** The songs from place `a` to place `b`, both included, in the list's order. */
	between(a: number, b: number): LibraryItem[] | Promise<LibraryItem[]>;
	/** What the queue says the picked songs play from when they are played ("Your likes", a playlist's name). */
	readonly from?: string;
	/** What the song menu is given besides the songs, by their places (a playlist's id, so they can be taken out of it). */
	extra?(places: number[]): Partial<MenuState>;
};

/** The most songs picked at once. */
export const PICK_MOST = 5000;

/** Which list each drawn row is in, for Ctrl+A on a row that has the focus. */
export const rowLists = new WeakMap<Element, PickList>();

class Picks {
	/** The list the songs are picked in. */
	list = $state.raw<PickList | null>(null);
	/** The picked songs by their place in the list: a new map on every change. */
	at = $state.raw(new Map<number, LibraryItem>());
	/** Where Shift+click starts: the last song picked or let go with a Ctrl+click (or the first Shift+click). */
	private anchor: number | null = null;
	/** Counts the reads of a range, so an older one that ends after a newer one is dropped. */
	private asked = 0;

	get count(): number {
		return this.at.size;
	}

	/** Whether the song at this place of the list is picked (and still the same song: the list may have changed). */
	has(list: PickList, place: number, item: LibraryItem): boolean {
		if (this.list?.scope !== list.scope) return false;
		const it = this.at.get(place);
		return !!it && sameCopy(it, item);
	}

	/** The picked songs in the list's order, with their places. */
	sorted(): { places: number[]; items: LibraryItem[] } {
		const entries = [...this.at.entries()].sort((x, y) => x[0] - y[0]);
		return { places: entries.map((e) => e[0]), items: entries.map((e) => e[1]) };
	}

	/** Lets every picked song go. */
	clear(): void {
		this.asked++;
		this.anchor = null;
		if (this.list === null && this.at.size === 0) return;
		this.list = null;
		this.at = new Map();
	}

	/** Lets the songs go when they were picked in this list (its songs changed, so their places did too). */
	forget(scope: string): void {
		if (this.list?.scope === scope) this.clear();
	}

	/** Ctrl+click: picks the song, or lets it go. */
	toggle(list: PickList, place: number, item: LibraryItem): void {
		this.asked++;
		const next = this.list?.scope === list.scope ? new Map(this.at) : new Map<number, LibraryItem>();
		if (next.has(place)) next.delete(place);
		else if (next.size >= PICK_MOST) return void ui.toast(`Up to ${PICK_MOST.toLocaleString()} songs at a time`);
		else next.set(place, item);
		this.anchor = place;
		this.set(list, next);
	}

	/** Shift+click: picks every song from where the last Ctrl+click was to this one. `add` (Ctrl+Shift+click) keeps the
	 * songs picked already. */
	async range(list: PickList, place: number, add: boolean): Promise<void> {
		const same = this.list?.scope === list.scope;
		const from = same && this.anchor !== null ? this.anchor : place;
		this.anchor = from;
		const first = Math.min(from, place);
		const last = Math.max(from, place);
		await this.read(list, first, last, add && same ? new Map(this.at) : new Map());
	}

	/** Ctrl+A: picks the whole list. */
	async all(list: PickList): Promise<void> {
		const n = list.count();
		if (n <= 0) return;
		this.anchor = 0;
		await this.read(list, 0, n - 1, new Map());
	}

	private async read(list: PickList, first: number, last: number, next: Map<number, LibraryItem>): Promise<void> {
		const asked = ++this.asked;
		const room = PICK_MOST - next.size;
		const end = Math.min(last, first + Math.max(room, 0) - 1);
		let items: LibraryItem[] = [];
		if (end >= first) {
			try {
				items = await list.between(first, end);
			} catch {
				if (asked === this.asked) ui.toast('Those songs could not be read. Try again in a moment.');
				return;
			}
		}
		if (asked !== this.asked) return;
		items.forEach((it, i) => next.set(first + i, it));
		if (end < last) ui.toast(`Up to ${PICK_MOST.toLocaleString()} songs at a time`);
		this.set(list, next);
	}

	private set(list: PickList, next: Map<number, LibraryItem>): void {
		this.list = next.size ? list : null;
		this.at = next;
	}
}

export const P = new Picks();

/** The menu for the picked songs: a group when there are two or more, else the one song with what its list gives it. */
export function pickedMenu(): { item: LibraryItem; extra: Partial<MenuState> } | null {
	const list = P.list;
	if (!list || P.count === 0) return null;
	const { places, items } = P.sorted();
	const extra = { ...list.extra?.(places), picks: places };
	return items.length > 1 ? { item: items[0], extra: { ...extra, group: items } } : { item: items[0], extra };
}
