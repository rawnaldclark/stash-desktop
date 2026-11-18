<script lang="ts">
	import { shelf } from '../../../../lib/ui/shelf';
	import { page } from '$app/state';
	import { browse, player } from '../../../../lib/app';
	import type { Artist, LibraryItem } from '../../../../lib/api';
	import { rawArt, Refused, YOUTUBE } from '../../../../lib/core';
	import { L } from '../../../../lib/ui/state.svelte';
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
	// A library artist's page in the catalogue (YouTube Music, else an extension that keeps one): all their albums and
	// singles, not only the ones you have.
	let tube = $state.raw<(Artist & { sourceId: string }) | null>(null);
	let status = $state<'loading' | 'ok' | 'missing' | 'failed'>('loading');
	let seq = 0;

	function load() {
		const mine = ++seq;
		const [s, i] = [sid, id];
		status = 'loading';
		artist = null;
		tube = null;
		browse.artist(s, i).then(
			(a) => {
				if (mine !== seq) return;
				artist = a;
				status = 'ok';
				if (s === LIBRARY_SOURCE)
					void browse.catalogueArtist(a.name).then((t) => {
						if (mine === seq) tube = t;
					});
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
	// What the header counts: all their albums when the catalogue has them.
	const told = $derived(tube ?? artist);
	const albumSub = (a: Artist['albums'][number]) => [a.year, a.trackCount !== undefined ? songs(a.trackCount) : ''].filter(Boolean).join(' · ');

	// The artist a follow is for: YouTube Music's own page, or (a library artist) the one it has of that name. Their new
	// records come to Home.
	const followable = $derived.by(() => {
		if (!artist) return null;
		if (sid === YOUTUBE) return { id, name: artist.name, art: rawArt(artist.artwork?.[0]?.url) };
		if (tube && tube.sourceId === YOUTUBE) return { id: tube.id, name: artist.name, art: rawArt(tube.artwork?.[0]?.url) };
		return null;
	});
	const followed = $derived(followable ? L.isFollowed(YOUTUBE, followable.id) : false);
	let following = $state(false);
	async function follow() {
		if (!followable || following) return;
		following = true;
		try {
			await L.toggleFollow(YOUTUBE, followable.id, followable.name, followable.art);
		} finally {
			following = false;
		}
	}
</script>

<div class="view view-in view-artist">
	{#if status !== 'ok' || !artist}
		<BrowseMsg kind="artist" state={status === 'ok' ? 'loading' : status} retry={load} />
	{:else}
		<div class="alb-top">
			<div class="alb-cover art-round"><Cover artwork={artist.artwork ?? tube?.artwork} seed={artist.name} min={300} class="cv" eager /></div>
			<div class="alb-txt">
				<p class="caps">Artist</p>
				<h1 class="alb-title">{artist.name}</h1>
				{#if told && told.albums.length}<p class="alb-meta"><b>{told.more?.albums ? `${told.albums.length}+ albums` : told.albums.length === 1 ? '1 album' : `${told.albums.length} albums`}</b></p>{/if}
			</div>
		</div>
		<div class="alb-act">
			<button type="button" class="act-play" aria-label="Play {artist.name}" disabled={!items.length} onclick={() => play()}><Icon name="play_arrow" /></button>
			<button type="button" class="act-ib" aria-label="Shuffle {artist.name}" title="Shuffle" disabled={!items.length} onclick={() => { player.setShuffle(true); play(); }}><Icon name="shuffle" /></button>
			{#if followable}
				{#if followed}
					<button type="button" class="act-ib act-on" aria-label="Stop following {artist.name}" title="Following: stop following" disabled={following} onclick={() => void follow()}><Icon name="how_to_reg" /></button>
				{:else}
					<button type="button" class="act-ib" aria-label="Follow {artist.name}" title="Follow: their new releases come to Home" disabled={following} onclick={() => void follow()}><Icon name="person_add" /></button>
				{/if}
			{/if}
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
			<h2 class="vh2">{tube?.albums.length ? 'In your library' : 'Albums'} {#if artist.more?.albums}<a class="vh2-more" href="{artistHref({ sourceId: sid, id })}/albums">Show all</a>{/if}</h2>
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
		{#if tube}
			{@const tid = tube.id}
			{@const tsid = tube.sourceId}
			{#if tube.albums.length}
				<h2 class="vh2">Albums {#if tube.more?.albums}<a class="vh2-more" href="{artistHref({ sourceId: tsid, id: tid })}/albums">Show all</a>{/if}</h2>
				<div class="shelf-w" use:shelf>
					<div class="shelf">
						{#each tube.albums as al (al.id)}
							<Card title={al.title} sub={albumSub(al)} artwork={al.artwork} href={albumHref({ sourceId: tsid, id: al.id })} />
						{/each}
					</div>
				</div>
			{/if}
			{#if tube.singles?.length}
				<h2 class="vh2">Singles and EPs {#if tube.more?.singles}<a class="vh2-more" href="{artistHref({ sourceId: tsid, id: tid })}/singles">Show all</a>{/if}</h2>
				<div class="shelf-w" use:shelf>
					<div class="shelf">
						{#each tube.singles as al (al.id)}
							<Card title={al.title} sub={albumSub(al)} artwork={al.artwork} href={albumHref({ sourceId: tsid, id: al.id })} />
						{/each}
					</div>
				</div>
			{/if}
			{#if tube.related?.length}
				<h2 class="vh2">Fans also like</h2>
				<div class="shelf-w" use:shelf>
					<div class="shelf shelf-artists">
						{#each tube.related as r (r.id)}
							<Card title={r.name} artwork={r.artwork} href={artistHref({ sourceId: tsid, id: r.id })} ini={r.name.slice(0, 2).toUpperCase()} />
						{/each}
					</div>
				</div>
			{/if}
			{#if tsid === YOUTUBE}<p class="s-hint tube-all"><a class="lnk" href={artistHref({ sourceId: tsid, id: tid })}>{artist.name} on YouTube Music</a></p>{/if}
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
		{#if !items.length && !artist.albums.length && !withItems.length && !tube}
			<p class="s-hint">There is nothing to show for this artist yet.</p>
		{/if}
	{/if}
</div>

<style>
	.tube-all { margin-top: 24px; }
	.act-on { color: var(--acc); }
</style>
