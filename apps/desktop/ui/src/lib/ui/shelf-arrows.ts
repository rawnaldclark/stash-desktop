import { ICONS } from './icons';

/**
 * The arrows of a horizontal shelf: one each side, shown while the pointer is over the shelf (or an arrow has
 * keyboard focus), each pages the shelf by one view-width with a smooth scroll (instant under reduced motion), and each is gone at
 * its end (so neither shows when everything fits). The raw scrollbar is hidden in CSS; touch swipe is untouched (no arrows there).
 * `wrap` holds exactly one `.shelf`; the buttons are added to `wrap`, centred on the first card's picture.
 */
export function arrows(wrap: HTMLElement): () => void {
	const row = wrap.querySelector<HTMLElement>(':scope > .shelf');
	if (!row) return () => {};
	const make = (dir: -1 | 1) => {
		const b = document.createElement('button');
		b.type = 'button';
		b.className = `sh-a ${dir < 0 ? 'sh-l' : 'sh-r'}`;
		b.setAttribute('aria-label', dir < 0 ? 'Scroll left' : 'Scroll right');
		b.title = dir < 0 ? 'Previous' : 'Next';
		b.innerHTML = `<svg class="ic" viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="${dir < 0 ? ICONS.chevron_left : ICONS.chevron_right}"/></svg>`;
		b.addEventListener('click', () => {
			const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
			row.scrollBy({ left: dir * row.clientWidth, behavior: reduced ? 'auto' : 'smooth' });
		});
		wrap.append(b);
		return b;
	};
	const left = make(-1);
	const right = make(1);

	let frame = 0;
	const update = () => {
		frame = 0;
		const max = row.scrollWidth - row.clientWidth;
		const atStart = row.scrollLeft <= 2;
		const atEnd = row.scrollLeft >= max - 2;
		// An arrow that goes away while it has focus hands the focus to the other one (or back to the shelf's first card).
		for (const [b, hide, other] of [
			[left, atStart, right],
			[right, atEnd, left]
		] as const) {
			if (hide && !b.hidden && document.activeElement === b) (other.hidden ? row.querySelector<HTMLElement>('a, button') : other)?.focus();
			b.hidden = hide;
		}
		// centred on the first card's picture (cards are the same height across a row)
		const pic = row.querySelector<HTMLElement>('.cv, .tile');
		const top = pic ? pic.getBoundingClientRect().top - wrap.getBoundingClientRect().top + pic.offsetHeight / 2 : row.offsetHeight / 2;
		wrap.style.setProperty('--sh-y', `${Math.round(top)}px`);
	};
	const soon = () => {
		if (!frame) frame = requestAnimationFrame(update);
	};
	row.addEventListener('scroll', soon, { passive: true });
	const ro = new ResizeObserver(soon);
	ro.observe(row);
	const mo = new MutationObserver(soon);
	mo.observe(row, { childList: true });
	update();
	return () => {
		cancelAnimationFrame(frame);
		row.removeEventListener('scroll', soon);
		ro.disconnect();
		mo.disconnect();
		left.remove();
		right.remove();
	};
}
