/**
 * `use:shelf` on the wrapper of a horizontal shelf (`<div class="shelf-w" use:shelf><div class="shelf">…</div></div>`): on a screen
 * with a mouse, left/right arrows that page the shelf (see shelf-arrows.ts, its own chunk, fetched only where a shelf is shown).
 * Touch screens keep the swipe and get no arrows.
 */
export function shelf(node: HTMLElement) {
	let off: (() => void) | undefined;
	let gone = false;
	if (typeof matchMedia === 'function' && matchMedia('(hover: hover) and (pointer: fine)').matches) {
		void import('./shelf-arrows')
			.then((m) => {
				if (!gone) off = m.arrows(node);
			})
			.catch(() => {}); // no arrows this time (a load cut short): the shelf still scrolls
	}
	return {
		destroy() {
			gone = true;
			off?.();
		}
	};
}
