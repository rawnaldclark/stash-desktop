<script lang="ts">
	import { page } from '$app/state';
	import { player, searchApi } from '../../lib/app';
	import type { LibraryItem, SearchResult } from '../../lib/api';
	import { albumHref, artistHref } from '../../lib/identity';
	import Card from '../../lib/ui/Card.svelte';
	import Empty from '../../lib/ui/Empty.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import Cover from '../../lib/ui/Cover.svelte';
	import TrackRow from '../../lib/ui/TrackRow.svelte';
	import { shelf } from '../../lib/ui/shelf';
	import { openSongMenu, songMenu } from '../../lib/ui/song-menu';
	import { clearRecent, loadRecent, recentId, removeRecent, saveRecent, saveRecentSong, L, ui, type RecentEntry } from '../../lib/ui/state.svelte';

	const q = $derived((page.url.searchParams.get('q') ?? '').trim());
	let result = $state.raw<SearchResult | null>(null);
	let loading = $state(false);
	let failedAll = $state(false);
	let open = $state<Record<string, boolean>>({});
	let recent = $state(loadRecent());

	// Keep the header box in step with the address (back/forward).
	$effect(() => {
		if (document.activeElement?.id !== 'q') ui.q = q;
	});

	$effect(() => {
		const query = q;
		if (!query) {
			result = null;
			loading = false;
			failedAll = false;
			recent = loadRecent();
			return;
		}
		// A new query lets the one before it go: its answers (an add-on's can take seconds) are not waited for.
		const stop = new AbortController();
		loading = true;
		searchApi.search(query, { signal: stop.signal, onLibrary: (r) => !stop.signal.aborted && ((result = r), (failedAll = false), (loading = false)) }).then(
			(r) => {
				if (stop.signal.aborted) return;
				result = r;
				failedAll = false;
				loading = false;
				if (r.top.length) saveRecent(query);
			},
			() => {
				if (stop.signal.aborted) return;
				result = null;
				failedAll = true; // the core did not answer
				loading = false;
			}
		);
		return () => stop.abort();
	});

	const topItems = $derived<LibraryItem[]>(result?.top ?? []);
	const sections = $derived(result?.bySource ?? []);
	const pick = $derived(result?.topPick ?? { kind: 'track' as const });
	// The song card is the first song; behind an artist or album card the songs start at the first one.
	const songStart = $derived(pick.kind === 'track' ? 1 : 0);
	const top = $derived(pick.kind === 'track' ? topItems[0] : undefined);
	const topRest = $derived(topItems.slice(songStart, songStart + 5));
	const nothing = $derived(!!result && !result.pending.length && topItems.length === 0 && result.artists.length === 0 && result.albums.length === 0 && sections.every((s) => s.items.length === 0));
	/** Two letters for an artist with no picture ("Vince Staples" -> "VS"). */
	const initials = (name: string) => name.replace(/[([{].*?[)\]}]/g, ' ').split(/\s+/).filter((w) => /\p{L}/u.test(w)).slice(0, 2).map((w) => w.match(/\p{L}/u)![0].toUpperCase()).join('');
	/** The add-ons that gave no results, each with its own words for why. */
	const failed = $derived(result?.failed ?? []);
	const sentence = (t: string) => (/[.!?]$/.test(t) ? t : `${t}.`);
	/** Add-ons are asked too: the hint under an empty search box says so. */
	const withAddons = $derived(L.sources.some((a) => a.enabled && !a.problem && a.resources.includes('search')));

	// A tapped result plays on its own: the rest of the results are not queued behind it.
	const play = (items: LibraryItem[], i: number) => {
		const it = items[i];
		if (!it) return;
		saveRecentSong(it);
		void player.playItems([it], 0, { from: 'Search' });
	};
	const playRecent = (e: Extract<RecentEntry, { kind: 'song' }>) => void player.playItems([e.item], 0, { from: 'Search' });
	const forget = (e: RecentEntry) => {
		removeRecent(e);
		recent = loadRecent();
	};
	const clearAll = () => {
		clearRecent();
		recent = [];
	};
	const SHOW = 5;
</script>

