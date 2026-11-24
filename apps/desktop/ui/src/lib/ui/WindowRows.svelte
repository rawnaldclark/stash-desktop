<!-- A long list of rows that are all in memory (your likes, a playlist, the downloads), drawn a screenful at a time. The
     list is as tall as all its rows, so the page scrolls as before; only the rows near the part of the page being looked
     at exist. Without this, a tab holding thousands of songs builds thousands of rows to show forty, and every tab
     switch, pick and delete pays for all of them. Every row is `rowHeight` tall. (A list whose rows come in pages from
     the core is `VirtualList.svelte`.) -->
<script lang="ts" generics="T">
	import type { Snippet } from 'svelte';

	type Props = {
		items: T[];
		rowHeight?: number;
		/** What tells one row from another, as in a keyed each. */
		key: (item: T, index: number) => string | number;
		row: Snippet<[T, number]>;
	};
	let { items, rowHeight = 56, key, row }: Props = $props();

	/** Rows kept above and below what shows, so a quick scroll does not outrun them. */
	const OVERSCAN = 14;

	let box = $state<HTMLDivElement>();
	/** How far the page is scrolled past the list's top, and how tall the page's view is. */
	let scrollTop = $state(0);
	let height = $state(900);

	$effect(() => {
		const own = box;
		const outer = document.getElementById('main');
		if (!own || !outer) return;
		let offsetTop = 0;
		const scrolled = () => (scrollTop = Math.max(0, outer.scrollTop - offsetTop));
		const place = () => {
			offsetTop = own.getBoundingClientRect().top - outer.getBoundingClientRect().top + outer.scrollTop;
			height = outer.clientHeight;
			scrolled();
		};
		place();
		outer.addEventListener('scroll', scrolled, { passive: true });
		// What is above the list can change its height, and the window can be resized.
		const resized = new ResizeObserver(place);
		resized.observe(outer);
		if (own.parentElement) resized.observe(own.parentElement);
		return () => {
			outer.removeEventListener('scroll', scrolled);
			resized.disconnect();
		};
	});

	const first = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN));
	const last = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / rowHeight) + OVERSCAN));
	const shown = $derived(items.slice(first, last).map((item, k) => ({ item, index: first + k })));
</script>

<div class="wrows" bind:this={box} style:height="{items.length * rowHeight}px" style:padding-top="{first * rowHeight}px">
	{#each shown as s (key(s.item, s.index))}
		{@render row(s.item, s.index)}
	{/each}
</div>

<style>
	.wrows { box-sizing: border-box; }
</style>
