<!-- The songs a phone sent (Send to computer, over USB), to choose which come in (the core's `phone_transfer.rs`).
     Nothing is read from the phone until they are chosen, and nothing is chosen to begin with: songs are ticked one by
     one, or all those the filters show at once; Take all takes every new one. The filters only choose what is shown, never
     what is chosen. The bar with Take stays on screen above the player bar. Once they are in, the phone is told the
     computer is done and clears its copies of the rest: its own songs are not touched. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount, tick } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import * as core from '../core';
	import Cover from '../ui/Cover.svelte';
	import Icon from '../ui/Icon.svelte';
	import { songs as songsWord } from '../ui/format';
	import PhoneFacet from './PhoneFacet.svelte';
	import { FACETS, bytesOf, detailOf, facet, narrowed as isNarrowed, noFilters, shows, size, toTake, type FacetId, type Filters, type Offer, type Offered } from './phone-songs';

	type Now = { state: string; phone: string | null; done: number; total: number; taken: number; had: number; not_kept: number; failed: number; left: number };

	let offer = $state.raw<Offer | null>(null);
	let now = $state.raw<Now | null>(null);
	let failure = $state<string | null>(null);
	let looked = $state(false);
	let sending = $state(false);
	let filters = $state<Filters>(noFilters());
	/** The list shows the songs not here yet (and those here, greyed, when asked), filtered; or every song chosen. */
	let show = $state<'new' | 'chosen'>('new');
	/** The songs chosen when the chosen ones were asked for: one unticked there stays in the list, to tick again. */
	let review = $state.raw<ReadonlySet<string>>(new Set());
	/** The filter whose panel is open under the row. */
	let open = $state<FacetId | null>(null);
	/** The songs chosen, by their file on the phone. */
	const chosen = new SvelteSet<string>();

	async function readOffer() {
		try {
			const o = await core.get<Offer>('/api/phone/songs');
			if (o.sent_at_ms !== offer?.sent_at_ms) {
				chosen.clear();
				filters = noFilters();
				show = 'new';
				open = null;
			}
			offer = o;
			failure = null;
		} catch (e) {
			if (e instanceof core.Refused && e.status === 404) offer = null;
			else failure = core.said(e);
		}
	}

	// What the phone is doing, every two seconds; the songs again when the send waiting changes.
	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const ask = async () => {
			try {
				const n = await core.get<Now>('/api/phone');
				if (!alive) return;
				now = n;
				if (n.state === 'ready' && (!offer || offer.songs.length !== n.total)) await readOffer();
				else if (n.state !== 'ready' && !sending) offer = null;
			} catch {
				/* the core is not answering: the page stays as it was */
			}
			looked = true;
			if (alive) timer = setTimeout(ask, 2000);
		};
		void ask();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	const all = $derived(offer?.songs ?? []);
	const fresh = $derived(all.filter((s) => !s.here));
	const freshBytes = $derived(bytesOf(fresh));
	const here = $derived(all.length - fresh.length);
	const taking = $derived(toTake(all, chosen));
	const takingBytes = $derived(bytesOf(taking));
	/** What each filter can choose from: the songs not here yet. */
	const choices = $derived(Object.fromEntries(FACETS.map((f) => [f.id, facet(fresh, f.id)])) as Record<FacetId, ReturnType<typeof facet>>);
	/** A filter with too few choices to tell songs apart is left out of the row. */
	const row = $derived(FACETS.filter((f) => choices[f.id].length >= f.least));
	/** The filters narrow the new songs; the chosen ones are shown all, so what is taken is plain. */
	const narrowed = $derived(show === 'new' && isNarrowed(filters));
	const base = $derived(show === 'chosen' ? all.filter((s) => review.has(s.file)) : filters.showHere ? all : fresh);
	const shown = $derived(show === 'chosen' ? base : base.filter((s) => shows(s, filters)));
	const choosable = $derived(shown.filter((s) => !s.here));
	const allOn = $derived(choosable.length > 0 && choosable.every((s) => chosen.has(s.file)));
	const pills = $derived(FACETS.flatMap((f) => (filters.picked[f.id] ?? []).map((value) => ({ id: f.id, label: f.label, value }))));
	const openFacet = $derived(FACETS.find((f) => f.id === open) ?? null);

	function tickOne(file: string) {
		if (chosen.has(file)) chosen.delete(file);
		else chosen.add(file);
	}

	function chooseShown(on: boolean) {
		for (const s of choosable) {
			if (on) chosen.add(s.file);
			else chosen.delete(s.file);
		}
	}

	function setPicked(id: FacetId, values: string[]) {
		const picked = { ...filters.picked };
		if (values.length) picked[id] = values;
		else delete picked[id];
		filters = { ...filters, picked };
	}

	async function closePanel() {
		const was = open;
		open = null;
		await tick();
		document.querySelector<HTMLElement>(`[data-facet="${was}"]`)?.focus({ preventScroll: true });
	}

	function seeChosen() {
		review = new Set(chosen);
		show = 'chosen';
		open = null;
		document.getElementById('main')?.scrollTo({ top: 0 });
	}

	/** Takes `songs`; none (every song here already) lets the phone clear its copies. */
	async function take(songs: Offered[]) {
		if (!offer) return;
		sending = true;
		failure = null;
		try {
			await core.post('/api/phone/take', songs.length ? { sent_at_ms: offer.sent_at_ms, files: songs.map((s) => s.file) } : { sent_at_ms: offer.sent_at_ms, files: [], none: true });
			offer = null;
		} catch (e) {
			failure = core.said(e);
			await readOffer();
		} finally {
			sending = false;
		}
	}

	const phone = $derived(offer?.phone ?? now?.phone ?? 'your phone');
	const busy = $derived(now?.state === 'taking' || now?.state === 'stalled');
	const takeAllLabel = $derived(`Take all ${fresh.length.toLocaleString()} new · ${size(freshBytes)}`);
	const baseCount = $derived(base.length);
	const shownLine = $derived(
		`${narrowed ? `${shown.length.toLocaleString()} of ${songsWord(baseCount)} shown` : songsWord(shown.length)} · ${size(bytesOf(choosable))}`
	);
