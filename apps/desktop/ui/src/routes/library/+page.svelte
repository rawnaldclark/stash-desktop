<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { library, player, saveLiked } from '../../lib/app';
	import type { HistoryEntry, LibraryItem, Playlist } from '../../lib/api';
	import { measuring } from '../../lib/core';
	import Albums from '../../lib/desktop/Albums.svelte';
	import Artists from '../../lib/desktop/Artists.svelte';
	import Downloads from '../../lib/desktop/Downloads.svelte';
	import NotFound from '../../lib/desktop/NotFound.svelte';
	import { NF } from '../../lib/desktop/unfound.svelte';
	import Songs from '../../lib/desktop/Songs.svelte';
	import { fold } from '../../lib/identity';
	import Card from '../../lib/ui/Card.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import TrackRow from '../../lib/ui/TrackRow.svelte';
	import { L, LS, ui } from '../../lib/ui/state.svelte';
	import { dayLabel, songs } from '../../lib/ui/format';

	const TABS = ['songs', 'likes', 'playlists', 'albums', 'artists', 'downloads', 'notfound', 'history'] as const;
	type Tab = (typeof TABS)[number];
	const tab = $derived<Tab>(TABS.find((t) => t === page.url.searchParams.get('tab')) ?? 'songs');

	const tabs: [Tab, string][] = [
		['songs', 'Songs'],
		['likes', 'Liked'],
		['playlists', 'Playlists'],
		['albums', 'Albums'],
		['artists', 'Artists'],
		['downloads', 'Downloads'],
		['notfound', 'Not found'],
		['history', 'History']
	];
	// Not found shows only while songs wait on it.
	const shownTabs = $derived(tabs.filter(([id]) => id !== 'notfound' || NF.open > 0 || tab === 'notfound'));
	$effect(() => {
		void NF.refresh();
	});

	// Sort orders, remembered in this window per tab.
	const LIKE_SORTS = { recent: 'Recently added', title: 'Title A–Z', artist: 'Artist A–Z', plays: 'Most played', duration: 'Duration' } as const;
	const PL_SORTS = { created: 'Oldest first', recent: 'Recently added', name: 'Name A–Z', updated: 'Recently changed' } as const;
	type LikeSort = keyof typeof LIKE_SORTS;
	type PlSort = keyof typeof PL_SORTS;
	const pick = <T extends string>(v: string | null, all: Record<T, string>, d: T): T => (v && v in all ? (v as T) : d);
	let likeSort = $state<LikeSort>(pick(LS.get('stash.sort.likes'), LIKE_SORTS, 'recent'));
	let plSort = $state<PlSort>(pick(LS.get('stash.sort.playlists'), PL_SORTS, 'created'));
	$effect(() => LS.set('stash.sort.likes', likeSort));
	$effect(() => LS.set('stash.sort.playlists', plSort));

	let q = $state('');
	const needle = $derived(fold(q));
	const hit = (i: LibraryItem) => !needle || fold(`${i.title} ${i.artist} ${i.album ?? ''}`).includes(needle);
	const byText = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: 'base', numeric: true });

	const plays = $derived.by(() => {
		const m = new Map<string, number>();
		for (const h of L.history) m.set(h.item.key, (m.get(h.item.key) ?? 0) + 1);
		return m;
	});
	const likes = $derived.by(() => {
		const l = L.likes.filter(hit);
		if (likeSort === 'title') return [...l].sort((a, b) => byText(a.title, b.title));
		if (likeSort === 'artist') return [...l].sort((a, b) => byText(a.artist, b.artist) || byText(a.title, b.title));
		if (likeSort === 'duration') return [...l].sort((a, b) => (b.durationMs ?? 0) - (a.durationMs ?? 0));
		if (likeSort === 'plays') return [...l].sort((a, b) => (plays.get(b.key) ?? 0) - (plays.get(a.key) ?? 0));
		return l;
	});
	const playlists = $derived.by(() => {
		const l = L.playlists.filter((p: Playlist) => !needle || fold(p.name).includes(needle) || p.items.some(hit));
		if (plSort === 'name') return [...l].sort((a, b) => byText(a.name, b.name));
		if (plSort === 'recent') return [...l].sort((a, b) => b.createdAt - a.createdAt);
		if (plSort === 'updated') return [...l].sort((a, b) => b.updatedAt - a.updatedAt);
		return l;
	});
	const historyAll = $derived(L.history.filter((h) => hit(h.item)));
	// History can hold hundreds of plays: show them a page at a time.
	const HIST_PAGE = 300;
	let histShown = $state(HIST_PAGE);
	const history = $derived(historyAll.slice(0, histShown));

	let naming = $state(false);
	let name = $state('');

	const groups = $derived.by(() => {
		const out: { day: string; entries: HistoryEntry[] }[] = [];
		for (const h of history) {
			const day = dayLabel(h.playedAt);
			const last = out[out.length - 1];
			if (last?.day === day) last.entries.push(h);
			else out.push({ day, entries: [h] });
		}
		return out;
	});
	const historyItems = $derived(history.map((h) => h.item));

	const play = (items: LibraryItem[], i = 0, from = 'Your likes') => void player.playItems(items, i, { from });
	async function create() {
		try {
			const p = await library.createPlaylist(name);
			naming = false;
			name = '';
			await goto(`/playlist/${p.id}`);
		} catch {
			/* said by the shell (the core's own words) */
		}
	}
	/** The tabs that are searched from here: Songs asks the core; Liked, Playlists and History filter what is listed. */
	const findable = $derived(tab === 'songs' ? (L.counts?.tracks ?? 0) > 0 : tab === 'likes' ? L.likes.length > 0 : tab === 'playlists' ? true : tab === 'history' ? L.history.length > 0 : false);
