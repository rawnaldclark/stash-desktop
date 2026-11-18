<!-- A mix of Stash's (Daily Discover, Recommended by Last.fm): its 40 songs of the day, as an album page shows its songs,
     with Play, Shuffle, Up next and Refresh (another draw now). New each day on its own. -->
<script lang="ts">
	import { page } from '$app/state';
	import { player, tubeItem } from '../../../lib/app';
	import { Refused } from '../../../lib/core';
	import { M, mixOf, refreshMix, type Mix } from '../../../lib/desktop/mixes.svelte';
	import Icon from '../../../lib/ui/Icon.svelte';
	import Tile from '../../../lib/ui/Tile.svelte';
	import TrackRow from '../../../lib/ui/TrackRow.svelte';
	import type { PickList } from '../../../lib/ui/pick.svelte';
	import { songs, totalLength } from '../../../lib/ui/format';
	import { ui } from '../../../lib/ui/state.svelte';

	const id = $derived(page.params.id ?? '');
	let mix = $state.raw<Mix | null>(null);
	let status = $state<'loading' | 'ok' | 'missing' | 'failed'>('loading');
	let refreshing = $state(false);
	let seq = 0;

	function load() {
		const mine = ++seq;
		status = 'loading';
		mixOf(id).then(
			(m) => {
				if (mine !== seq) return;
				mix = m;
				status = 'ok';
			},
			(e) => {
				if (mine !== seq) return;
				status = e instanceof Refused && (e.status === 404 || e.status === 409) ? 'missing' : 'failed';
			}
		);
	}
	$effect(() => {
		id;
		load();
	});
	// Made again meanwhile (the day turned, or Refresh in another window): the new songs.
	$effect(() => {
		const fresh = M.list.find((m) => m.id === id);
		if (fresh && mix && fresh.made_ms !== mix.made_ms) mix = fresh;
	});

	const items = $derived(mix ? mix.songs.map(tubeItem) : []);
	const play = (i = 0) => void player.playItems(items, i, { from: mix?.title });
	const pick: PickList = {
		get scope() {
			return `mix:${id}`;
		},
		count: () => items.length,
		between: (a, b) => items.slice(a, b + 1),
		get from() {
			return mix?.title;
		}
	};
	function upNext() {
		if (!items.length) return;
		player.enqueue(items);
		ui.toast(`Added ${songs(items.length)} to Up next`);
	}
	async function refresh() {
		if (refreshing) return;
		refreshing = true;
		try {
			mix = await refreshMix(id);
			ui.toast(`${mix.title}: ${songs(mix.songs.length)}, drawn again`);
		} catch (e) {
			ui.toast(e instanceof Error ? e.message : 'It did not refresh');
		} finally {
			refreshing = false;
		}
	}
	const made = $derived(mix?.made_ms ? new Date(mix.made_ms).toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' }) : null);
</script>

<div class="view view-in view-album">
	{#if status === 'loading'}
		<p class="s-hint" role="status">Loading…</p>
	{:else if status === 'missing'}
		<p class="s-hint" role="status">This mix isn’t on offer. Daily Discover needs YouTube Music; Recommended by Last.fm needs Last.fm connected in Settings › Scrobbling.</p>
	{:else if status === 'failed' || !mix}
		<p class="s-hint" role="alert">This mix didn’t load. <button type="button" class="lnk" onclick={load}>Try again</button></p>
	{:else}
		<div class="alb-top">
			<div class="alb-cover"><Tile items={items} seed={mix.title} /></div>
			<div class="alb-txt">
				<p class="caps">Mix</p>
				<h1 class="alb-title">{mix.title}</h1>
				<p class="alb-meta"><span>{mix.description}</span>{#if items.length}<span class="sep">{' · '}</span><span>{songs(items.length)}{#if items.some((i) => i.durationMs)}, {totalLength(items)}{/if}</span>{/if}</p>
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {mix.title}" disabled={!items.length} onclick={() => play()}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {mix.title}" title="Shuffle" disabled={!items.length} onclick={() => { player.setShuffle(true); play(); }}><Icon name="shuffle" /></button>
			<button type="button" class="act-ib" aria-label="Add {mix.title} to Up next" title="Add to Up next" disabled={!items.length} onclick={upNext}><Icon name="queue_play_next" /></button>
			<button type="button" class="act-ib" class:spin={refreshing} aria-label="Refresh {mix.title}" title={refreshing ? 'Finding songs…' : 'Refresh: other songs now'} disabled={refreshing || ui.offline} onclick={() => void refresh()}><Icon name="refresh" /></button>
		</div>
		{#if items.length}
			<div class="trk-list">
				{#each items as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} cover album {pick} at={i} onplay={() => play(i)} />
				{/each}
			</div>
			{#if made}<p class="s-hint">Made {made}. New every day.</p>{/if}
		{:else}
			<p class="s-hint" role="status">{refreshing ? 'Finding songs for you…' : 'No songs yet. Play some music and they come, or Refresh now.'}</p>
		{/if}
	{/if}
</div>

<style>
	.spin :global(svg) { animation: mix-spin 1s linear infinite; }
	@keyframes mix-spin { to { transform: rotate(360deg); } }
	@media (prefers-reduced-motion: reduce) { .spin :global(svg) { animation: none; } }
</style>
