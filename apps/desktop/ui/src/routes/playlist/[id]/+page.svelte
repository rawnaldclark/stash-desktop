<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { library, player } from '../../../lib/app';
	import DownloadSwitch from '../../../lib/desktop/DownloadSwitch.svelte';
	import type { LibraryItem } from '../../../lib/api';
	import Icon from '../../../lib/ui/Icon.svelte';
	import Tile from '../../../lib/ui/Tile.svelte';
	import TrackRow from '../../../lib/ui/TrackRow.svelte';
	import { reorder } from '../../../lib/ui/reorder';
	import { L, ui } from '../../../lib/ui/state.svelte';
	import { songs, totalLength } from '../../../lib/ui/format';
	import { describe } from '../../../lib/desktop/smart';

	const pl = $derived(L.playlists.find((p) => p.id === page.params.id));
	/** A followed mix is its owner's (it came in with an imported library): it plays; its songs are not edited here. */
	const followed = $derived(!!pl?.followed);
	/** A smart playlist fills itself by its rules: its songs are not edited by hand. */
	const smart = $derived(pl?.rules);
	let editing = $state(false);
	let draft = $state('');
	let confirmDelete = $state(false);

	const play = (items: LibraryItem[], i = 0) => void player.playItems(items, i, { from: pl?.name });
	async function rename() {
		if (pl && draft.trim()) await library.renamePlaylist(pl.id, draft);
		editing = false;
	}
	async function remove() {
		if (!pl) return;
		try {
			await library.deletePlaylist(pl.id);
			await goto('/library?tab=playlists');
		} catch {
			confirmDelete = false; // said by the shell (the core's own words)
		}
	}
</script>

{#if pl}
	<div class="view view-in view-playlist">
		<div class="alb-top">
			<div class="alb-cover"><Tile items={pl.items} seed={pl.name} /></div>
			<div class="alb-txt">
				<p class="caps">{smart ? 'Smart playlist' : followed ? 'Shared mix' : 'Playlist'}</p>
				{#if editing}
					<form class="rename" onsubmit={(e) => { e.preventDefault(); void rename(); }}>
						<!-- svelte-ignore a11y_autofocus -->
						<input aria-label="Playlist name" bind:value={draft} maxlength="80" autofocus onkeydown={(e) => e.key === 'Escape' && (editing = false)} />
						<button type="submit" class="btn">Save</button>
					</form>
				{:else}
					<h1 class="alb-title">{pl.name}</h1>
				{/if}
				<p class="alb-meta"><b>{songs(pl.items.length)}</b>{#if pl.items.length}<span class="sep">{' · '}</span><span>{totalLength(pl.items)}</span>{/if}</p>
				{#if smart}<p class="pl-rules">{describe(smart)}</p>{/if}
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {pl.name}" disabled={!pl.items.length} onclick={() => play(pl.items)}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {pl.name}" title="Shuffle" disabled={!pl.items.length} onclick={() => { player.setShuffle(true); play(pl.items); }}><Icon name="shuffle" /></button>
			<DownloadSwitch list={pl.id} name={pl.name} count={pl.items.length} />
			{#if smart}<button type="button" class="btn ghost" onclick={() => (ui.smart = { id: pl.id })}>Edit rules</button>{/if}
			<button type="button" class="act-ib" aria-label="Rename playlist" title="Rename" onclick={() => { draft = pl.name; editing = true; }}><Icon name="edit" /></button>
			{#if confirmDelete}
				<button type="button" class="btn danger" onclick={remove}>Delete this playlist</button>
				<button type="button" class="btn ghost" onclick={() => (confirmDelete = false)}>Keep it</button>
			{:else}
				<button type="button" class="act-ib" aria-label="Delete playlist" title="Delete" onclick={() => (confirmDelete = true)}><Icon name="delete" /></button>
			{/if}
		</div>
		{#if pl.items.length}
			{#if followed || smart}
				<div class="trk-list">
					{#each pl.items as it, i (it.key + i)}
						<TrackRow item={it} num={i + 1} album onplay={() => play(pl.items, i)} />
					{/each}
				</div>
			{:else}
				<div class="trk-list" use:reorder={{ row: '.trk-row', handle: '.rd-h', onmove: (a, b) => void library.moveInPlaylist(pl.id, a, b) }}>
					{#each pl.items as it, i (it.key + i)}
						<TrackRow item={it} num={i + 1} album handle onplay={() => play(pl.items, i)} extra={{ playlistId: pl.id, playlistIndex: i, playlistLength: pl.items.length }} />
					{/each}
				</div>
			{/if}
		{:else if smart}
			<p class="s-hint">No song in your library matches these rules yet. It fills itself when one does.</p>
		{:else}
			<p class="s-hint">This playlist is empty. Use the “more” menu on any song and choose Add to playlist.</p>
		{/if}
	</div>
{:else if !L.loaded}
	<div class="view"></div>
{:else}
	<div class="view view-in">
		<p class="s-hint">That playlist isn’t here. <a class="lnk" href="/library?tab=playlists">Back to your playlists</a></p>
	</div>
{/if}

<style>
	.pl-rules { margin: 6px 0 0; color: var(--wp-text-2); font: 400 13px/1.45 var(--font-body); }
</style>
