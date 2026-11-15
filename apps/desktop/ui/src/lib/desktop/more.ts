/**
 * `use:more={load}` on a marker at the end of a list that is read in pages: `load` runs when the marker comes near the screen (the
 * list was scrolled to its end), and again for as long as the marker stays there (a page that did not fill the screen).
 */
export function more(node: HTMLElement, load: () => Promise<boolean>) {
	let gone = false;
	let busy = false;
	const io = new IntersectionObserver(
		(entries) => {
			if (entries.some((e) => e.isIntersecting)) void run();
		},
		{ root: document.getElementById('main'), rootMargin: '0px 0px 800px 0px' }
	);
	/** `load` answers whether there may be more: the marker is looked at again, in case it is still near the screen. */
	async function run() {
		if (busy || gone) return;
		busy = true;
		const again = await load().catch(() => false);
		busy = false;
		if (gone || !again) return;
		io.unobserve(node);
		io.observe(node);
	}
	io.observe(node);
	return {
		destroy() {
			gone = true;
			io.disconnect();
		}
	};
}
