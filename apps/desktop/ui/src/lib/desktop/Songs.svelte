<!-- Library › Songs: the whole library in title order. It can be 100,000 songs and more, so it is never held in memory: the virtual
     list draws the rows on screen and reads their pages from the core as it scrolls. Each row is the web player's song row.
     `query` filters in place (the core searches; the first hundred matches are listed). `here` lists only the songs that play with
     no connection ("On this computer"), and says how many there are in `hereTotal`.
     This is also the list the window measures itself on (`?bench`, see bench.ts). -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { itemOfRow, player } from '../app';
	import { measure, type Drawn } from '../bench';
	import * as core from '../core';
	import VirtualList from '../VirtualList.svelte';
	import TrackRow from '../ui/TrackRow.svelte';
	import { L, ui } from '../ui/state.svelte';

	let { query = '', here = false, hereTotal = $bindable(null) }: { query?: string; here?: boolean; hereTotal?: number | null } = $props();

	/** The web player's song row with its cover. */
	const ROW_HEIGHT = 56;
	const FROM = 'Your library';

	let list: VirtualList<core.TrackRow> | undefined = $state();
	let scroller = $state<HTMLElement | null>(null);
	/** How many songs the library has: the library's own count, or (measuring) the one this view asked for itself. */
	let measuredCount = $state<number | null>(null);
	const count = $derived(here ? hereTotal : core.measuring ? measuredCount : (L.counts?.tracks ?? null));
	// How many are on this computer: read again when the library changes, and while the list is shown now and then (songs
	// being saved arrive), but only when that number moved.
	$effect(() => {
		if (!here) return;
		void L.changed;
		let gone = false;
		const read = () =>
			core
				.hereCount()
				.then((n) => {
					if (!gone && n !== hereTotal) hereTotal = n;
				})
				.catch(() => {});
		void read();
		const timer = setInterval(read, 5000);
		return () => {
			gone = true;
			clearInterval(timer);
		};
	});
	let failure = $state<string | null>(null);

	// ---- the filter: what the core finds for the text, in place of the list

	let results = $state.raw<core.TrackRow[]>([]);
	/** The query whose results are on screen (null: the whole library shows). */
	let resultsFor = $state<string | null>(null);
	const resultItems = $derived(results.map(itemOfRow));
	$effect(() => {
		const text = query;
		if (text.trim() === '') {
			results = [];
			resultsFor = null;
			return;
		}
		const stop = new AbortController();
		core
			.searchTracks(text, 100, stop.signal)
			.then((rows) => {
				results = here ? rows.filter((r) => r.here) : rows;
				resultsFor = text;
			})
			.catch(() => {});
		return () => stop.abort();
	});

	// ---- measuring

	let drawn: Drawn = { first: 0, last: 0, loaded: 0 };
	/** When things first happened, in ms since the page began to load (not tied to frames, for the measurement). */
	const marks: Record<string, number> = { scriptMs: core.scriptStartedMs };
	function noteDrawn(state: Drawn) {
		drawn = state;
		if (marks.firstRowsDataMs === undefined && state.last > state.first && state.loaded === state.last - state.first) {
			marks.firstRowsDataMs = Math.round(performance.now());
		}
	}

	onMount(async () => {
		scroller = document.getElementById('main');
		if (!core.measuring) return;
		let tracks: number;
		try {
			tracks = (await core.counts()).tracks;
			marks.countsMs = Math.round(performance.now());
			measuredCount = tracks;
		} catch (e) {
			failure = `The Stash core is not answering (${e instanceof Error ? e.message : e}).`;
			return;
		}
		const result = await measure({
			count: tracks,
			rowHeight: ROW_HEIGHT,
			drawn: () => drawn,
			scrollToIndex: (index) => list?.scrollToIndex(index),
			scrollBy: (pixels) => list?.scrollBy(pixels),
			// The text goes into the header's search box, which filters this list in place while the window measures itself.
			type: (text) => (ui.q = text),
			showing: () => resultsFor,
			marks
		});
		await core.postMeasurement(result).catch(() => {});
		document.title = 'Stash (measured)';
	});
</script>

{#snippet songRow(track: core.TrackRow | undefined, index: number)}
	{#if track}
		<TrackRow item={itemOfRow(track)} album onplay={() => void player.playLibrary(index, { from: FROM, here })} />
	{:else}
		<div class="trk-row has-cv has-al" aria-hidden="true"><div class="trk-main"><span class="cv trk-cv"></span><span class="tr-t"><b class="ph"></b></span></div></div>
	{/if}
{/snippet}

<div class="songs">
	{#if failure}
		<p class="s-hint" role="alert">{failure}</p>
	{:else if resultsFor !== null}
		{#if results.length === 0}
			<p class="s-hint" role="status">Nothing {here ? 'on this computer' : 'in your library'} matches “{resultsFor.trim()}”.</p>
		{:else}
			<div class="trk-list">
				{#each resultItems as it, i (it.refs.lib)}
					<TrackRow item={it} album onplay={() => void player.playItems(resultItems, i, { from: FROM })} />
				{/each}
			</div>
		{/if}
	{:else if here && count === 0}
		<p class="s-hint">No songs are on this computer yet. Keep a playlist or your liked songs on this computer (the arrow on its page), or add a music folder in <a class="nl lnk" href="/settings#library">Settings</a>.</p>
	{:else if count === 0}
		<p class="s-hint">Your library has no songs yet. Add a music folder or import a library in <a class="nl lnk" href="/settings#library">Settings</a>.</p>
	{:else if count !== null && scroller}
		{#key `${L.changed}:${here}:${here ? count : ''}`}
			<VirtualList bind:this={list} {count} rowHeight={ROW_HEIGHT} fetchPage={here ? core.tracksHereAt : core.tracksAt} row={songRow} {scroller} onDrawn={noteDrawn} />
		{/key}
	{/if}
</div>

<style>
	/* the rows bleed 8 px each side (their hover fill), as in every list: the list takes the bleed, so a row is not cut by its own box */
	.songs { margin: 4px -8px 0; }
	.songs :global(.trk-row) { margin: 0; }
	.songs > :global(.s-hint) { margin-left: 8px; margin-right: 8px; }
	/* a row whose page has not arrived yet: the row's own boxes, empty */
	.ph { display: block; width: min(240px, 60%); height: 12px; border-radius: 6px; background: var(--wp-hover); }
</style>
