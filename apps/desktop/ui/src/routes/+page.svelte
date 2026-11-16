<script lang="ts">
	import { shelf } from '../lib/ui/shelf';
	import { player } from '../lib/app';
	import type { LibraryItem } from '../lib/api';
	import Card from '../lib/ui/Card.svelte';
	import { L } from '../lib/ui/state.svelte';
	import { songs } from '../lib/ui/format';
	import { jumpBackIn } from '../lib/home/jump';

	const recent = $derived.by(() => {
		const seen = new Set<string>();
		const out: LibraryItem[] = [];
		for (const h of L.history) {
			if (seen.has(h.item.key)) continue;
			seen.add(h.item.key);
			out.push(h.item);
			if (out.length === 8) break;
		}
		return out;
	});
	/** Nothing in the library at all: no songs, likes, plays or playlists. */
	const empty = $derived((L.counts?.tracks ?? 0) === 0 && L.likes.length === 0 && L.history.length === 0 && L.playlists.every((p) => p.items.length === 0));

	/** Old favourites you haven't played lately, never what "Recently played" already shows (lib/home/jump.ts). */
	const albums = $derived(jumpBackIn(L.history, new Set(recent.map((i) => i.key))));
	/** There are songs, but nothing has been played and no playlist made yet: Home has no shelf to show. */
	const quiet = $derived(!recent.length && !L.playlists.length && !albums.length);

	const play = (items: LibraryItem[], i = 0, from?: string) => void player.playItems(items, i, { from });
</script>

{#if !L.loaded}
	<div class="view"></div>
{:else if empty}
	<section class="empty" aria-label="Get started">
		<p class="empty-line">Add a music folder or import a library to begin.</p>
		<a class="btn" href="/settings#library">Open Settings</a>
	</section>
{:else if quiet}
	<section class="empty" aria-label="Get started">
		<p class="empty-line">Play a song and it shows up here.</p>
		<a class="btn" href="/library?tab=songs">Open your library</a>
	</section>
{:else}
	<div class="view view-in">
		{#if recent.length}
			<h2 class="vh2">Recently played</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each recent as it, i (it.key)}
						<Card title={it.title} sub={it.artist} artwork={it.artwork} label="Play {it.title} by {it.artist}" song={it} onclick={() => play(recent, i, 'Recently played')} />
					{/each}
				</div>
			</div>
		{/if}
		{#if L.playlists.length}
			<h2 class="vh2">Your playlists <a class="vh2-more" href="/library?tab=playlists">Show all</a></h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each L.playlists.slice(0, 8) as p (p.id)}
						<Card title={p.name} sub={songs(p.items.length)} items={p.items} href="/playlist/{p.id}" kept={p.download} />
					{/each}
				</div>
			</div>
		{/if}
		{#if albums.length}
			<h2 class="vh2">Jump back in</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each albums as g (g[0].album + g[0].artist)}
						<Card title={g[0].album ?? ''} sub={g[0].artist} artwork={g[0].artwork} label="Play {g[0].album} by {g[0].artist}" menu={{ item: g[0], extra: { group: g } }} onclick={() => play(g, 0, g[0].album)} />
					{/each}
				</div>
			</div>
		{/if}
	</div>
{/if}
