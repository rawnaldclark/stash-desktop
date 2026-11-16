/**
 * Carrying a playlist onto a folder (the sidebar), as a Svelte action on the playlist's row.
 *
 * - Pointer events only, as in reorder.ts: a drag the system starts (HTML5 drag and drop) does not reach the page in the
 *   desktop window on Windows. The row's own (link) drag is turned off.
 * - A press with a mouse or pen that travels a few pixels picks the row up: a chip with its name follows the pointer, and
 *   the place under it that takes playlists lights up (`.drop-on`). A place is an element with `data-drop-folder`: a
 *   folder's id, or empty for "out of any folder".
 * - Letting go there calls `drop(id, folder)` (null: out of any folder). Anywhere else, nothing happens; Escape (or a
 *   cancelled pointer) puts it back. The click that follows a carry is swallowed, so the row's link is not followed.
 * - A touch scrolls the sidebar; the playlist's menu (right-click, the menu key) moves it too.
 */
import { DRAG_SLOP } from './reorder';

export interface CarryOptions {
	id: string;
	name: string;
	drop: (id: string, folder: number | null) => void;
}

/** The place for playlists under a point, if any. */
function placeAt(x: number, y: number): HTMLElement | null {
	const el = document.elementFromPoint(x, y);
	return el instanceof Element ? el.closest<HTMLElement>('[data-drop-folder]') : null;
}

export function carry(node: HTMLElement, options: CarryOptions) {
	let o = options;
	let cleanup: (() => void) | null = null;
	node.draggable = false;
	const noDrag = (e: DragEvent) => e.preventDefault();
	node.addEventListener('dragstart', noDrag);

	const down = (e: PointerEvent) => {
		if (e.button !== 0 || e.pointerType === 'touch' || e.ctrlKey || e.shiftKey || e.metaKey || e.altKey) return;
		cleanup?.();
		const startX = e.clientX;
		const startY = e.clientY;
		let chip: HTMLElement | null = null;
		let over: HTMLElement | null = null;
		const light = (el: HTMLElement | null) => {
			if (el === over) return;
			over?.classList.remove('drop-on');
			over = el;
			over?.classList.add('drop-on');
		};
		const begin = () => {
			chip = document.createElement('div');
			chip.className = 'carry-chip';
			chip.textContent = o.name;
			document.body.append(chip);
			document.body.classList.add('carrying');
			try {
				node.setPointerCapture(e.pointerId);
			} catch {
				/* the pointer is gone */
			}
		};
		const move = (ev: PointerEvent) => {
			if (ev.pointerId !== e.pointerId) return;
			if (!chip && Math.hypot(ev.clientX - startX, ev.clientY - startY) > DRAG_SLOP) begin();
			if (!chip) return;
			ev.preventDefault();
			chip.style.transform = `translate(${ev.clientX + 14}px, ${ev.clientY + 10}px)`;
			light(placeAt(ev.clientX, ev.clientY));
		};
		const finish = (commit: boolean) => {
			window.removeEventListener('pointermove', move);
			window.removeEventListener('pointerup', up);
			window.removeEventListener('pointercancel', cancel);
			window.removeEventListener('keydown', key, true);
			cleanup = null;
			if (!chip) return;
			const to = over;
			chip.remove();
			chip = null;
			light(null);
			document.body.classList.remove('carrying');
			try {
				node.releasePointerCapture(e.pointerId);
			} catch {
				/* already released */
			}
			// The click that follows a carry is not a press on anything (nor a link followed).
			const swallow = (ev: Event) => {
				ev.preventDefault();
				ev.stopPropagation();
			};
			window.addEventListener('click', swallow, { capture: true, once: true });
			setTimeout(() => window.removeEventListener('click', swallow, true), 0);
			if (commit && to) {
				const folder = to.dataset.dropFolder ?? '';
				o.drop(o.id, folder === '' ? null : Number(folder));
			}
		};
		const up = (ev: PointerEvent) => ev.pointerId === e.pointerId && finish(true);
		const cancel = (ev: PointerEvent) => ev.pointerId === e.pointerId && finish(false);
		const key = (ev: KeyboardEvent) => {
			if (ev.key !== 'Escape' || !chip) return;
			ev.preventDefault();
			ev.stopPropagation();
			finish(false);
		};
		window.addEventListener('pointermove', move);
		window.addEventListener('pointerup', up);
		window.addEventListener('pointercancel', cancel);
		window.addEventListener('keydown', key, true);
		cleanup = () => finish(false);
	};
	node.addEventListener('pointerdown', down);

	return {
		update(next: CarryOptions) {
			o = next;
		},
		destroy() {
			cleanup?.();
			node.removeEventListener('pointerdown', down);
			node.removeEventListener('dragstart', noDrag);
		}
	};
}