</script>

<div class="view view-in ps">
	<div class="pg-head">
		<h1 class="pg-title">Songs from {phone}</h1>
		{#if offer && fresh.length}
			<button type="button" class="btn" disabled={sending} onclick={() => void take(fresh)}>{takeAllLabel}</button>
		{/if}
	</div>
	{#if offer}
		<p class="pg-sub ps-sub">
			<span>
				{songsWord(all.length)} sent{here ? ` · ${here.toLocaleString()} on this computer already${filters.showHere ? ', shown greyed' : ', hidden'}` : ''} · nothing comes in until you take it.
			</span>
			{#if here}
				<button type="button" class="ps-link" onclick={() => (filters = { ...filters, showHere: !filters.showHere })}>{filters.showHere ? 'Hide them' : 'Show them'}</button>
			{/if}
		</p>

		{#if show === 'new'}
			<div class="ps-tools" role="group" aria-label="Filters">
				<label class="ps-find"><Icon name="search" /><input type="search" placeholder="Find a song, artist or album" aria-label="Find a song, artist or album" bind:value={filters.text} maxlength="200" /></label>
				{#each row as f (f.id)}
					{@const n = filters.picked[f.id]?.length ?? 0}
					<button type="button" class="chip ps-chip" class:on={n > 0} class:open={open === f.id} data-facet={f.id} aria-expanded={open === f.id} onclick={() => (open = open === f.id ? null : f.id)}>
						{f.label}{n ? ` · ${n}` : ''}<Icon name="expand_more" />
					</button>
				{/each}
			</div>

			{#if openFacet}
				{#key openFacet.id}
					<PhoneFacet
						label={openFacet.label}
						many={openFacet.many}
						find={openFacet.find}
						facets={choices[openFacet.id]}
						bind:picked={() => filters.picked[openFacet.id] ?? [], (v) => setPicked(openFacet.id, v)}
						onclose={() => void closePanel()}
					/>
				{/key}
			{/if}

			{#if pills.length}
				<div class="ps-pills" role="group" aria-label="Filters on">
					{#each pills as p (`${p.id}:${p.value}`)}
						<span class="ps-pill"><i>{p.label}</i>{p.value}<button type="button" aria-label="Remove {p.value}" onclick={() => setPicked(p.id, (filters.picked[p.id] ?? []).filter((v) => v !== p.value))}><Icon name="close" /></button></span>
					{/each}
					<button type="button" class="ps-link" onclick={() => (filters = { ...noFilters(), showHere: filters.showHere })}>Clear filters</button>
				</div>
			{/if}
		{/if}

		<div class="ps-head">
			<div class="ps-head-l">
				<div class="ps-seg" role="group" aria-label="Show">
					<button type="button" class:on={show === 'new'} aria-pressed={show === 'new'} onclick={() => (show = 'new')}>New songs {fresh.length.toLocaleString()}</button>
					<button type="button" class:on={show === 'chosen'} aria-pressed={show === 'chosen'} onclick={seeChosen}>Chosen {taking.length.toLocaleString()}</button>
				</div>
				<span class="ps-n">{shownLine}</span>
			</div>
			{#if choosable.length}
				<button type="button" class="btn ghost" onclick={() => chooseShown(!allOn)}>
					{allOn ? 'Unchoose' : 'Choose'}
					{choosable.length === 1 ? 'it' : `${narrowed ? 'these' : 'all'} ${choosable.length.toLocaleString()}`}
				</button>
			{/if}
		</div>

		<div class="ps-list" role="group" aria-label="Songs">
			{#each shown as s (s.file)}
				{@const on = !s.here && chosen.has(s.file)}
				<label class="ps-r" class:ps-on={on} class:ps-here={s.here}>
					<input type="checkbox" checked={on} disabled={s.here} onchange={() => tickOne(s.file)} />
					<Cover artwork={s.art_url ? [{ url: core.artUrl(s.art_url) ?? '' }] : undefined} seed={s.album ?? s.title} class="ps-art" />
					<span class="ps-song">
						<b>{s.title}{#if s.here}<i class="ps-tag">On this computer</i>{/if}</b>
						<span>{[s.artist, s.album ?? ''].filter(Boolean).join(' · ')}</span>
					</span>
					<span class="ps-fmt">{detailOf(s)}</span>
					<span class="ps-size">{size(s.bytes)}</span>
				</label>
			{:else}
				<p class="s-hint">
					{#if show === 'chosen' && !taking.length}
						Nothing chosen yet. Tick songs, or filter and choose them all.
					{:else}
						No songs match. <button type="button" class="ps-link" onclick={() => (filters = { ...noFilters(), showHere: filters.showHere })}>Clear filters</button>
					{/if}
				</p>
			{/each}
		</div>

		<!-- Sticks above the player bar (`--bar-clear`), like the bar for songs picked in a list. -->
		<div class="ps-take">
			<div class="ps-take-in" role="region" aria-label="Songs to take">
				{#if failure}<span class="ps-bad" role="alert">{failure}</span>{/if}
				{#if taking.length}
					<span class="ps-take-n" aria-live="polite">{taking.length.toLocaleString()} chosen · {size(takingBytes)}</span>
					<button type="button" class="ps-tb" onclick={seeChosen}>See them</button>
					<button type="button" class="ps-tb go" disabled={sending} onclick={() => void take(taking)}>{sending ? 'Starting…' : `Take ${taking.length.toLocaleString()}`}</button>
					<button type="button" class="ps-tb x" aria-label="Unchoose them all" title="Unchoose them all" onclick={() => chosen.clear()}><Icon name="close" /></button>
				{:else if fresh.length}
					<span class="ps-take-q">Tick songs, or filter and choose them all, or</span>
					<button type="button" class="ps-tb go" disabled={sending} onclick={() => void take(fresh)}>{sending ? 'Starting…' : takeAllLabel}</button>
				{:else}
					<span class="ps-take-q">Every song {phone} sent is on this computer already.</span>
					<button type="button" class="ps-tb go" disabled={sending} onclick={() => void take([])}>{sending ? 'Starting…' : 'Done'}</button>
				{/if}
			</div>
		</div>
	{:else if busy && now}
		<p class="s-hint" role="status">Taking {songsWord(now.total)} from {phone}: {now.done.toLocaleString()} done. Keep it plugged in.</p>
		<button type="button" class="btn ghost" onclick={() => void goto('/library')}>Go to your library</button>
	{:else if now?.state === 'done'}
		<p class="s-hint" role="status">
			{songsWord(now.taken)} came in{now.had ? `, ${now.had.toLocaleString()} you had already` : ''}{now.failed ? `. ${songsWord(now.failed)} didn't: send again from the phone to try them` : ''}.
			{now.left === 1 ? `The other song stays in Stash on ${phone}.` : now.left ? `The other ${now.left.toLocaleString()} stay in Stash on ${phone}.` : ''}
		</p>
		<button type="button" class="btn ghost" onclick={() => void goto('/library')}>Go to your library</button>
	{:else if !looked}
		<p class="s-hint">Looking at your phone…</p>
	{:else}
		<p class="s-hint">
			No songs are waiting. Plug your phone in and choose File transfer, then in Stash on the phone: Settings › Library &amp; Storage › Send to computer. Its songs show up here
			to choose from.
		</p>
	{/if}
</div>

<style>
	.ps-sub { display: flex; align-items: center; flex-wrap: wrap; gap: 4px; margin-top: 0; }
	.ps-link { height: 28px; padding: 0 8px; border-radius: 8px; color: var(--acc-text); font: 600 13px/1 var(--font-body); }
	.ps-link:hover { background: var(--wp-hover); }
	.ps-tools { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 0 0 12px; }
	.ps-find { display: flex; align-items: center; gap: 8px; width: 280px; max-width: 100%; height: 36px; padding: 0 12px; border-radius: 12px; background: var(--wp-hover); color: var(--wp-text-2); }
	.ps-find :global(svg) { width: 18px; height: 18px; fill: currentColor; flex: none; }
	.ps-find input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--wp-text); font: 400 13.5px/1 var(--font-body); }
	.ps-find:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
	.ps-chip { padding-right: 10px; }
	.ps-chip :global(svg) { width: 16px; height: 16px; fill: currentColor; transition: rotate var(--dur-short); }
	.ps-chip.open { border-color: var(--acc); color: var(--wp-text); }
	.ps-chip.open :global(svg) { rotate: 180deg; }
	.ps-pills { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin: 0 0 12px; }
	.ps-pill { display: inline-flex; align-items: center; gap: 6px; height: 28px; padding: 0 4px 0 12px; border-radius: 999px; background: var(--acc-wash); color: var(--wp-text); font: 600 12.5px/1 var(--font-body); }
	.ps-pill i { font-style: normal; font-weight: 500; color: var(--wp-text-2); }
	.ps-pill button { display: grid; place-items: center; width: 22px; height: 22px; border-radius: 50%; color: var(--wp-text-2); }
	.ps-pill button:hover { background: var(--wp-hover); color: var(--wp-text); }
	.ps-pill :global(svg) { width: 14px; height: 14px; fill: currentColor; }
	.ps-head { position: sticky; top: 0; z-index: 4; display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 12px; margin: 0 -8px 4px; padding: 10px 8px; background: var(--wp-main); border-bottom: 1px solid var(--wp-line-2); }
	.ps-head-l { display: flex; align-items: center; flex-wrap: wrap; gap: 14px; }
	.ps-seg { display: inline-flex; padding: 3px; border-radius: 12px; background: var(--wp-hover); }
	.ps-seg button { height: 30px; padding: 0 12px; border-radius: 9px; color: var(--wp-text-2); font: 600 13px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	.ps-seg button.on { background: var(--wp-field); color: var(--wp-text); box-shadow: 0 1px 2px rgb(0 0 0 / .4); }
	.ps-n { color: var(--wp-text-2); font: 400 13px/1.3 var(--font-body); font-variant-numeric: tabular-nums; }
	.ps-r { display: grid; grid-template-columns: 20px 36px minmax(0, 1fr) 128px 64px; align-items: center; gap: 12px; min-height: 48px; padding: 4px 8px; border-radius: 8px; font: 400 13.5px/1.35 var(--font-body); cursor: pointer; content-visibility: auto; contain-intrinsic-size: auto 48px; }
	@media (hover: hover) and (pointer: fine) { .ps-r:hover { background: var(--wp-hover); } .ps-on:hover { background: color-mix(in oklab, var(--acc) 22%, transparent); } }
	.ps-r input { width: 16px; height: 16px; margin: 0; accent-color: var(--acc); cursor: pointer; }
	.ps-on { background: var(--acc-wash); }
	.ps-here { cursor: default; }
	.ps-here > :not(input) { opacity: 0.5; }
	.ps-r :global(.ps-art) { display: block; width: 36px; height: 36px; border-radius: 6px; object-fit: cover; }
	.ps-song { display: grid; min-width: 0; }
	.ps-song b { font-weight: 500; color: var(--wp-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ps-song span, .ps-fmt, .ps-size { color: var(--wp-text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ps-song span { font-size: 12.5px; }
	.ps-tag { margin-left: 8px; padding: 1px 7px; border-radius: 999px; background: var(--wp-hover); color: var(--wp-text-2); font: 500 11.5px/1.4 var(--font-body); font-style: normal; vertical-align: 1px; }
	.ps-size { text-align: right; font-variant-numeric: tabular-nums; }
	.ps-take { position: sticky; z-index: 6; bottom: calc(var(--bar-clear, 0px) + 16px); display: flex; justify-content: center; margin-top: 16px; pointer-events: none; }
	.ps-take-in { pointer-events: auto; display: flex; align-items: center; gap: 4px; max-width: 100%; padding: 4px 4px 4px 18px; border-radius: 999px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); font: 500 13px/1 var(--font-body); color: var(--wp-text); animation: ps-in 180ms var(--ease-out-expo) both; }
	@keyframes ps-in { from { opacity: 0; translate: 0 6px; } }
	@media (prefers-reduced-motion: reduce) { .ps-take-in { animation: none; } }
	.ps-take-n { margin-right: 8px; font-weight: 600; font-variant-numeric: tabular-nums; white-space: nowrap; }
	.ps-take-q { margin-right: 8px; color: var(--wp-text-2); white-space: nowrap; }
	.ps-bad { margin-right: 8px; color: var(--red); }
	.ps-tb { display: inline-flex; align-items: center; gap: 6px; height: 36px; padding: 0 14px; border-radius: 999px; font-weight: 600; white-space: nowrap; transition: background-color var(--dur-short); }
	.ps-tb:hover { background: var(--wp-hover); }
	.ps-tb.go { background: var(--action); color: var(--on-action); }
	.ps-tb.go:hover { background: var(--action-hover); }
	.ps-tb.x { width: 36px; padding: 0; justify-content: center; color: var(--wp-text-2); }
	.ps-tb.x :global(svg) { width: 20px; height: 20px; fill: currentColor; }
</style>