</script>

<div class="view view-in view-library">
	<h1 class="vh2 lib-title">Your library</h1>
	<nav class="lib-chips" aria-label="Library sections">
		{#each shownTabs as [id, label] (id)}
			<a class="chip" class:on={tab === id} href="/library?tab={id}" aria-current={tab === id ? 'page' : undefined}>{label}</a>
		{/each}
	</nav>

	{#if findable && !measuring}
		<div class="lib-tools">
			<label class="lib-find"><Icon name="search" /><input type="search" placeholder="Search your library" aria-label="Search your library" bind:value={q} maxlength="100" /></label>
			{#if tab === 'likes'}
				<div class="lib-sort" role="group" aria-label="Sort your likes">{#each Object.entries(LIKE_SORTS) as [k, v] (k)}<button type="button" class="chip" class:on={likeSort === k} aria-pressed={likeSort === k} onclick={() => (likeSort = k as LikeSort)}>{v}</button>{/each}</div>
			{:else if tab === 'playlists'}
				<div class="lib-sort" role="group" aria-label="Sort your playlists">{#each Object.entries(PL_SORTS) as [k, v] (k)}<button type="button" class="chip" class:on={plSort === k} aria-pressed={plSort === k} onclick={() => (plSort = k as PlSort)}>{v}</button>{/each}</div>
			{:else if tab === 'songs' && L.counts}
				<span class="lib-count">{L.counts.tracks.toLocaleString()} {L.counts.tracks === 1 ? 'song' : 'songs'}</span>
			{/if}
		</div>
	{/if}

	{#if tab === 'songs'}
		<!-- While the window measures itself, the header's search box filters this list in place. -->
		<Songs query={measuring ? ui.q : q} />
	{:else if tab === 'likes'}
		{#if L.likes.length}
			<div class="lib-actions">
				<button type="button" class="act-play" aria-label="Play your likes" disabled={!likes.length} onclick={() => play(likes)}><Icon name="play_arrow" /></button>
				<button type="button" class="act-ib" aria-label="Shuffle your likes" title="Shuffle" disabled={!likes.length} onclick={() => { player.setShuffle(true); play(likes); }}><Icon name="shuffle" /></button>
				<button type="button" class="act-ib" aria-label="Save your likes to this computer" title="Download" onclick={() => void saveLiked().then((said) => ui.toast(said), () => {})}><Icon name="download" /></button>
				<span class="lib-count">{needle ? `${likes.length} of ${songs(L.likes.length)}` : songs(L.likes.length)}</span>
			</div>
			<div class="trk-list">
				{#each likes as it, i (it.refs.lib ?? it.key)}
					<TrackRow item={it} num={i + 1} album onplay={() => play(likes, i)} />
				{/each}
			</div>
			{#if !likes.length}<p class="s-hint">Nothing in your likes matches “{q.trim()}”.</p>{/if}
		{:else if L.loaded}
			<p class="s-hint">Songs you like land here. Press the heart on any song.</p>
		{/if}
	{:else if tab === 'playlists'}
		<div class="cards">
			{#if !needle}
				<button type="button" class="card card-new" onclick={() => (naming = true)} aria-label="New playlist"><span class="tile new-tile"><Icon name="add" /></span><b>New playlist</b></button>
				<button type="button" class="card card-new" onclick={() => (ui.smart = { id: null })} aria-label="New smart playlist"><span class="tile new-tile"><Icon name="queue_music" /></span><b>New smart playlist</b></button>
			{/if}
			{#each playlists as p (p.id)}
				<Card title={p.name} sub={p.rules ? `Smart · ${songs(p.items.length)}` : p.followed ? `Mix · ${songs(p.items.length)}` : songs(p.items.length)} items={p.items} href="/playlist/{p.id}" />
			{/each}
		</div>
		{#if needle && !playlists.length}<p class="s-hint">No playlist matches “{q.trim()}”.</p>{/if}
		{#if naming}
			<form class="new-form" onsubmit={(e) => { e.preventDefault(); void create(); }}>
				<!-- svelte-ignore a11y_autofocus -->
				<input aria-label="Playlist name" placeholder="Playlist name" bind:value={name} maxlength="80" autofocus />
				<button type="submit" class="btn" disabled={!name.trim()}>Create</button>
				<button type="button" class="btn ghost" onclick={() => (naming = false)}>Cancel</button>
			</form>
		{/if}
	{:else if tab === 'albums'}
		{#key L.changed}<Albums />{/key}
	{:else if tab === 'artists'}
		{#key L.changed}<Artists />{/key}
	{:else if tab === 'downloads'}
		<Downloads />
	{:else if tab === 'notfound'}
		{#key page.url.search}<NotFound />{/key}
	{:else if groups.length}
		{#each groups as g (g.day)}
			<p class="caps hist-day">{g.day}</p>
			<div class="trk-list">
				{#each g.entries as h, i (h.playedAt + h.item.key + i)}
					<TrackRow item={h.item} album onplay={() => play(historyItems, historyItems.indexOf(h.item), 'History')} />
				{/each}
			</div>
		{/each}
		{#if historyAll.length > history.length}
			<button type="button" class="btn ghost hist-more" onclick={() => (histShown += HIST_PAGE)}>Show more</button>
		{/if}
	{:else if L.history.length}
		<p class="s-hint">Nothing you played matches “{q.trim()}”.</p>
	{:else if L.loaded}
		<p class="s-hint">Nothing played yet. What you listen to shows up here.</p>
	{/if}
</div>

<style>
	.lib-tools { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; margin: 0 0 14px; }
	.hist-more { margin: 16px 8px 0 0; }
	.lib-find { display: flex; align-items: center; gap: 8px; flex: 1 1 220px; max-width: 360px; height: 38px; padding: 0 12px; border-radius: 10px; background: var(--wp-field); border: 1px solid var(--wp-line); color: var(--wp-text-2); }
	.lib-find:focus-within { outline: 2px solid var(--focus); outline-offset: 2px; }
	.lib-find :global(.ic) { width: 18px; height: 18px; flex: none; }
	.lib-find input { flex: 1; min-width: 0; height: 100%; border: 0; background: none; color: var(--wp-text); font: 400 14px/1 var(--font-body); outline: none; }
	.lib-sort { display: flex; flex-wrap: wrap; gap: 6px; }
</style>
