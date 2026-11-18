<script lang="ts">
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { browse, library, player } from '../../../../lib/app';
	import type { Album, LibraryItem } from '../../../../lib/api';
	import { Refused } from '../../../../lib/core';
	import { artistHref, artistLink, LIB_REF, LIBRARY_SOURCE, toLibraryItem } from '../../../../lib/identity';
	import BrowseMsg from '../../../../lib/ui/BrowseMsg.svelte';
	import Cover from '../../../../lib/ui/Cover.svelte';
	import Icon from '../../../../lib/ui/Icon.svelte';
	import TrackRow from '../../../../lib/ui/TrackRow.svelte';
	import type { PickList } from '../../../../lib/ui/pick.svelte';
	import { songs, totalLength } from '../../../../lib/ui/format';
	import { ui } from '../../../../lib/ui/state.svelte';

	const sid = $derived(page.params.source ?? '');
	const id = $derived(page.params.id ?? '');
	let album = $state.raw<Album | null>(null);
	let status = $state<'loading' | 'ok' | 'missing' | 'failed'>('loading');
	let seq = 0;

	function load() {
		const mine = ++seq;
		const [s, i] = [sid, id];
		status = 'loading';
		album = null;
		browse.album(s, i).then(
			(a) => {
				if (mine !== seq) return;
				album = a;
				status = 'ok';
			},
			(e) => {
				if (mine !== seq) return;
				status = e instanceof Refused && e.status === 404 ? 'missing' : 'failed';
			}
		);
	}
	$effect(() => {
		sid;
		id;
		load();
	});

	// The library's album: its songs are rows of the library, each carrying its row (`refs.lib`) and the library's own album
	// and artist pages. YouTube Music's album or playlist: its songs are that source's (`refs.youtube`), not on this computer.
	const items = $derived<LibraryItem[]>(
		album ? album.tracks.map((t) => (sid === LIBRARY_SOURCE ? toLibraryItem(t, { [LIB_REF]: t.id }, Date.now(), sid) : { ...toLibraryItem(t, { [sid]: t.id }, 0, sid), here: false })) : []
	);
	const isPlaylist = $derived(album?.kind === 'playlist');
	// The album's artist links to their page when every track carries the same artist id (a compilation has several: no link).
	const artistTo = $derived.by(() => {
		const links = items.map((i) => artistLink(i)?.id);
		return links.length > 0 && links.every((l) => l && l === links[0]) ? artistLink(items[0]!) : null;
	});
	const play = (i = 0) => void player.playItems(items, i, { from: album?.title });
	const pick: PickList = {
		get scope() {
			return `album:${sid}:${id}`;
		},
		count: () => items.length,
		between: (a, b) => items.slice(a, b + 1),
		get from() {
			return album?.title;
		}
	};
	const upNext = () => {
		if (!items.length) return;
		player.enqueue(items);
		ui.toast(`Added ${songs(items.length)} to Up next`);
	};
	// An album found elsewhere (YouTube Music's, an extension's) is put in the library: its songs become yours, and the
	// album is listed with your albums. `kept`: how many of its songs the library has (null while that is asked).
	let kept = $state<number | null>(null);
	let keeping = $state(false);
	const foreign = $derived(sid !== LIBRARY_SOURCE && !isPlaylist);
	$effect(() => {
		const [s, list] = [sid, items];
		kept = null;
		if (s === LIBRARY_SOURCE || !list.length) return;
		void library.countKept(s, list).then(
			(n) => {
				if (s === sid && list === items) kept = n;
			},
			() => {}
		);
	});
	const allKept = $derived(kept !== null && kept >= items.length && items.length > 0);
	async function keepAlbum() {
		if (!album || !items.length || allKept || keeping) return;
		keeping = true;
		try {
			await library.keepSongs(items);
			kept = items.length;
			ui.toast(`Added “${album.title}” to your library`);
		} catch {
			/* said by the shell (the core's own words) */
		} finally {
			keeping = false;
		}
	}
	// The album becomes a playlist with its name (saved once; a second tap says so).
	async function save() {
		if (!album || !items.length) return;
		const name = album.title;
		// Already saved = a playlist of that name that holds every song of the album. A playlist of yours that only shares the name
		// (or an older, partial save) does not block saving the album.
		const keys = items.map((i) => i.key);
		if (get(library.playlists).some((p) => p.name.toLowerCase() === name.toLowerCase() && keys.every((k) => p.items.some((x) => x.key === k)))) {
			ui.toast(`“${name}” is already in your playlists`);
			return;
		}
		try {
			const pl = await library.createPlaylist(name);
			await library.addToPlaylist(pl.id, items);
			ui.toast(`Saved “${name}” as a playlist`);
		} catch {
			/* said by the shell (the core's own words) */
		}
	}
</script>

<div class="view view-in view-album">
	{#if status !== 'ok' || !album}
		<BrowseMsg kind="album" state={status === 'ok' ? 'loading' : status} retry={load} />
	{:else}
		<div class="alb-top">
			<div class="alb-cover"><Cover artwork={album.artwork ?? items[0]?.artwork} seed={album.title} min={300} class="cv" eager /></div>
			<div class="alb-txt">
				<p class="caps">{isPlaylist ? 'Playlist' : album.kind === 'single' ? 'Single' : 'Album'}</p>
				<h1 class="alb-title">{album.title}</h1>
				<p class="alb-meta">
					{#if artistTo}<a class="alb-by" href={artistHref(artistTo)}>{album.artist}</a>{:else}<b>{album.artist}</b>{/if}{#if album.year}<span class="sep">{' · '}</span><span>{album.year}</span>{/if}<span class="sep">{' · '}</span><span>{songs(items.length)}{#if items.length}, {totalLength(items)}{/if}</span>
				</p>
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {album.title}" disabled={!items.length} onclick={() => play()}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {album.title}" title="Shuffle" disabled={!items.length} onclick={() => { player.setShuffle(true); play(); }}><Icon name="shuffle" /></button>
			<button type="button" class="act-ib" aria-label="Add {album.title} to Up next" title="Add to Up next" disabled={!items.length} onclick={upNext}><Icon name="queue_play_next" /></button>
			{#if foreign}
				{#if allKept}
					<button type="button" class="act-ib act-on" aria-label="{album.title} is in your library" title="In your library" onclick={() => ui.toast(`“${album?.title}” is in your library`)}><Icon name="library_add_check" /></button>
				{:else}
					<button type="button" class="act-ib" aria-label="Add {album.title} to your library" title="Add to your library" disabled={!items.length || keeping || kept === null} onclick={() => void keepAlbum()}><Icon name="library_add" /></button>
				{/if}
			{/if}
			<button type="button" class="act-ib" aria-label="Save {album.title} as a playlist" title="Save as a playlist" disabled={!items.length} onclick={() => void save()}><Icon name="playlist_add" /></button>
		</div>
		{#if items.length}
			<div class="trk-list">
				{#each items as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} cover={isPlaylist} album={isPlaylist} {pick} at={i} onplay={() => play(i)} />
				{/each}
			</div>
			{#if album.more}<p class="s-hint">The first {items.length} songs of this playlist are shown.</p>{/if}
		{:else}
			<p class="s-hint">This {isPlaylist ? 'playlist' : 'album'} has no songs to show.</p>
		{/if}
	{/if}
</div>

<style>
	.act-on { color: var(--acc); }
</style>
