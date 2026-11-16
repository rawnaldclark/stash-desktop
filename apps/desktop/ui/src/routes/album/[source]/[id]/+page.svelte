<script lang="ts">
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { browse, library, player } from '../../../../lib/app';
	import type { Album, LibraryItem } from '../../../../lib/api';
	import { Refused } from '../../../../lib/core';
	import { artistHref, artistLink, LIB_REF, toLibraryItem } from '../../../../lib/identity';
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

	// The album's songs are rows of the library: each carries its row (`refs.lib`) and the library's own album and artist pages.
	const items = $derived<LibraryItem[]>(album ? album.tracks.map((t) => toLibraryItem(t, { [LIB_REF]: t.id }, Date.now(), sid)) : []);
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
				<p class="caps">Album</p>
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
			<button type="button" class="act-ib" aria-label="Save {album.title} as a playlist" title="Save as a playlist" disabled={!items.length} onclick={() => void save()}><Icon name="playlist_add" /></button>
		</div>
		{#if items.length}
			<div class="trk-list">
				{#each items as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} cover={false} {pick} at={i} onplay={() => play(i)} />
				{/each}
			</div>
		{:else}
			<p class="s-hint">This album has no songs to show.</p>
		{/if}
	{/if}
</div>
