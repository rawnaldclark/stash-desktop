<!-- Library › Albums: the library's albums as the web player's cards, grouped by name, artist, decade, genre or when they
     came (the choice is remembered). Every album is read at once (a few thousand at most), and the grid is drawn a part
     at a time as it is scrolled. A card opens the album's page. -->
<script lang="ts">
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { fold } from '../identity';
	import { more } from './more';
	import { GROUPINGS, groupAlbums, isGroupBy, upToCards, type GroupBy } from './album-groups';

	/** What is typed in the library's search box: the grid shows only what matches it. `here`: only albums with a file. */
	let { q = '', here = false }: { q?: string; here?: boolean } = $props();
	const needle = $derived(fold(q));

	const KEY = 'stash.albums.group';
	const saved = typeof localStorage === 'undefined' ? null : localStorage.getItem(KEY);
	let by = $state<GroupBy>(isGroupBy(saved) ? saved : 'name');
	function choose(next: GroupBy) {
		by = next;
		try {
			localStorage.setItem(KEY, next);
		} catch {
			/* a full or closed store: the choice lasts this run */
		}
	}

	let cards = $state.raw<core.AlbumCard[] | null>(null);
	let failure = $state<string | null>(null);
	// Each read is numbered, and only the latest one's answer is kept: the window learns that Stash runs for local files
	// only just after this opens, and the first answer (every album, slower) must not land on top of the second.
	let reading = 0;
	async function read() {
		const mine = ++reading;
		const only = here;
		try {
			const got = await core.albumCards(only);
			if (mine !== reading) return;
			cards = got;
			failure = null;
		} catch (e) {
			if (mine === reading) failure = core.said(e);
		}
	}
	$effect(() => {
		void here;
		void read();
	});

	const PAGE = 120;
	let upTo = $state(PAGE);
	$effect(() => {
		void needle;
		void by;
		upTo = PAGE;
	});
	const matching = $derived(cards ? (needle ? cards.filter((a) => fold(`${a.title} ${a.artist} ${a.genre ?? ''}`).includes(needle)) : cards) : []);
	const groups = $derived(groupAlbums(matching, by, Date.now()));
	const shown = $derived(upToCards(groups, upTo));
	async function load(): Promise<boolean> {
		if (upTo >= matching.length) return false;
		upTo += PAGE;
		return true;
	}

	const art = (a: core.AlbumCard) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
	/** A card's second line says what its group does not. */
	const sub = (a: core.AlbumCard) => {
		const songs = here && a.songs_here < a.tracks ? `${a.songs_here} of ${a.tracks} songs` : null;
		if (by === 'artist') return [a.year, songs].filter(Boolean).join(' · ') || ' ';
		return [a.year, a.artist].filter(Boolean).join(' · ');
	};
	const count = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'album' : 'albums'}`;
</script>

<div class="ag-bar" role="radiogroup" aria-label="Group albums by">
	<span class="ag-lab">Group by</span>
	{#each GROUPINGS as g (g.id)}
		<button type="button" class="chip" class:on={by === g.id} role="radio" aria-checked={by === g.id} onclick={() => choose(g.id)}>{g.label}</button>
	{/each}
	{#if cards}<span class="ag-n">{count(matching.length)}</span>{/if}
</div>

{#if failure && !cards}
	<p class="s-hint" role="alert">{failure} <button type="button" class="lnk" onclick={() => void read()}>Try again</button></p>
{:else if !cards}
	<p class="s-hint">Reading your albums…</p>
{:else if !matching.length}
	<p class="s-hint">
		{needle ? `No album in your library matches “${q.trim()}”.` : here ? 'No albums on this computer yet. They show up here once your music folder has files with an album in their tags.' : 'No albums yet. Albums show up here once your library has songs.'}
	</p>
{:else}
	{#each shown as g (g.key)}
		<section class="ag" aria-label={g.label}>
			<h2 class="ag-h">{g.label} <span>{count(groups.find((x) => x.key === g.key)?.albums.length ?? g.albums.length)}</span></h2>
			<div class="cards">
				{#each g.albums as a (a.id)}
					<Card title={a.title} sub={sub(a)} artwork={art(a)} href="/album/library/{a.id}" />
				{/each}
			</div>
		</section>
	{/each}
	{#if upTo < matching.length}
		{#key `${needle}|${by}`}<div class="more" use:more={load} aria-hidden="true"></div>{/key}
	{/if}
{/if}

<style>
	.ag-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 0 0 14px; }
	.ag-lab { color: var(--wp-text-2); font: 500 13px/1 var(--font-body); margin-right: 2px; }
	.ag-n { margin-left: auto; color: var(--wp-text-2); font: 400 13px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	.ag + .ag { margin-top: 22px; }
	.ag-h { position: sticky; top: 0; z-index: 1; display: flex; align-items: baseline; gap: 10px; margin: 0 0 10px; padding: 6px 0; background: var(--wp-main); font: 600 17px/1.25 var(--font-display, var(--font-body)); color: var(--wp-text); }
	.ag-h span { color: var(--wp-text-2); font: 400 13px/1 var(--font-body); }
	.more { height: 1px; }
</style>
