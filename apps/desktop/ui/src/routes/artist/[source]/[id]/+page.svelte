<script lang="ts">
	import { shelf } from '../../../../lib/ui/shelf';
	import { page } from '$app/state';
	import { browse, player } from '../../../../lib/app';
	import type { Artist, LibraryItem } from '../../../../lib/api';
	import { Refused } from '../../../../lib/core';
	import { albumHref, LIB_REF, toLibraryItem } from '../../../../lib/identity';
	import BrowseMsg from '../../../../lib/ui/BrowseMsg.svelte';
	import Card from '../../../../lib/ui/Card.svelte';
	import Cover from '../../../../lib/ui/Cover.svelte';
	import Icon from '../../../../lib/ui/Icon.svelte';
	import TrackRow from '../../../../lib/ui/TrackRow.svelte';
	import { songs } from '../../../../lib/ui/format';

	const sid = $derived(page.params.source ?? '');
	const id = $derived(page.params.id ?? '');
	let artist = $state.raw<Artist | null>(null);
	let status = $state<'loading' | 'ok' | 'missing' | 'failed'>('loading');
	let seq = 0;

	function load() {
		const mine = ++seq;
		const [s, i] = [sid, id];
		status = 'loading';
		artist = null;
		browse.artist(s, i).then(
			(a) => {
				if (mine !== seq) return;
				artist = a;
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

	// The artist's songs are rows of the library: each carries its row (`refs.lib`) and the library's own album and artist pages.
	const items = $derived<LibraryItem[]>(artist ? artist.topTracks.map((t) => toLibraryItem(t, { [LIB_REF]: t.id }, Date.now(), sid)) : []);
	const play = (i = 0) => void player.playItems(items, i, { from: artist?.name });
	const albumSub = (a: Artist['albums'][number]) => [a.year, a.trackCount !== undefined ? songs(a.trackCount) : ''].filter(Boolean).join(' · ');
</script>

<div class="view view-in view-artist">
	{#if status !== 'ok' || !artist}
		<BrowseMsg kind="artist" state={status === 'ok' ? 'loading' : status} retry={load} />
	{:else}
		<div class="alb-top">
			<div class="alb-cover art-round"><Cover artwork={artist.artwork} seed={artist.name} min={300} class="cv" eager /></div>
			<div class="alb-txt">
				<p class="caps">Artist</p>
				<h1 class="alb-title">{artist.name}</h1>
				{#if artist.albums.length}<p class="alb-meta"><b>{artist.albums.length === 1 ? '1 album' : `${artist.albums.length} albums`}</b></p>{/if}
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {artist.name}" disabled={!items.length} onclick={() => play()}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {artist.name}" title="Shuffle" disabled={!items.length} onclick={() => { player.setShuffle(true); play(); }}><Icon name="shuffle" /></button>
		</div>
		{#if items.length}
			<h2 class="vh2">Songs</h2>
			<div class="trk-list">
				{#each items as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} onplay={() => play(i)} />
				{/each}
			</div>
		{/if}
		{#if artist.albums.length}
			<h2 class="vh2">Albums</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each artist.albums as al (al.id)}
						<Card title={al.title} sub={albumSub(al)} artwork={al.artwork} href={albumHref({ sourceId: sid, id: al.id })} />
					{/each}
				</div>
			</div>
		{/if}
		{#if !items.length && !artist.albums.length}
			<p class="s-hint">There is nothing to show for this artist yet.</p>
		{/if}
	{/if}
</div>
