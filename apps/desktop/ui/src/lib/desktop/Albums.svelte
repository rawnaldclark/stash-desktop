<!-- Library › Albums: the library's albums as the web player's cards, read from the core a page at a time as the grid is scrolled.
     A card opens the album's page. -->
<script lang="ts">
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { fold } from '../identity';
	import { more } from './more';

	/** What is typed in the library's search box: the grid shows only what matches it. */
	let { q = '' }: { q?: string } = $props();
	const needle = $derived(fold(q));

	const PAGE = 120;
	let rows = $state.raw<core.AlbumRow[]>([]);
	let done = $state(false);
	let failure = $state<string | null>(null);

	/** How many cards are drawn: a page more each time the end of the grid comes near, however many rows are read. */
	let upTo = $state(PAGE);

	/** Reads the next page from the core. */
	async function read(): Promise<boolean> {
		if (done) return false;
		try {
			const page = await core.albumsAt(rows.length, PAGE);
			rows = [...rows, ...page];
			done = page.length < PAGE;
			failure = null;
			return !done;
		} catch (e) {
			failure = core.said(e);
			return false;
		}
	}
	// Searching looks through everything, so the rest of the pages are read first (a few hundred rows a page).
	$effect(() => {
		if (!needle || done) return;
		let stop = false;
		(async () => {
			while (!stop && (await read())) {
				/* the next page */
			}
		})();
		return () => {
			stop = true;
		};
	});
	/** The end of the grid came near: a page more of what is already read, else the next page from the core. */
	async function load(): Promise<boolean> {
		if (upTo < matching.length) {
			upTo += PAGE;
			return true;
		}
		if (needle) return false;
		const again = await read();
		upTo = rows.length;
		return again;
	}
	// A new search starts from the top.
	$effect(() => {
		void needle;
		upTo = PAGE;
	});
	const matching = $derived(needle ? rows.filter((a) => fold(`${a.title} ${a.artist}`).includes(needle)) : rows);
	const shown = $derived(matching.slice(0, upTo));
	const art = (a: core.AlbumRow) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
</script>

{#if shown.length}
	<div class="cards">
		{#each shown as a (a.id)}
			<Card title={a.title} sub={[a.year, a.artist].filter(Boolean).join(' · ')} artwork={art(a)} href="/album/library/{a.id}" />
		{/each}
	</div>
{:else if needle && rows.length}
	<p class="s-hint">{done ? `No album in your library matches “${q.trim()}”.` : 'Looking…'}</p>
{:else if done}
	<p class="s-hint">No albums yet. Albums show up here once your library has songs.</p>
{/if}
{#if failure}
	<p class="s-hint" role="alert">{failure} <button type="button" class="lnk" onclick={() => void load()}>Try again</button></p>
{:else if !done || upTo < matching.length}
	{#key needle}<div class="more" use:more={load} aria-hidden="true"></div>{/key}
{/if}

<style>
	.more { height: 1px; }
</style>