<div class="view view-in view-search">
	{#if !q}
		{#if recent.length}
			<div class="rs-head">
				<p class="caps">Recent searches</p>
				<button type="button" class="rs-clear" onclick={clearAll}>Clear all</button>
			</div>
			<ul class="rs-list">
				{#each recent as r (recentId(r))}
					<li class="rs-row" use:songMenu={r.kind === 'song' ? { item: r.item } : null}>
						{#if r.kind === 'query'}
							<a class="rs-main" href="/search?q={encodeURIComponent(r.q)}">
								<span class="rs-pic"><Icon name="history" /></span>
								<span class="rs-txt"><b>{r.q}</b></span>
							</a>
						{:else}
							<button type="button" class="rs-main" onclick={() => playRecent(r)}>
								<span class="rs-pic"><Cover artwork={r.item.artwork} seed={r.item.title} min={100} class="cv" /></span>
								<span class="rs-txt"><b>{r.item.title}</b><i>{r.item.artist}</i></span>
							</button>
						{/if}
						<button type="button" class="rs-x" aria-label="Remove from recent searches" title="Remove" onclick={() => forget(r)}><Icon name="close" /></button>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="s-hint">{withAddons ? 'Songs, albums and artists from your library, and songs from every add-on you have.' : 'Songs, albums and artists from your library.'}</p>
		{/if}
	{:else if loading && !result}
		<p class="s-hint" role="status">Searching</p>
	{:else if failedAll}
		<Empty line="The Stash core is not answering." />
	{:else if nothing}
		<p class="s-hint" role="status">Nothing found for “{q}”. Try fewer words or another spelling.</p>
		{#each failed as f (f.sourceId)}<p class="s-hint">{f.name} gave no results: {sentence(f.message)}</p>{/each}
	{:else if result}
		<div class="sr">
			{#if top || pick.kind !== 'track'}
				<div class="sr-top">
					<div class="top-col">
						<h2 class="vh2">Top result</h2>
						{#if pick.kind === 'artist'}
							<div class="top-card">
								<a class="top-main" href={artistHref({ sourceId: pick.artist.sourceId, id: pick.artist.id })}>
									<span class="top-cv"><Cover artwork={pick.artist.artwork} seed={pick.artist.name} min={200} class="cv" ini={initials(pick.artist.name)} /></span>
									<span class="top-kind">Artist</span>
									<b>{pick.artist.name}</b>
								</a>
							</div>
						{:else if pick.kind === 'album'}
							<div class="top-card">
								<a class="top-main" href={albumHref({ sourceId: pick.album.sourceId, id: pick.album.id })}>
									<span class="top-cv"><Cover artwork={pick.album.artwork} seed={pick.album.title} min={200} class="cv" /></span>
									<span class="top-kind">Album</span>
									<b>{pick.album.title}</b>
									<i>{pick.album.artist}</i>
								</a>
							</div>
						{:else if top}
							<div class="top-card" use:songMenu={{ item: top }}>
								<button type="button" class="top-main" onclick={() => play(topItems, 0)} aria-label="Play {top.title} by {top.artist}">
									<span class="top-cv"><Cover artwork={top.artwork} seed={top.title} min={200} class="cv" /></span>
									<span class="top-kind">Song</span>
									<b>{top.title}</b>
									<i>{top.artist}</i>
								</button>
								<span class="top-play" aria-hidden="true"><Icon name="play_arrow" /></span>
								<button type="button" class="more top-more" aria-label="More for {top.title}" aria-haspopup="menu" onclick={(e) => openSongMenu({ item: top }, e.currentTarget)}><Icon name="more_horiz" /></button>
							</div>
						{/if}
					</div>
					{#if topRest.length}
						<div class="songs-col">
							<h2 class="vh2">Songs</h2>
							<div class="sr-songs">
								{#each topRest as it, i (it.key)}
									<TrackRow item={it} onplay={() => play(topItems, i + songStart)} />
								{/each}
							</div>
						</div>
					{/if}
				</div>
			{/if}
			{#if result.artists.length}
				<h2 class="vh2 sec-h">Artists</h2>
				<div class="shelf-w" use:shelf>
					<div class="shelf shelf-artists">
						{#each result.artists as a (a.sourceId + a.id)}
							<Card title={a.name} artwork={a.artwork} href={artistHref({ sourceId: a.sourceId, id: a.id })} ini={initials(a.name)} />
						{/each}
					</div>
				</div>
			{/if}
			{#if result.albums.length}
				<h2 class="vh2 sec-h">Albums</h2>
				<div class="shelf-w" use:shelf>
					<div class="shelf shelf-albums">
						{#each result.albums as al (al.sourceId + al.id)}
							<Card title={al.title} sub={[al.year, al.artist].filter(Boolean).join(' · ')} artwork={al.artwork} href={albumHref({ sourceId: al.sourceId, id: al.id })} />
						{/each}
					</div>
				</div>
			{/if}
			{#each sections as sec (sec.sourceId)}
				{#if sec.items.length}
					<h2 class="vh2 sec-h">{sec.name}</h2>
					<div class="sec-list">
						{#each open[sec.sourceId] ? sec.items : sec.items.slice(0, SHOW) as it, i (it.key + i)}
							<TrackRow item={it} album tag={sec.tags?.[it.refs[sec.sourceId] ?? '']} onplay={() => play(sec.items, i)} />
						{/each}
					</div>
					{#if sec.items.length > SHOW}
						<button type="button" class="more-btn" onclick={() => (open[sec.sourceId] = !open[sec.sourceId])}>{open[sec.sourceId] ? 'Show fewer' : `Show all ${sec.items.length}`}</button>
					{/if}
				{/if}
			{/each}
			{#if result.pending.length}
				<p class="s-hint" role="status">Asking {result.pending.join(', ')}…</p>
			{/if}
			{#each failed as f (f.sourceId)}<p class="s-hint">{f.name} gave no results: {sentence(f.message)}</p>{/each}
		</div>
	{/if}
</div>
