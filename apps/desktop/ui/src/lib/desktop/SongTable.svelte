<!-- Library › Songs as a table, the way a library manager shows a collection: a row a song, a column a tag, any column's
     heading sorts by it (again turns it round), a genre can be chosen, and the columns shown are the listener's choice.
     The rows come a page at a time from the core in the order asked (`/api/table`), so it is as quick with 100,000 songs
     as with a hundred. A click picks a row (Ctrl and Shift pick more), a double click plays from it. -->
<script lang="ts">
	import { onMount, tick, untrack } from 'svelte';
	import type { LibraryItem } from '../api';
	import { itemOfRow, player } from '../app';
	import * as core from '../core';
	import VirtualList from '../VirtualList.svelte';
	import Icon from '../ui/Icon.svelte';
	import { mmss } from '../ui/format';
	import { P, rowLists, type PickList } from '../ui/pick.svelte';
	import { songMenu } from '../ui/song-menu';
	import { L, S, ui, type Choice } from '../ui/state.svelte';
	import { under } from './playlist-folders';
	import { folders } from './folders.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { decadeName } from './browse';
	import { COLUMNS, formatOf, gridOf, readColumns, type ColumnId } from './song-table';

	let { query = '', here = false }: { query?: string; here?: boolean } = $props();

	const ROW_HEIGHT = 36;
	const FROM = 'Your library';

	let sort = $state<core.TableSort>((localStorage.getItem('stash.table.sort') as core.TableSort) || 'artist');
	let down = $state(localStorage.getItem('stash.table.down') === '1');
	// The genre, decade and folder shown are in the address, so Library > Genres and Folders open the table on theirs,
	// and Back returns to what was shown.
	const genre = $derived(page.url.searchParams.get('genre'));
	const decade = $derived.by(() => {
		const d = page.url.searchParams.get('decade');
		return d !== null && /^\d{4}$/.test(d) ? Number(d) : null;
	});
	const folder = $derived(page.url.searchParams.get('folder'));
	function show(name: 'genre' | 'decade' | 'folder', value: string | null) {
		const url = new URL(page.url.href);
		if (value === null) url.searchParams.delete(name);
		else url.searchParams.set(name, value);
		url.searchParams.set('tab', 'songs');
		void goto(`${url.pathname}${url.search}`, { replaceState: true });
	}
	let decades = $state.raw<core.DecadeRow[]>([]);
	let shown = $state<ColumnId[]>(readColumns(localStorage.getItem('stash.table.cols.2')));
	let choosing = $state(false);
	$effect(() => {
		localStorage.setItem('stash.table.sort', sort);
		localStorage.setItem('stash.table.down', down ? '1' : '0');
		localStorage.setItem('stash.table.cols.2', shown.join(','));
	});

	const ask = $derived<core.TableAsk>({ sort, down, q: query.trim() || undefined, genre: genre ?? undefined, here, decade: decade ?? undefined, folder: folder ?? undefined });
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
		core.decades(a.here ?? false).then(
			(d) => {
				if (!gone) decades = d;
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

	const readPage = (offset: number, limit: number, signal: AbortSignal) => core.table(ask, offset, limit, signal).then((t) => t.rows);

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
			at = null;
			moved++;
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
		moved++;
		at = i;
		table?.focus({ preventScroll: true });
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
	/** The genre and the decade shown are chosen in the window's own menu, as every other choice is. */
	function genreMenu(el: HTMLElement) {
		const choices: Choice[] = [
			{ icon: 'library_music', label: 'All genres', checked: genre === null, run: () => show('genre', null) },
			...genres.map((g): Choice => ({ icon: 'sell', label: `${g.genre ?? 'No genre'} (${g.songs.toLocaleString()})`, checked: genre === (g.genre ?? ''), run: () => show('genre', g.genre ?? '') }))
		];
		ui.openChoices({ ...under(el), label: 'Show one genre', title: 'Genre', choices });
	}
	function decadeMenu(el: HTMLElement) {
		const choices: Choice[] = [
			{ icon: 'library_music', label: 'All years', checked: decade === null, run: () => show('decade', null) },
			...decades
				.filter((d) => d.decade !== null)
				.map((d): Choice => ({ icon: 'history', label: `${decadeName(d.decade)} (${d.songs.toLocaleString()})`, checked: decade === d.decade, run: () => show('decade', String(d.decade)) }))
		];
		ui.openChoices({ ...under(el), label: 'Show one decade', title: 'Years', choices });
	}

	// ---- The keyboard: the arrow keys move through the songs (Shift picks the ones on the way), Page Up and Down a
	// screen, Home and End to the ends, Enter plays from the song, the menu key (or Shift+F10) opens its menu.
	/** The song the keyboard is on, by its place; null until a key or a click puts it somewhere. */
	let at = $state<number | null>(null);
	let table = $state<HTMLElement>();
	/**
	 * What the keys pick is applied in the order they came, each after the one before (a song may have to be read from
	 * the core first): held down, Shift+Down still picks from where the plain Down keys left off. A click or another list
	 * (`moved`) drops the picks still waiting.
	 */
	let picks: Promise<void> = Promise.resolve();
	let moved = 0;
	// Ctrl+A with the table focused picks all of it, as it does on a row of any other list.
	$effect(() => {
		if (table) rowLists.set(table, pick);
	});
	/** Brings the song at `i` on screen (below the sticky headings), drawing it first when it is far away. */
	async function reveal(i: number) {
		await tick();
		let el = document.getElementById(`tb-r-${i}`);
		if (!el) {
			list?.scrollToIndex(Math.max(0, i - 3));
			await new Promise((r) => requestAnimationFrame(() => r(null)));
			await tick();
			el = document.getElementById(`tb-r-${i}`);
		}
		el?.scrollIntoView({ block: 'nearest' });
	}
	/** The menu key or Shift+F10 on the table (the browser sends those as a right-click on what has the focus): the menu of
	 * the song the keyboard is on. A right-click on a row opens that row's own. */
	async function tableMenu(e: MouseEvent) {
		const keyboard = (e.clientX === 0 && e.clientY === 0) || e.button !== 2;
		if (e.target !== table || at === null || !keyboard) return;
		e.preventDefault();
		const i = at;
		await reveal(i);
		document.getElementById(`tb-r-${i}`)?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true }));
	}
	async function tableKey(e: KeyboardEvent) {
		// Only on the table itself: Enter on a column heading sorts by it, as it always did.
		if (e.target !== table || !count || e.altKey || e.metaKey) return;
		if (e.key === 'Enter') {
			if (at === null) return;
			e.preventDefault();
			void play(at);
			return;
		}
		if (e.ctrlKey) return;
		const screen = Math.max(1, Math.floor((scroller?.clientHeight ?? 400) / ROW_HEIGHT) - 2);
		const from = at ?? -1;
		const last = count - 1;
		const to =
			e.key === 'ArrowDown' ? Math.min(last, from + 1)
			: e.key === 'ArrowUp' ? Math.max(0, from - 1)
			: e.key === 'PageDown' ? Math.min(last, Math.max(0, from) + screen)
			: e.key === 'PageUp' ? Math.max(0, from - screen)
			: e.key === 'Home' ? 0
			: e.key === 'End' ? last
			: null;
		if (to === null) return;
		e.preventDefault();
		at = to;
		const add = e.shiftKey;
		const when = moved;
		picks = picks
			.then(async () => {
				if (when !== moved) return;
				if (add) return P.range(pick, to, false);
				const [item] = await pick.between(to, to);
				if (item && when === moved) {
					P.clear();
					P.toggle(pick, to, item);
				}
			})
			.catch(() => {});
		await reveal(to);
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
	<button type="button" class="chip tb-pick" class:on={genre !== null} aria-haspopup="menu" title="Show one genre" onclick={(e) => genreMenu(e.currentTarget)}><span>{genre === null ? 'All genres' : genre || 'No genre'}</span><Icon name="expand_more" /></button>
	{#if decades.some((d) => d.decade !== null)}
		<button type="button" class="chip tb-pick" class:on={decade !== null} aria-haspopup="menu" title="Show one decade" onclick={(e) => decadeMenu(e.currentTarget)}><span>{decade === null ? 'All years' : decadeName(decade)}</span><Icon name="expand_more" /></button>
	{/if}
	{#if folder}
		<button type="button" class="chip on tb-in" title="{folder}: click to show every folder" onclick={() => show('folder', null)}><Icon name="folder" /><span>{folder.split(/[\\/]+/).pop()}</span><Icon name="close" /></button>
	{/if}
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
	{#if query.trim() || genre !== null || decade !== null || folder}
		<p class="s-hint">No song matches.</p>
	{:else}
		<p class="s-hint">{here ? 'No music on this computer yet.' : 'No songs yet.'} Add the folder your music is in, and its songs show up here.</p>
		<button type="button" class="btn ghost" onclick={() => void folders.add()}><Icon name="create_new_folder" />Add a music folder…</button>
	{/if}
{:else if count !== null && scroller}
	<div class="tb" role="grid" aria-label="Songs" aria-rowcount={count} style:--tb-grid={grid} tabindex="0" data-picks="" aria-activedescendant={at !== null ? `tb-r-${at}` : undefined} bind:this={table} onkeydown={tableKey} oncontextmenu={tableMenu}>
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
				<!-- The table holds the focus and names the song the keyboard is on (aria-activedescendant), so a row takes none. -->
				<!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
				<div class="tb-r" id="tb-r-{i}" class:pk={picked} class:kb={at === i} class:cur={playingId === String(r.id)} class:away={!r.here} role="row" aria-rowindex={i + 1} aria-selected={picked} use:songMenu={{ item }} onclick={(e) => rowClick(e, r, i)} ondblclick={() => void play(i)}>
					{#each columns as c (c.id)}
						<span class="tb-c" class:num={c.num} class:strong={c.id === 'title'} role="gridcell" title={c.id === 'title' ? r.title : undefined}>
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
		<VirtualList bind:this={list} {count} rowHeight={ROW_HEIGHT} fetchPage={readPage} row={songRow} {scroller} />
	</div>
{/if}

<style>
	.tb-tools { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin: 4px 0 8px; }
	.tb-pick { max-width: 280px; }
	.tb-pick span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.tb-pick.chip :global(.ic) { margin: 0 -6px 0 -2px; }
	.tb-in { display: inline-flex; align-items: center; gap: 6px; max-width: 260px; }
	.tb-in span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.tb-in :global(svg) { width: 16px; height: 16px; flex: none; }
	.tb-count { color: var(--wp-text-2); font: 400 13px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	.tb-count i { font-style: normal; }
	.tb-cols { position: relative; margin-left: auto; }
	.tb-pop { position: absolute; z-index: 30; right: 0; top: calc(100% + 6px); display: grid; gap: 2px; min-width: 170px; padding: 8px; border-radius: 12px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 12px 32px rgb(0 0 0 / .18); }
	.tb-pop label { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 8px; font: 400 14px/1.2 var(--font-body); cursor: pointer; }
	.tb-pop input { accent-color: var(--acc); }
	.tb { margin: 0 -8px; font: 400 13.5px/1 var(--font-body); }
	.tb-h, .tb-r { display: grid; grid-template-columns: var(--tb-grid); align-items: center; column-gap: 12px; padding: 0 10px; }
	.tb-h { position: sticky; top: 0; z-index: 5; height: 34px; background: var(--wp-main); border-bottom: 1px solid var(--wp-line-2); }
	.tb-hc { display: inline-flex; align-items: center; gap: 4px; min-width: 0; height: 100%; padding: 0; color: var(--wp-text-2); font: 600 12px/1 var(--font-body); letter-spacing: .02em; text-align: left; white-space: nowrap; }
	.tb-hc span { overflow: hidden; text-overflow: ellipsis; }
	.tb-hc.num { justify-content: flex-end; }
	.tb-hc.on { color: var(--wp-text); }
	.tb-hc :global(svg) { width: 14px; height: 14px; flex: none; transition: rotate 120ms; }
	.tb-hc :global(.tb-dn) { rotate: 180deg; }
	/* Moved to with the keys: below the sticky headings, above the player bar and the bar of picked songs. */
	.tb-r { height: 36px; border-radius: 6px; cursor: default; user-select: none; scroll-margin: 40px 0 calc(var(--bar-clear, 0px) + var(--pick-clear, 0px) + 12px); }
	.tb:focus { outline: none; }
	.tb:focus .tb-r.kb { box-shadow: inset 0 0 0 2px var(--acc); }
	.tb-c { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--wp-text-2); }
	.tb-cv { display: block; width: 28px; height: 28px; border-radius: 4px; object-fit: cover; background: var(--wp-hover); }
	.tb-c.strong { color: var(--wp-text); font-weight: 500; }
	.tb-c.num { text-align: right; font-variant-numeric: tabular-nums; }
	.tb-r.away .tb-c { opacity: .55; }
	.tb-r.cur .tb-c.strong { color: var(--acc-text); }
	.tb-r.pk, .tb-r.pk:hover { background: color-mix(in oklab, var(--np-vib) 30%, transparent); }
	.tb-ph i { display: block; width: min(240px, 60%); height: 10px; border-radius: 5px; background: var(--wp-hover); }
	@media (hover: hover) and (pointer: fine) { .tb-r:hover { background: var(--wp-hover); } button.tb-hc:hover { color: var(--wp-text); } .tb-pop label:hover { background: var(--wp-hover); } }
	button.tb-hc:focus-visible { outline: 2px solid var(--acc); outline-offset: -2px; border-radius: 4px; }
</style>
