<!-- A playlist tile: a 2x2 mosaic of its songs' covers, a single cover, or the Likes heart tile. -->
<script lang="ts">
	import type { LibraryItem } from '../api';
	import Cover from './Cover.svelte';
	import Icon from './Icon.svelte';

	let { items = [], liked = false, seed = '' }: { items?: LibraryItem[]; liked?: boolean; seed?: string } = $props();

	const covers = $derived.by(() => {
		const seen = new Set<string>();
		const out: LibraryItem[] = [];
		for (const i of items) {
			const u = i.artwork?.[0]?.url ?? i.key;
			if (seen.has(u)) continue;
			seen.add(u);
			out.push(i);
			if (out.length === 4) break;
		}
		return out;
	});
</script>

<span class="tile" aria-hidden="true">
	{#if liked}
		<span class="tile-liked"><Icon name="favorite" /></span>
	{:else if covers.length >= 4}
		<span class="mos">{#each covers as c, n (n)}<Cover artwork={c.artwork} seed={c.title} min={60} />{/each}</span>
	{:else if covers.length > 0}
		<Cover artwork={covers[0].artwork} seed={covers[0].title} min={120} />
	{:else}
		<Cover {seed} />
	{/if}
</span>
