<!-- Library › Songs as a table, the way a library manager shows a collection: a row a song, a column a tag, any column's
     heading sorts by it (again turns it round), a genre can be chosen, and the columns shown are the listener's choice.
     The rows come a page at a time from the core in the order asked (`/api/table`), so it is as quick with 100,000 songs
     as with a hundred. A click picks a row (Ctrl and Shift pick more), a double click plays from it. -->
<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import type { LibraryItem } from '../api';
	import { itemOfRow, player } from '../app';
	import * as core from '../core';
	import VirtualList from '../VirtualList.svelte';
	import Icon from '../ui/Icon.svelte';
	import { mmss } from '../ui/format';
	import { P, type PickList } from '../ui/pick.svelte';
	import { songMenu } from '../ui/song-menu';
	import { L, S } from '../ui/state.svelte';
	import { COLUMNS, formatOf, gridOf, readColumns, type ColumnId } from './song-table';

	let { query = '', here = false }: { query?: string; here?: boolean } = $props();

	const ROW_HEIGHT = 36;
	const FROM = 'Your library';

	let sort = $state<core.TableSort>((localStorage.getItem('stash.table.sort') as core.TableSort) || 'artist');
	let down = $state(localStorage.getItem('stash.table.down') === '1');
	let genre = $state<string | null>(null);
	let shown = $state<ColumnId[]>(readColumns(localStorage.getItem('stash.table.cols.2')));
	let choosing = $state(false);
	$effect(() => {
		localStorage.setItem('stash.table.sort', sort);
		localStorage.setItem('stash.table.down', down ? '1' : '0');
		localStorage.setItem('stash.table.cols.2', shown.join(','));
	});

	const ask = $derived<core.TableAsk>({ sort, down, q: query.trim() || undefined, genre: genre ?? undefined, here });
	const askKey = $derived(JSON.stringify(ask));

	let count = $state<number | null>(null);
	/** Files whose tags are still being read: the genres and years fill in as it goes. */
	let waiting = $state(0);
	let genres = $state.raw<core.GenreRow[]>([]);
	let failure = $state<string | null>(null);
	let scroller = $state<HTMLElement | null>(null);
	let list: VirtualList<core.TableRow> | undefined = $state();
	onMount(() => {
		scroller = document.getElementById('main');
	});

	// The count for the question asked, and the genres to choose from; read again when the library changes.
	$effect(() => {
		const a = ask;
		void L.changed;
		let gone = false;
		core.table(a, 0, 1).then(
			(t) => {
				if (gone) return;
				count = t.count;
				waiting = t.waiting;
				failure = null;
			},
			(e) => {
				if (!gone) failure = core.said(e);
			}
		);
		core.genres(a.here ?? false).then(
			(g) => {
				if (!gone) genres = g;
			},
			() => {}
		);
		return () => {
			gone = true;
		};
	});
	// The library changed (tags edited, songs deleted or added): the rows on screen are read again.
	$effect(() => {
		void L.changed;
		untrack(() => list?.refresh());
	});
	// While tags are still being read, look again now and then: the columns fill in without a click.
	$effect(() => {
		if (waiting <= 0) return;
		const timer = setInterval(() => {
			if (document.hidden) return;
			void core.table(untrack(() => ask), 0, 1).then((t) => {
				const done = t.waiting < waiting;
				waiting = t.waiting;
				if (done) list?.refresh();
			});
		}, 4000);
		return () => clearInterval(timer);
	});

	const page = (offset: number, limit: number, signal: AbortSignal) => core.table(ask, offset, limit, signal).then((t) => t.rows);

	/** A table row as the rest of the window knows a song (its menu, the picks, the player). */
	const itemOf = (r: core.TableRow): LibraryItem =>
		itemOfRow({ ...r, origin: r.here ? 'file' : 'described', sort_key: '', isrc: null } as unknown as core.TrackRow);

	const pick: PickList = {
		get scope() {
			return `table:${askKey}`;
		},
		count: () => count ?? 0,
		from: FROM,
		async between(a, b) {
			const out: LibraryItem[] = [];
			for (let at = a; at <= b; at += 500) {
				const want = Math.min(500, b - at + 1);
				const rows = (await core.table(ask, at, want)).rows;
				out.push(...rows.map(itemOf));
				if (rows.length < want) break;
			}
			return out;
		}
	};
	// Another order or question: the songs picked in the old one are let go, and the list starts from its top.
	$effect(() => {
		void askKey;
		untrack(() => {
			P.clear();
			list?.refresh();
		});
	});

	function sortBy(id: core.TableSort) {
		if (sort === id) down = !down;
		else {
			sort = id;
			// Newest and most played first is what those columns are for.
			down = id === 'added' || id === 'plays';
		}
	}
	function rowClick(e: MouseEvent, r: core.TableRow, i: number) {
		const item = itemOf(r);
		if (e.shiftKey) void P.range(pick, i, e.ctrlKey || e.metaKey);
		else if (e.ctrlKey || e.metaKey) P.toggle(pick, i, item);
		else {
			const only = P.count === 1 && P.has(pick, i, item);
			P.clear();
			if (!only) P.toggle(pick, i, item);
		}
	}
	async function play(i: number) {
		if (count === null) return;
		const items = await pick.between(i, Math.min(count - 1, i + 499));
		if (items.length) void player.playItems(items, 0, { from: FROM });
	}
	function toggleColumn(id: ColumnId) {
		shown = shown.includes(id) ? shown.filter((c) => c !== id) : COLUMNS.filter((c) => c.id === id || shown.includes(c.id)).map((c) => c.id);
	}
	const columns = $derived(COLUMNS.filter((c) => c.fixed || shown.includes(c.id)));
	const grid = $derived(gridOf(columns));
	const playingId = $derived(S.v.current?.refs?.lib);
	const date = (ms: number) => (ms > 0 ? new Date(ms).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' }) : '');
</script>

<div class="tb-tools">
	<label class="tb-genre">
		<span class="vh">Genre</span>
		<select bind:value={genre} aria-label="Genre">
			<option value={null}>All genres</option>
			{#each genres as g (g.genre ?? '')}
				<option value={g.genre ?? ''}>{g.genre ?? 'No genre'} ({g.songs.toLocaleString()})</option>
			{/each}
		</select>
	</label>
	<span class="tb-count">
		{#if count !== null}{count.toLocaleString()} {count === 1 ? 'song' : 'songs'}{/if}
		{#if waiting > 0}<i> · reading tags, {waiting.toLocaleString()} to go</i>{/if}
	</span>
	<div class="tb-cols">
		<button type="button" class="chip" aria-haspopup="true" aria-expanded={choosing} onclick={() => (choosing = !choosing)}>Columns</button>
		{#if choosing}
			<div class="tb-pop" role="group" aria-label="Columns shown">
				{#each COLUMNS.filter((c) => !c.fixed) as c (c.id)}
					<label><input type="checkbox" checked={shown.includes(c.id)} onchange={() => toggleColumn(c.id)} /> {c.name || 'Cover'}</label>
				{/each}
			</div>
		{/if}
	</div>
</div>

{#if failure}
	<p class="s-hint" role="alert">{failure}</p>
{:else if count === 0}
	<p class="s-hint">{query.trim() || genre !== null ? 'No song matches.' : here ? 'No music on this computer yet. Add a music folder in Settings › Library.' : 'No songs yet.'}</p>
{:else if count !== null && scroller}
	<div class="tb" role="table" aria-label="Songs" aria-rowcount={count} style:--tb-grid={grid}>
		<div class="tb-h" role="row">
			{#each columns as c (c.id)}
				{#if c.sort}
					<button type="button" class="tb-hc" class:num={c.num} class:on={sort === c.sort} role="columnheader" aria-sort={sort === c.sort ? (down ? 'descending' : 'ascending') : 'none'} title="Sort by {c.name.toLowerCase()}" onclick={() => sortBy(c.sort!)}>
						<span>{c.name}</span>{#if sort === c.sort}<Icon name="arrow_upward" class={down ? 'tb-dn' : ''} />{/if}
					</button>
				{:else}
					<span class="tb-hc" class:num={c.num} role="columnheader">{c.name}</span>
				{/if}
			{/each}
		</div>
		{#snippet songRow(r: core.TableRow | undefined, i: number)}
			{#if r}
				{@const item = itemOf(r)}
				{@const picked = P.has(pick, i, item)}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<div class="tb-r" class:pk={picked} class:cur={playingId === String(r.id)} class:away={!r.here} role="row" tabindex="-1" aria-selected={picked} use:songMenu={{ item }} onclick={(e) => rowClick(e, r, i)} ondblclick={() => void play(i)}>
					{#each columns as c (c.id)}
						<span class="tb-c" class:num={c.num} class:strong={c.id === 'title'} role="cell" title={c.id === 'title' ? r.title : undefined}>
							{#if c.id === 'cover'}{#if core.artUrl(r.art)}<img class="tb-cv" src={core.artUrl(r.art)} alt="" loading="lazy" decoding="async" crossorigin="anonymous" />{/if}
							{:else if c.id === 'title'}{r.title}
							{:else if c.id === 'artist'}{r.artist}
							{:else if c.id === 'album'}{r.album ?? ''}
							{:else if c.id === 'genre'}{r.genre ?? ''}
							{:else if c.id === 'year'}{r.year ?? ''}
							{:else if c.id === 'track'}{r.track_no || ''}
							{:else if c.id === 'length'}{r.duration_ms ? mmss(r.duration_ms) : ''}
							{:else if c.id === 'format'}{formatOf(r)}
							{:else if c.id === 'added'}{date(r.added_at_ms)}
							{:else if c.id === 'plays'}{r.plays || ''}
							{/if}
						</span>
					{/each}
				</div>
			{:else}
				<div class="tb-r tb-ph" role="row"><span class="tb-c"><i></i></span></div>
			{/if}
		{/snippet}
		<VirtualList bind:this={list} {count} rowHeight={ROW_HEIGHT} fetchPage={page} row={songRow} {scroller} />
	</div>
{/if}

<style>
	.tb-tools { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin: 4px 0 8px; }
	.tb-genre select { height: 34px; max-width: 260px; padding: 0 10px; border-radius: 10px; border: 1px solid var(--wp-line-2); background: var(--wp-field, transparent); color: var(--wp-text); font: 400 14px/1 var(--font-body); }
	.tb-count { color: var(--wp-text-2); font: 400 13px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	.tb-count i { font-style: normal; }
	.tb-cols { position: relative; margin-left: auto; }
	.tb-pop { position: absolute; z-index: 30; right: 0; top: calc(100% + 6px); display: grid; gap: 2px; min-width: 170px; padding: 8px; border-radius: 12px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 12px 32px rgb(0 0 0 / .18); }
	.tb-pop label { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 8px; font: 400 14px/1.2 var(--font-body); cursor: pointer; }
	.tb-pop input { accent-color: var(--wp-accent); }
	.vh { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
	.tb { margin: 0 -8px; font: 400 13.5px/1 var(--font-body); }
	.tb-h, .tb-r { display: grid; grid-template-columns: var(--tb-grid); align-items: center; column-gap: 12px; padding: 0 10px; }
	.tb-h { position: sticky; top: 0; z-index: 5; height: 34px; background: var(--wp-main); border-bottom: 1px solid var(--wp-line-2); }
	.tb-hc { display: inline-flex; align-items: center; gap: 4px; min-width: 0; height: 100%; padding: 0; color: var(--wp-text-2); font: 600 12px/1 var(--font-body); letter-spacing: .02em; text-align: left; white-space: nowrap; }
	.tb-hc span { overflow: hidden; text-overflow: ellipsis; }
	.tb-hc.num { justify-content: flex-end; }
	.tb-hc.on { color: var(--wp-text); }
	.tb-hc :global(svg) { width: 14px; height: 14px; flex: none; transition: rotate 120ms; }
	.tb-hc :global(.tb-dn) { rotate: 180deg; }
	.tb-r { height: 36px; border-radius: 6px; cursor: default; user-select: none; }
	.tb-c { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--wp-text-2); }
	.tb-cv { display: block; width: 28px; height: 28px; border-radius: 4px; object-fit: cover; background: var(--wp-hover); }
	.tb-c.strong { color: var(--wp-text); font-weight: 500; }
	.tb-c.num { text-align: right; font-variant-numeric: tabular-nums; }
	.tb-r.away .tb-c { opacity: .55; }
	.tb-r.cur .tb-c.strong { color: var(--wp-accent); }
	.tb-r.pk, .tb-r.pk:hover { background: color-mix(in oklab, var(--np-vib, var(--wp-accent)) 30%, transparent); }
	.tb-ph i { display: block; width: min(240px, 60%); height: 10px; border-radius: 5px; background: var(--wp-hover); }
	@media (hover: hover) and (pointer: fine) { .tb-r:hover { background: var(--wp-hover); } button.tb-hc:hover { color: var(--wp-text); } .tb-pop label:hover { background: var(--wp-hover); } }
	button.tb-hc:focus-visible { outline: 2px solid var(--wp-accent); outline-offset: -2px; border-radius: 4px; }
</style>
