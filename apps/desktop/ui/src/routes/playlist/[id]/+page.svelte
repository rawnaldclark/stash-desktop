<script lang="ts">
	import { savePlaylistFile } from '../../../lib/desktop/playlist-files';
	import { focusNow } from '../../../lib/ui/focus';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { library, player } from '../../../lib/app';
	import DownloadSwitch from '../../../lib/desktop/DownloadSwitch.svelte';
	import type { LibraryItem } from '../../../lib/api';
	import Icon from '../../../lib/ui/Icon.svelte';
	import Tile from '../../../lib/ui/Tile.svelte';
	import TrackRow from '../../../lib/ui/TrackRow.svelte';
	import { untrack } from 'svelte';
	import { P, type PickList } from '../../../lib/ui/pick.svelte';
	import { reorder } from '../../../lib/ui/reorder';
	import { L, ui } from '../../../lib/ui/state.svelte';
	import { songs, totalLength } from '../../../lib/ui/format';
	import { describe } from '../../../lib/desktop/smart';
	import { folderHref, playlistMenu, under } from '../../../lib/desktop/playlist-folders';

	const pl = $derived(L.playlists.find((p) => p.id === page.params.id));
	/** A followed mix is its owner's (it came in with an imported library): it plays; its songs are not edited here. */
	const followed = $derived(!!pl?.followed);
	/** A YouTube Music playlist it follows: read again when it is opened and was last read over an hour ago (the core
	 *  reads followed lists every six hours as well). */
	const tube = $derived(pl?.followsTube);
	let readAgain = '';
	$effect(() => {
		const [list, checked, plId] = [tube, pl?.followsCheckedAt ?? 0, pl?.id];
		if (!list || !plId || readAgain === plId || Date.now() - checked < 3_600_000) return;
		readAgain = plId;
		void library.followTube(list).catch(() => {});
	});
	/** A smart playlist fills itself by its rules: its songs are not edited by hand. */
	const smart = $derived(pl?.rules);
	/** The folder it is in, if any. */
	const folder = $derived(pl?.folder ? L.folders.find((f) => f.id === pl.folder) : undefined);
	let editing = $state(false);
	let draft = $state('');
	let confirmDelete = $state(false);

	const play = (items: LibraryItem[], i = 0) => void player.playItems(items, i, { from: pl?.name });

	/** Songs picked in the playlist; the listener's own can then be taken out of it together. */
	const pick: PickList = {
		get scope() {
			return `playlist:${pl?.id ?? ''}`;
		},
		count: () => pl?.items.length ?? 0,
		between: (a, b) => (pl?.items ?? []).slice(a, b + 1),
		get from() {
			return pl?.name;
		},
		extra: (places) =>
			pl && !followed && !smart ? { playlistId: pl.id, playlistLength: pl.items.length, ...(places.length === 1 ? { playlistIndex: places[0] } : {}) } : {}
	};
	// The playlist's songs changed (one added, taken out, moved): the places did too, and the picks go. A rename or the
	// keep switch leaves them.
	const songsNow = $derived(pl?.items);
	$effect(() => {
		void songsNow;
		untrack(() => P.forget(pick.scope));
	});
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
				<p class="caps">{smart ? 'Smart playlist' : tube ? 'Follows YouTube Music' : followed ? 'Shared mix' : 'Playlist'}{#if folder}<span class="sep">{' · '}</span><a class="pl-folder" href={folderHref(folder.id)}>{folder.name}</a>{/if}</p>
				{#if editing}
					<form class="rename" onsubmit={(e) => { e.preventDefault(); void rename(); }}>
						<input aria-label="Playlist name" bind:value={draft} maxlength="80" use:focusNow onkeydown={(e) => e.key === 'Escape' && (editing = false)} />
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
			<button type="button" class="act-ib" aria-label="Move to folder" title={folder ? `In ${folder.name}` : 'Move to folder'} aria-haspopup="menu" onclick={(e) => playlistMenu(pl, under(e.currentTarget))}><Icon name="drive_file_move" /></button>
			<button type="button" class="act-ib" aria-label="Rename playlist" title="Rename" onclick={() => { draft = pl.name; editing = true; }}><Icon name="edit" /></button>
			<button type="button" class="act-ib" aria-label="Save as a playlist file" title="Save as a playlist file (.m3u8)" onclick={() => void savePlaylistFile(pl.id)}><Icon name="ios_share" /></button>
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
						<TrackRow item={it} num={i + 1} album {pick} at={i} onplay={() => play(pl.items, i)} />
					{/each}
				</div>
			{:else}
				<div class="trk-list" use:reorder={{ row: '.trk-row', handle: '.rd-h', onmove: (a, b) => void library.moveInPlaylist(pl.id, a, b) }}>
					{#each pl.items as it, i (it.key + i)}
						<TrackRow item={it} num={i + 1} album handle {pick} at={i} onplay={() => play(pl.items, i)} extra={{ playlistId: pl.id, playlistIndex: i, playlistLength: pl.items.length }} />
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
