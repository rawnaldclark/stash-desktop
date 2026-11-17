<!-- Shelves of YouTube Music's cards (its home page): a song plays with the songs of its shelf after it; an album, an
     artist or a playlist opens its page. -->
<script lang="ts">
	import { player, tubeCard } from '../app';
	import type { TubeShelf } from '../core';
	import { shelf as shelfScroll } from '../ui/shelf';
	import Card from '../ui/Card.svelte';

	let { shelves }: { shelves: TubeShelf[] } = $props();

	const shown = $derived(
		shelves.map((s) => {
			const cards = s.cards.map(tubeCard);
			const songs = cards.flatMap((c) => (c.song ? [c.song] : []));
			return { title: s.title, over: s.over, cards, songs };
		})
	);
	const initials = (name: string) => name.slice(0, 2).toUpperCase();
</script>

{#each shown as s, n (s.title + n)}
	<h2 class="vh2 tube-h">
		{#if s.over}<span class="caps">{s.over}</span>{/if}
		{s.title}
	</h2>
	<div class="shelf-w" use:shelfScroll>
		<div class="shelf">
			{#each s.cards as c (c.key)}
				{#if c.song}
					{@const song = c.song}
					<Card title={c.title} sub={c.sub} artwork={c.artwork} label="Play {c.title} by {c.sub}" {song} onclick={() => void player.playItems(s.songs, s.songs.indexOf(song), { from: s.title })} />
				{:else}
					<Card title={c.title} sub={c.sub} artwork={c.artwork} href={c.href} ini={c.round ? initials(c.title) : undefined} />
				{/if}
			{/each}
		</div>
	</div>
{/each}

<style>
	.tube-h { flex-direction: column; align-items: flex-start; justify-content: flex-start; gap: 2px; }
</style>
