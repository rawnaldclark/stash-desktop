/**
 * Drag-and-drop reordering for a vertical list of rows (Up next, playlists), as a Svelte action.
 *
 * - The row is picked up by its HANDLE only (pointer events: mouse, touch and pen alike; the handle has `touch-action: none`), so a swipe
 *   or flick anywhere else on the row scrolls the page and never starts a drag, a play or a remove.
 * - Nothing moves until the pointer has travelled a few pixels; a tap on the handle does nothing, and the click that follows a drag is swallowed.
 * - The dragged row follows the pointer; the rows it passes slide aside (transforms only, compositor-friendly). The list is not touched
 *   until the drop, when `onmove(from, to)` is called once with row positions inside the list.
 * - Near the top or bottom edge of the scrolling ancestor the list scrolls by itself, faster the closer the pointer is to the edge.
 * - Escape (or a cancelled pointer) puts everything back with no change.
 * - Keyboard: ArrowUp / ArrowDown on a focused handle moves that row by one (the menus have Move up / Move down too).
 */
import { tick } from 'svelte';

export interface ReorderOptions {
	/** Selector of the rows (children of the node), in list order. */
	row: string;
	/** Selector of the drag handle inside a row. */
	handle: string;
	/** Called once per completed move; `from` and `to` are positions among the rows. */
	onmove: (from: number, to: number) => void;
	disabled?: boolean;
}

/** Pixels the pointer must travel before a press on the handle becomes a drag. */
export const DRAG_SLOP = 4;
/** Distance from the scroller's edge where autoscroll starts, and its top speed in px per frame. */
export const EDGE = 64;
export const MAX_SPEED = 18;

/**
 * Where the dragged row lands: its centre against the midpoints of the others. `mids` and `centre` share one coordinate space.
 * The row goes before every row whose midpoint it has passed upward, after every one it has passed downward.
 */
export function targetIndex(mids: number[], from: number, centre: number): number {
	let to = from;
	for (let j = from - 1; j >= 0; j--) if (centre < mids[j]!) to = j;
	for (let j = from + 1; j < mids.length; j++) if (centre > mids[j]!) to = j;
	return to;
}

/** Autoscroll speed (px per frame, signed) for a pointer at `y` with the scroller spanning [top, bottom]. */
export function edgeSpeed(y: number, top: number, bottom: number): number {
	if (y < top + EDGE) return -MAX_SPEED * Math.min(1, (top + EDGE - y) / EDGE);
	if (y > bottom - EDGE) return MAX_SPEED * Math.min(1, (y - (bottom - EDGE)) / EDGE);
	return 0;
}

function scrollParent(el: HTMLElement): HTMLElement {
	for (let p = el.parentElement; p; p = p.parentElement) {
		const oy = getComputedStyle(p).overflowY;
		if ((oy === 'auto' || oy === 'scroll') && p.scrollHeight > p.clientHeight) return p;
	}
	return (document.scrollingElement as HTMLElement) ?? document.documentElement;
}

