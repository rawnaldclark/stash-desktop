/**
 * The one way a song's menu opens, for every song row and card (search, album, artist, playlist, library, history, Up next, the
 * Top result, Home): the "more" button, a right-click, the keyboard's menu key (or Shift+F10), and a long press on a touch screen.
 * The menu itself is `ContextMenu.svelte` (loaded on first use).
 */
import type { LibraryItem } from '../api';
import { ui, type MenuState } from './state.svelte';

export type MenuArgs = { item: LibraryItem; extra?: Partial<MenuState> };
/** A menu, or one whose songs are fetched when it opens (a Home catalog album: its songs are asked of the source then). */
export type SongMenuArgs = MenuArgs | { load: () => Promise<MenuArgs> } | null | undefined;

/** How long a finger rests on a row before the menu opens (Android's long press is about this), and how far it may drift. */
export const LONG_PRESS_MS = 500;
export const LONG_PRESS_SLOP = 10;

/** Opens the menu below a button's right edge (focus goes back there when it closes), or at a point. */
export function openSongMenu(a: NonNullable<SongMenuArgs>, at: HTMLElement | { x: number; y: number }) {
	const btn = at instanceof HTMLElement ? at : null;
	const r = btn?.getBoundingClientRect();
	const p = r ? { x: r.right, y: r.bottom } : (at as { x: number; y: number });
	if ('load' in a) {
		btn?.setAttribute('aria-busy', 'true');
		void a
			.load()
			.then((m) => ui.openMenu({ ...m.extra, x: p.x, y: p.y, item: m.item, anchor: btn }))
			.catch(() => ui.toast("Couldn't open that album's songs. Try again in a moment."))
			.finally(() => btn?.removeAttribute('aria-busy'));
		return;
	}
	ui.openMenu({ ...a.extra, x: p.x, y: p.y, item: a.item, anchor: btn });
}

/** The click a finger's lift sends after a long press is not a tap on the row (or on whatever the menu now covers). */
function swallowNextClick() {
	const off = () => {
		removeEventListener('click', eat, true);
		removeEventListener('pointerdown', off, true);
	};
	const eat = (e: Event) => {
		e.preventDefault();
		e.stopPropagation();
		off();
		// The lift's tap may have focused a link in the row: the menu keeps focus.
		document.querySelector<HTMLElement>('.menu [role="menuitem"]')?.focus();
	};
	addEventListener('click', eat, true);
	addEventListener('pointerdown', off, true);
	setTimeout(off, 1500);
}

/** `use:songMenu={{ item, extra }}` on a row: right-click, the menu key and a long press open the song menu. Null does nothing. */
export function songMenu(node: HTMLElement, initial: SongMenuArgs) {
	let args = initial;
	let timer: ReturnType<typeof setTimeout> | undefined;
	let x0 = 0;
	let y0 = 0;
	/** A long press opened the menu during this touch (Android then sends a contextmenu as well: that one is a no-op). */
	let pressed = false;

	const stop = () => {
		clearTimeout(timer);
		timer = undefined;
	};
	function down(e: PointerEvent) {
		pressed = false;
		if (!args || e.pointerType !== 'touch' || !e.isPrimary || (e.target as Element).closest('.rd-h')) return;
		x0 = e.clientX;
		y0 = e.clientY;
		stop();
		timer = setTimeout(() => {
			timer = undefined;
			if (!args) return;
			pressed = true;
			swallowNextClick();
			navigator.vibrate?.(8);
			openSongMenu(args, { x: x0, y: y0 });
		}, LONG_PRESS_MS);
	}
	function move(e: PointerEvent) {
		if (timer && Math.hypot(e.clientX - x0, e.clientY - y0) > LONG_PRESS_SLOP) stop();
	}
	function context(e: MouseEvent) {
		if (!args) return;
		e.preventDefault();
		const touch = timer !== undefined;
		stop();
		if (pressed) return;
		if (touch) {
			pressed = true;
			swallowNextClick();
		}
		// The menu key and Shift+F10 carry no pointer position: open at the row's "more" button (or the row) and return focus there.
		const keyboard = !touch && ((e.clientX === 0 && e.clientY === 0) || (e.button !== 2 && !e.ctrlKey && node.contains(document.activeElement)));
		const btn = node.querySelector<HTMLElement>('.more') ?? node;
		openSongMenu(args, keyboard ? btn : { x: e.clientX, y: e.clientY });
	}

	node.addEventListener('pointerdown', down);
	node.addEventListener('pointermove', move);
	node.addEventListener('pointerup', stop);
	node.addEventListener('pointercancel', stop);
	node.addEventListener('contextmenu', context);
	return {
		update(next: SongMenuArgs) {
			args = next;
		},
		destroy() {
			stop();
			node.removeEventListener('pointerdown', down);
			node.removeEventListener('pointermove', move);
			node.removeEventListener('pointerup', stop);
			node.removeEventListener('pointercancel', stop);
			node.removeEventListener('contextmenu', context);
		}
	};
}
