<script lang="ts">
	import { shelf } from '../../../../lib/ui/shelf';
	import { page } from '$app/state';
	import { browse, player } from '../../../../lib/app';
	import type { Artist, LibraryItem } from '../../../../lib/api';
	import { Refused } from '../../../../lib/core';
	import { albumHref, artistHref, LIB_REF, LIBRARY_SOURCE, toLibraryItem } from '../../../../lib/identity';
	import type { Track } from '../../../../lib/sources/contract';
	import BrowseMsg from '../../../../lib/ui/BrowseMsg.svelte';
	import Card from '../../../../lib/ui/Card.svelte';
	import Cover from '../../../../lib/ui/Cover.svelte';
	import Icon from '../../../../lib/ui/Icon.svelte';
	import { R } from '../../../../lib/desktop/radio.svelte';
	import TrackRow from '../../../../lib/ui/TrackRow.svelte';
	import type { PickList } from '../../../../lib/ui/pick.svelte';
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
	// (YouTube Music's artist: the songs are that source's, `refs.youtube`, not on this computer.)
	const asItem = (t: Track): LibraryItem => (sid === LIBRARY_SOURCE ? toLibraryItem(t, { [LIB_REF]: t.id }, Date.now(), sid) : { ...toLibraryItem(t, { [sid]: t.id }, 0, sid), here: false });
	const items = $derived<LibraryItem[]>(artist ? artist.topTracks.map(asItem) : []);
	const play = (i = 0) => void player.playItems(items, i, { from: artist?.name });
	// Their songs with others ("A & B", "B feat. A"): a list of its own.
	const withItems = $derived<LibraryItem[]>(artist ? (artist.withOthers ?? []).map(asItem) : []);
	const playWith = (i: number) => void player.playItems(withItems, i, { from: artist?.name });
	const pickWith: PickList = {
		get scope() {
			return `artist-with:${sid}:${id}`;
		},
		count: () => withItems.length,
		between: (a, b) => withItems.slice(a, b + 1),
		get from() {
			return artist?.name;
		}
	};
	const pick: PickList = {
		get scope() {
			return `artist:${sid}:${id}`;
		},
		count: () => items.length,
		between: (a, b) => items.slice(a, b + 1),
		get from() {
			return artist?.name;
		}
	};
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
				{#if artist.albums.length}<p class="alb-meta"><b>{artist.more?.albums ? `${artist.albums.length}+ albums` : artist.albums.length === 1 ? '1 album' : `${artist.albums.length} albums`}</b></p>{/if}
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {artist.name}" disabled={!items.length} onclick={() => play()}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {artist.name}" title="Shuffle" disabled={!items.length} onclick={() => { player.setShuffle(true); play(); }}><Icon name="shuffle" /></button>
			{#if R.ready}
				{@const name = artist.name}
				<button type="button" class="act-ib" aria-label="Start {name} radio" title="Radio" onclick={() => void player.startRadio({ artist: name })}><Icon name="radio" /></button>
			{/if}
		</div>
		{#if items.length}
			<h2 class="vh2">Songs {#if artist.more?.songs}<a class="vh2-more" href={albumHref({ sourceId: sid, id: artist.more.songs })}>Show all</a>{/if}</h2>
			<div class="trk-list">
				{#each items as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} {pick} at={i} onplay={() => play(i)} />
				{/each}
			</div>
		{/if}
		{#if artist.albums.length}
			<h2 class="vh2">Albums {#if artist.more?.albums}<a class="vh2-more" href="{artistHref({ sourceId: sid, id })}/albums">Show all</a>{/if}</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each artist.albums as al (al.id)}
						<Card title={al.title} sub={albumSub(al)} artwork={al.artwork} href={albumHref({ sourceId: sid, id: al.id })} />
					{/each}
				</div>
			</div>
		{/if}
		{#if artist.singles?.length}
			<h2 class="vh2">Singles and EPs {#if artist.more?.singles}<a class="vh2-more" href="{artistHref({ sourceId: sid, id })}/singles">Show all</a>{/if}</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each artist.singles as al (al.id)}
						<Card title={al.title} sub={albumSub(al)} artwork={al.artwork} href={albumHref({ sourceId: sid, id: al.id })} />
					{/each}
				</div>
			</div>
		{/if}
		{#if artist.related?.length}
			<h2 class="vh2">Fans also like</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf shelf-artists">
					{#each artist.related as r (r.id)}
						<Card title={r.name} artwork={r.artwork} href={artistHref({ sourceId: sid, id: r.id })} ini={r.name.slice(0, 2).toUpperCase()} />
					{/each}
				</div>
			</div>
		{/if}
		{#if withItems.length}
			<h2 class="vh2">With others</h2>
			<div class="trk-list">
				{#each withItems as it, i (it.key + i)}
					<TrackRow item={it} num={i + 1} pick={pickWith} at={i} onplay={() => playWith(i)} />
				{/each}
			</div>
		{/if}
		{#if !items.length && !artist.albums.length && !withItems.length}
			<p class="s-hint">There is nothing to show for this artist yet.</p>
		{/if}
	{/if}
</div>