export function reorder(node: HTMLElement, initial: ReorderOptions) {
	let o = initial;
	let cleanup: (() => void) | null = null;

	const rowsOf = () => [...node.querySelectorAll<HTMLElement>(o.row)];

	function start(e: PointerEvent) {
		if (o.disabled || cleanup || (e.pointerType === 'mouse' && e.button !== 0)) return;
		const h = (e.target as Element | null)?.closest<HTMLElement>(o.handle);
		if (!h || !node.contains(h)) return;
		const rows = rowsOf();
		const row = h.closest<HTMLElement>(o.row);
		const from = row ? rows.indexOf(row) : -1;
		if (!row || from < 0) return;
		e.preventDefault(); // no text selection, no focus jump, no emulated mouse events
		const scroller = scrollParent(node);
		const startScroll = scroller.scrollTop;
		const startY = e.clientY;
		// Row geometry in content coordinates (viewport top + scrollTop), measured once: nothing else moves while dragging.
		const geo = rows.map((r) => {
			const b = r.getBoundingClientRect();
			return { top: b.top + startScroll, h: b.height };
		});
		const mids = geo.map((g) => g.top + g.h / 2);
		const size = geo[from]!.h;
		let y = startY;
		let active = false;
		let to = from;
		let raf = 0;
		const sRect = () => (scroller === document.scrollingElement ? { top: 0, bottom: innerHeight } : scroller.getBoundingClientRect());

		const paint = () => {
			const off = y - startY + (scroller.scrollTop - startScroll);
			row.style.transform = `translate3d(0, ${off}px, 0)`;
			to = targetIndex(mids, from, mids[from]! + off);
			rows.forEach((r, j) => {
				if (j === from) return;
				const shift = to > from && j > from && j <= to ? -size : to < from && j < from && j >= to ? size : 0;
				r.style.transform = shift ? `translate3d(0, ${shift}px, 0)` : '';
			});
		};
		const loop = () => {
			raf = 0;
			if (!active) return;
			const r = sRect();
			const v = edgeSpeed(y, r.top, r.bottom);
			if (v) scroller.scrollTop += v;
			paint();
			raf = requestAnimationFrame(loop);
		};
		const begin = () => {
			active = true;
			row.classList.add('rd-drag');
			node.classList.add('rd-active');
			raf = requestAnimationFrame(loop);
		};
		const clear = () => {
			cancelAnimationFrame(raf);
			node.classList.remove('rd-active');
			for (const r of rows) {
				r.classList.remove('rd-drag');
				r.style.transform = '';
			}
		};
		const onMove = (ev: PointerEvent) => {
			if (ev.pointerId !== e.pointerId) return;
			y = ev.clientY;
			if (!active && Math.abs(y - startY) > DRAG_SLOP) begin();
			if (active) {
				ev.preventDefault();
				paint();
			}
		};
		const finish = (commit: boolean) => {
			const wasActive = active;
			active = false;
			h.removeEventListener('pointermove', onMove);
			h.removeEventListener('pointerup', onUp);
			h.removeEventListener('pointercancel', onCancel);
			h.removeEventListener('lostpointercapture', onCancel);
			window.removeEventListener('keydown', onKey, true);
			try {
				h.releasePointerCapture(e.pointerId);
			} catch {
				/* already released */
			}
			if (wasActive) {
				// The click that follows a drag is not a press on anything.
				const swallow = (ev: Event) => ev.stopPropagation();
				window.addEventListener('click', swallow, { capture: true, once: true });
				setTimeout(() => window.removeEventListener('click', swallow, true), 0);
			}
			const move = commit && wasActive && to !== from ? to : -1;
			if (move >= 0) o.onmove(from, move);
			// Rows that survive the re-render drop their transforms only after it, so nothing flashes back to the old place.
			if (move >= 0) void tick().then(clear);
			else clear();
			cleanup = null;
		};
		const onUp = (ev: PointerEvent) => ev.pointerId === e.pointerId && finish(true);
		const onCancel = () => finish(false);
		const onKey = (ev: KeyboardEvent) => {
			if (ev.key === 'Escape') {
				ev.stopPropagation();
				finish(false);
			}
		};
		h.setPointerCapture(e.pointerId);
		h.addEventListener('pointermove', onMove);
		h.addEventListener('pointerup', onUp);
		h.addEventListener('pointercancel', onCancel);
		h.addEventListener('lostpointercapture', onCancel);
		window.addEventListener('keydown', onKey, true);
		cleanup = () => finish(false);
	}

	function key(e: KeyboardEvent) {
		if (o.disabled || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') || e.altKey || e.ctrlKey || e.metaKey) return;
		const h = (e.target as Element | null)?.closest<HTMLElement>(o.handle);
		if (!h) return;
		const rows = rowsOf();
		const from = rows.indexOf(h.closest<HTMLElement>(o.row)!);
		const to = from + (e.key === 'ArrowUp' ? -1 : 1);
		e.preventDefault();
		e.stopPropagation();
		if (from < 0 || to < 0 || to >= rows.length) return;
		o.onmove(from, to);
		// the moved row may be a new element after the re-render: keep the keyboard where it was
		void tick().then(() => rowsOf()[to]?.querySelector<HTMLElement>(o.handle)?.focus());
	}

	node.addEventListener('pointerdown', start);
	node.addEventListener('keydown', key);
	return {
		update(next: ReorderOptions) {
			o = next;
		},
		destroy() {
			node.removeEventListener('pointerdown', start);
			node.removeEventListener('keydown', key);
			cleanup?.();
		}
	};
}
