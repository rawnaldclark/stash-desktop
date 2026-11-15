/**
 * Swipe a row sideways to remove it (Up next on touch screens), as a Svelte action. The app's rules (QueueBottomSheet, 2026-09):
 * - By distance only: a row leaves only when it is past half its width as the finger lifts. A flick never removes anything.
 * - Touch only; the drag handle and the buttons in a row never swipe; nothing happens while the list scrolls or a row is dragged
 *   (the gesture must be clearly sideways before it is a swipe, and the rows have `touch-action: pan-y`, so a vertical move is a scroll).
 * - The removal names the row as it is at that moment (`onremove(row)`; the caller looks it up by its own id), so the wrong song cannot
 *   go after the queue changed under the finger.
 * Transforms and opacity only.
 */
export interface SwipeOptions {
	/** Selector of the rows (inside the node). */
	row: string;
	/** Presses that start inside these never swipe (the drag handle, buttons that are not the row itself). */
	ignore: string;
	onremove: (row: HTMLElement) => void;
	disabled?: boolean;
}

/** Pixels of sideways travel before a touch is a swipe (and it must be clearly more sideways than down). */
export const SWIPE_SLOP = 12;

/** Whether a release at `dx` (px) on a row `width` px wide removes it. */
export const removes = (dx: number, width: number): boolean => width > 0 && Math.abs(dx) > width / 2;

export function swipe(node: HTMLElement, initial: SwipeOptions) {
	let o = initial;
	let live: (() => void) | null = null;

	function start(e: PointerEvent) {
		if (o.disabled || live || e.pointerType !== 'touch' || node.classList.contains('rd-active')) return;
		const t = e.target as Element | null;
		if (!t || t.closest(o.ignore)) return;
		const row = t.closest<HTMLElement>(o.row);
		if (!row || !node.contains(row)) return;
		const x0 = e.clientX;
		const y0 = e.clientY;
		const width = row.getBoundingClientRect().width;
		let state: 'pending' | 'swipe' | 'scroll' = 'pending';
		let dx = 0;

		const paint = () => {
			row.style.transform = `translate3d(${dx}px, 0, 0)`;
			row.style.opacity = String(Math.max(0.25, 1 - Math.abs(dx) / width));
		};
		const done = () => {
			node.removeEventListener('pointermove', move);
			node.removeEventListener('pointerup', up);
			node.removeEventListener('pointercancel', cancel);
			live = null;
		};
		const settle = (to: number, after?: () => void) => {
			row.classList.add('sw-settle');
			dx = to;
			paint();
			if (!to) row.style.opacity = '';
			setTimeout(() => {
				row.classList.remove('sw-settle', 'sw-on');
				if (after) after();
				else row.style.transform = '';
			}, 160);
		};
		const move = (ev: PointerEvent) => {
			if (ev.pointerId !== e.pointerId) return;
			const mx = ev.clientX - x0;
			const my = ev.clientY - y0;
			if (state === 'pending') {
				if (Math.abs(my) > SWIPE_SLOP && Math.abs(my) >= Math.abs(mx)) state = 'scroll';
				else if (Math.abs(mx) > SWIPE_SLOP && Math.abs(mx) > Math.abs(my) * 1.5) {
					state = 'swipe';
					row.classList.add('sw-on');
				}
			}
			if (state === 'scroll') return done();
			if (state === 'swipe') {
				dx = mx;
				paint();
			}
		};
		const up = (ev: PointerEvent) => {
			if (ev.pointerId !== e.pointerId) return;
			done();
			if (state !== 'swipe') return;
			// The click that may follow is not a press on the row.
			const swallow = (c: Event) => c.stopPropagation();
			window.addEventListener('click', swallow, { capture: true, once: true });
			setTimeout(() => window.removeEventListener('click', swallow, true), 0);
			if (removes(dx, width))
				settle(Math.sign(dx) * width, () => {
					o.onremove(row);
					// Normally the row is gone with its song; if it is still here (the song had already left), it comes back.
					requestAnimationFrame(() => {
						if (!row.isConnected) return;
						row.style.transform = '';
						row.style.opacity = '';
					});
				});
			else settle(0);
		};
		const cancel = (ev: PointerEvent) => {
			if (ev.pointerId !== e.pointerId) return;
			done();
			if (state === 'swipe') settle(0);
		};
		node.addEventListener('pointermove', move);
		node.addEventListener('pointerup', up);
		node.addEventListener('pointercancel', cancel);
		live = () => {
			done();
			row.style.transform = '';
			row.style.opacity = '';
			row.classList.remove('sw-on', 'sw-settle');
		};
	}

	node.addEventListener('pointerdown', start);
	return {
		update(next: SwipeOptions) {
			o = next;
		},
		destroy() {
			node.removeEventListener('pointerdown', start);
			live?.();
		}
	};
}
