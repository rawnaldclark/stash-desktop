<script lang="ts">
	import { untrack } from 'svelte';
	import { shelf } from '../lib/ui/shelf';
	import { player } from '../lib/app';
	import type { LibraryItem, NewRelease } from '../lib/api';
	import { artUrl } from '../lib/core';
	import { albumHref } from '../lib/identity';
	import Card from '../lib/ui/Card.svelte';
	import { L, ui } from '../lib/ui/state.svelte';
	import { T } from '../lib/desktop/youtube.svelte';
	import TubeShelves from '../lib/desktop/TubeShelves.svelte';
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
	const quiet = $derived(!recent.length && !L.playlists.length && !albums.length && !L.releases.length);

	/** Records by the artists the listener follows that came out lately. */
	const releaseArt = (art: string | null) => {
		const url = artUrl(art);
		return url ? [{ url }] : undefined;
	};
	const releaseSub = (r: NewRelease) => [r.kind === 'single' ? 'Single' : 'Album', r.artist].join(' · ');

	const play = (items: LibraryItem[], i = 0, from?: string) => void player.playItems(items, i, { from });

	// YouTube Music's home page under the library's shelves: the listener's own when they signed in.
	const tubeOn = $derived(T.on && !ui.offline);
	$effect(() => {
		if (tubeOn) untrack(() => void T.refreshAccount().then(() => T.loadHome()));
	});
	const tube = $derived(T.on && !ui.offline && T.home?.length ? T.home : null);
</script>

{#snippet fresh()}
	{#if L.releases.length}
		<h2 class="vh2">New releases</h2>
		<div class="shelf-w" use:shelf>
			<div class="shelf">
				{#each L.releases as r (r.source + r.id)}
					<Card title={r.title} sub={releaseSub(r)} artwork={releaseArt(r.art)} href={albumHref({ sourceId: r.source, id: r.id })} />
				{/each}
			</div>
		</div>
	{/if}
{/snippet}

{#snippet youtube()}
	{#if tube}
		<TubeShelves shelves={tube} />
	{:else if T.on && !ui.offline && T.homeTrouble}
		<p class="s-hint" role="status">YouTube Music’s home did not load: {T.homeTrouble} <button type="button" class="lnk" onclick={() => void T.loadHome(true)}>Try again</button></p>
	{/if}
{/snippet}

{#if !L.loaded}
	<div class="view"></div>
{:else if empty}
	<section class="empty" aria-label="Get started">
		<p class="empty-line">Add a music folder or import a library to begin.</p>
		<a class="btn" href="/settings#library">Open Settings</a>
	</section>
	{#if tube || L.releases.length}<div class="view view-in">{@render fresh()}{@render youtube()}</div>{/if}
{:else if quiet}
	<section class="empty" aria-label="Get started">
		<p class="empty-line">Play a song and it shows up here.</p>
		<a class="btn" href="/library?tab=songs">Open your library</a>
	</section>
	{#if tube}<div class="view view-in">{@render youtube()}</div>{/if}
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
		{@render fresh()}
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
		{@render youtube()}
	</div>
{/if}
