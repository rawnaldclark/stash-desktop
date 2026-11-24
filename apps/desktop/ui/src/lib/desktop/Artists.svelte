<!-- Library › Artists: the library's artists as the web player's round artist cards, read from the core a page at a time as the grid
     is scrolled. A card opens the artist's page. -->
<script lang="ts">
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { songs } from '../ui/format';
	import { fold } from '../identity';
	import { more } from './more';

	/** What is typed in the library's search box: the grid shows only what matches it. */
	let { q = '' }: { q?: string } = $props();
	const needle = $derived(fold(q));

	const PAGE = 120;
	let rows = $state.raw<core.ArtistRow[]>([]);
	let done = $state(false);
	let failure = $state<string | null>(null);

	/** How many cards are drawn: a page more each time the end of the grid comes near, however many rows are read. */
	let upTo = $state(PAGE);

	/** Reads the next page from the core. */
	async function read(): Promise<boolean> {
		if (done) return false;
		try {
			const page = await core.artistsAt(rows.length, PAGE);
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
	const matching = $derived(needle ? rows.filter((a) => fold(a.name).includes(needle)) : rows);
	const shown = $derived(matching.slice(0, upTo));
	const art = (a: core.ArtistRow) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
	/** Two letters for an artist with no picture ("Vince Staples" -> "VS"). */
	const initials = (name: string) => name.replace(/[([{].*?[)\]}]/g, ' ').split(/\s+/).filter((w) => /\p{L}/u.test(w)).slice(0, 2).map((w) => w.match(/\p{L}/u)![0].toUpperCase()).join('');
	const sub = (a: core.ArtistRow) => [a.albums ? (a.albums === 1 ? '1 album' : `${a.albums} albums`) : '', songs(a.tracks)].filter(Boolean).join(' · ');
</script>

{#if shown.length}
	<div class="cards cards-artists">
		{#each shown as a (a.id)}
			<Card title={a.name} sub={sub(a)} artwork={art(a)} href="/artist/library/{a.id}" ini={initials(a.name)} />
		{/each}
	</div>
{:else if needle && rows.length}
	<p class="s-hint">{done ? `No artist in your library matches “${q.trim()}”.` : 'Looking…'}</p>
{:else if done}
	<p class="s-hint">No artists yet. Artists show up here once your library has songs.</p>
{/if}
{#if failure}
	<p class="s-hint" role="alert">{failure} <button type="button" class="lnk" onclick={() => void load()}>Try again</button></p>
{:else if !done || upTo < matching.length}
	{#key needle}<div class="more" use:more={load} aria-hidden="true"></div>{/key}
{/if}

<style>
	.more { height: 1px; }
	/* the web player's artist cards (its Artists shelf): centred, the picture a circle */
	.cards-artists :global(.card) { text-align: center; }
	.cards-artists :global(.card > .cv) { border-radius: 50%; }
</style>
