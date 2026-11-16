<!-- Library › Not found: songs your add-ons couldn't find when you played or saved them. Open one, pick the add-on's track
     that is it, and it plays from then on; "None of these" takes it off the list (it stays in your library, dimmed). Songs come
     here only from playing or saving: no add-on is asked about the whole library. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { itemOfRow } from '../app';
	import * as core from '../core';
	import TrackRow from '../ui/TrackRow.svelte';
	import FindSong from './FindSong.svelte';
	import { NF } from './unfound.svelte';

	const PAGE = 300;
	let d = $state<core.Unfound | null>(null);
	let shown = $state(PAGE);
	/** The song being fixed (the menu's "Find on your add-ons…" opens one). */
	let open = $state<number | null>(Number(page.url.searchParams.get('song')) || null);

	async function read() {
		try {
			d = await core.unfound(0, shown);
			// The song asked for is further down: the list is read on to it.
			while (open !== null && !d.rows.some((r) => r.id === open) && d.open > d.rows.length && shown < 20 * PAGE) {
				shown += PAGE;
				d = await core.unfound(0, shown);
			}
			NF.take(d);
		} catch {
			/* the core said nothing new: the list stays as it was */
		}
	}
	onMount(() => {
		void read();
	});

	const rows = $derived(d?.rows ?? []);
	const items = $derived(rows.map((r) => itemOfRow(r)));
	const done = () => {
		open = null;
		void read();
	};
</script>

{#if d}
	{#if rows.length}
		<p class="nf-say">Songs your add-ons couldn’t find. Pick the right one and it plays from then on.</p>
		<div class="trk-list nf-list">
			{#each rows as r, i (r.id)}
				<TrackRow item={items[i]} album hit="Find {r.title} by {r.artist} on your add-ons" onplay={() => (open = open === r.id ? null : r.id)} />
				{#if open === r.id}<FindSong id={r.id} title={r.title} ondone={done} />{/if}
			{/each}
		</div>
		{#if d.open > rows.length && rows.length >= shown}
			<button
				type="button"
				class="btn ghost nf-more"
				onclick={() => {
					shown += PAGE;
					void read();
				}}>Show more</button
			>
		{/if}
	{:else}
		<p class="s-hint">Every song you’ve played is on your add-ons.</p>
	{/if}
{/if}

<style>
	.nf-say { margin: 0 0 12px; color: var(--wp-text-2); font: 400 14px/1.45 var(--font-body); }
	.nf-more { margin: 16px 8px 0 0; }
	/* On its own list a song is not dimmed: it is what the list is about. */
	.nf-list :global(.trk-row.off .trk-main) { opacity: 1; }
</style>
