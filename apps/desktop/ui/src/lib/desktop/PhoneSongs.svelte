<!-- The songs a phone sent (Send to computer, over USB), to choose which come in (the core's `phone_transfer.rs`).
     Nothing is read from the phone until they are chosen. Filters above the list choose what is shown (songs on this
     computer already are hidden to begin with); what is shown and ticked is taken. Once they are in, the phone is told
     the computer is done (its list is deleted) and clears its copies of the rest: its own songs are not touched. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import * as core from '../core';
	import Cover from '../ui/Cover.svelte';
	import Icon from '../ui/Icon.svelte';
	import { songs as songsWord } from '../ui/format';
	import PhoneFacet from './PhoneFacet.svelte';
	import { NO_FILTERS, artistOf, bytesOf, detailOf, facets as facetsOf, shows, size, type Filters, type Offer, type Quality } from './phone-songs';

	type Now = { state: string; phone: string | null; done: number; total: number; taken: number; had: number; not_kept: number; failed: number; left: number };

	let offer = $state.raw<Offer | null>(null);
	let now = $state.raw<Now | null>(null);
	let failure = $state<string | null>(null);
	let looked = $state(false);
	let sending = $state(false);
	let filters = $state<Filters>({ ...NO_FILTERS });
	/** The songs whose tick was taken off, by their file on the phone. */
	const unticked = new SvelteSet<string>();

	async function readOffer() {
		try {
			const o = await core.get<Offer>('/api/phone/songs');
			if (o.sent_at_ms !== offer?.sent_at_ms) {
				unticked.clear();
				filters = { ...NO_FILTERS };
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
	const here = $derived(all.filter((s) => s.here).length);
	/** What the filters choose from: the songs not here yet, unless those here are shown too. */
	const base = $derived(filters.newOnly ? all.filter((s) => !s.here) : all);
	const f = $derived(facetsOf(base));
	const shown = $derived(all.filter((s) => shows(s, filters)));
	const picked = $derived(shown.filter((s) => !unticked.has(s.file)));
	const pickedBytes = $derived(bytesOf(picked));
	const allTicked = $derived(shown.length > 0 && picked.length === shown.length);
	const someTicked = $derived(picked.length > 0 && picked.length < shown.length);
	const narrowed = $derived(
		!!filters.text.trim() || filters.completeOnly || filters.likedOnly || !!(filters.formats.length + filters.qualities.length + filters.artists.length + filters.albums.length + filters.playlists.length)
	);

	function flip<K extends 'formats' | 'qualities'>(key: K, value: Filters[K][number]) {
		const list = filters[key] as string[];
		filters[key] = (list.includes(value) ? list.filter((v) => v !== value) : [...list, value]) as Filters[K];
	}

	function tickShown(on: boolean) {
		for (const s of shown) {
			if (on) unticked.delete(s.file);
			else unticked.add(s.file);
		}
	}

	function tickOne(file: string) {
		if (unticked.has(file)) unticked.delete(file);
		else unticked.add(file);
	}

	async function take() {
		if (!offer || !picked.length) return;
		sending = true;
		failure = null;
		try {
			await core.post('/api/phone/take', { sent_at_ms: offer.sent_at_ms, files: picked.map((s) => s.file) });
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
</script>

<div class="view view-in ps">
	<div class="pg-head"><h1 class="pg-title">Songs from {phone}</h1></div>
	{#if offer}
		<p class="pg-sub">
			{songsWord(all.length)} sent · {here ? `${here.toLocaleString()} on this computer already · ` : ''}{size(bytesOf(all))}. Choose which come in.
		</p>
		<div class="ps-tools">
			<label class="ps-find"><Icon name="search" /><input type="search" placeholder="Find a song, artist or album" aria-label="Find a song, artist or album" bind:value={filters.text} maxlength="200" /></label>
			{#if here}
				<button type="button" class="chip" class:on={filters.newOnly} aria-pressed={filters.newOnly} onclick={() => (filters.newOnly = !filters.newOnly)}>Only new songs</button>
			{/if}
			<button type="button" class="chip" class:on={filters.completeOnly} aria-pressed={filters.completeOnly} title="With a title, artist, album and cover" onclick={() => (filters.completeOnly = !filters.completeOnly)}>Complete tags</button>
			{#if base.some((s) => s.liked)}
				<button type="button" class="chip" class:on={filters.likedOnly} aria-pressed={filters.likedOnly} onclick={() => (filters.likedOnly = !filters.likedOnly)}>Liked</button>
			{/if}
		</div>
		<div class="ps-tools" role="group" aria-label="Filters">
			{#each f.formats as x (x.value)}
				<button type="button" class="chip" class:on={filters.formats.includes(x.value)} aria-pressed={filters.formats.includes(x.value)} onclick={() => flip('formats', x.value)}>{x.label} <span class="ps-n">{x.count.toLocaleString()}</span></button>
			{/each}
			{#if f.qualities.length > 1}
				<span class="ps-gap" aria-hidden="true"></span>
				{#each f.qualities as x (x.value)}
					<button type="button" class="chip" class:on={filters.qualities.includes(x.value as Quality)} aria-pressed={filters.qualities.includes(x.value as Quality)} onclick={() => flip('qualities', x.value as Quality)}>{x.label} <span class="ps-n">{x.count.toLocaleString()}</span></button>
				{/each}
			{/if}
			<span class="ps-gap" aria-hidden="true"></span>
			<PhoneFacet label="Artists" facets={f.artists} bind:chosen={filters.artists} />
			{#if f.albums.length}<PhoneFacet label="Albums" facets={f.albums} bind:chosen={filters.albums} />{/if}
			{#if f.playlists.length}<PhoneFacet label="Playlists" facets={f.playlists} bind:chosen={filters.playlists} />{/if}
			{#if narrowed}
				<button type="button" class="ps-reset" onclick={() => (filters = { ...NO_FILTERS, newOnly: filters.newOnly })}>Clear filters</button>
			{/if}
		</div>

		<div class="ps-list" role="group" aria-label="Songs">
			<label class="ps-h">
				<input type="checkbox" checked={allTicked} indeterminate={someTicked} disabled={!shown.length} onchange={() => tickShown(!allTicked)} />
				<span>{shown.length === all.length ? songsWord(shown.length) : `${shown.length.toLocaleString()} of ${songsWord(all.length)} shown`}</span>
			</label>
			{#each shown as s (s.file)}
				<label class="ps-r" class:ps-off={unticked.has(s.file)}>
					<input type="checkbox" checked={!unticked.has(s.file)} onchange={() => tickOne(s.file)} />
					<Cover artwork={s.art_url ? [{ url: core.artUrl(s.art_url) ?? '' }] : undefined} seed={s.album ?? s.title} class="ps-art" />
					<span class="ps-song">
						<b>{s.title}{#if s.here}<i class="ps-here">On this computer</i>{/if}</b>
						<span>{[s.artist, artistOf(s) === s.artist ? '' : artistOf(s), s.album ?? ''].filter(Boolean).join(' · ')}</span>
					</span>
					<span class="ps-fmt">{detailOf(s)}</span>
					<span class="ps-size">{size(s.bytes)}</span>
				</label>
			{:else}
				<p class="s-hint">No songs match. <button type="button" class="ps-reset" onclick={() => (filters = { ...NO_FILTERS, newOnly: false })}>Show them all</button></p>
			{/each}
		</div>

		<div class="ps-bar">
			{#if failure}<span class="pg-status bad" role="alert">{failure}</span>{:else}<span class="pg-status">Songs you don't take stay in Stash on {phone}.</span>{/if}
			<button type="button" class="btn" disabled={!picked.length || sending} onclick={take}>
				{sending ? 'Starting…' : picked.length ? `Take ${songsWord(picked.length)} · ${size(pickedBytes)}` : 'Tick the songs to take'}
			</button>
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
	.ps { padding-bottom: 0; }
	.ps-tools { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 0 0 10px; }
	.ps-find { display: flex; align-items: center; gap: 8px; width: 280px; max-width: 100%; height: 36px; padding: 0 12px; border-radius: 12px; background: var(--wp-hover); color: var(--wp-text-2); }
	.ps-find :global(svg) { width: 18px; height: 18px; fill: currentColor; flex: none; }
	.ps-find input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--wp-text); font: 400 13.5px/1 var(--font-body); }
	.ps-n { color: var(--wp-text-2); font-weight: 500; font-variant-numeric: tabular-nums; }
	.ps-gap { width: 1px; height: 20px; margin: 0 4px; background: var(--wp-line-2); }
	.ps-reset { height: 32px; padding: 0 10px; border-radius: 8px; color: var(--acc-text); font: 600 13px/1 var(--font-body); }
	.ps-reset:hover { background: var(--wp-hover); }
	.ps-list { margin: 6px 0 0; border-top: 1px solid var(--wp-line-2); }
	.ps-h, .ps-r { display: grid; grid-template-columns: 20px 36px minmax(0, 1fr) 128px 64px; align-items: center; gap: 12px; padding: 4px 8px; border-radius: 8px; font: 400 13.5px/1.35 var(--font-body); cursor: pointer; }
	.ps-h { grid-template-columns: 20px minmax(0, 1fr); min-height: 40px; color: var(--wp-text-2); font-weight: 500; }
	.ps-r { min-height: 48px; content-visibility: auto; contain-intrinsic-size: auto 48px; }
	@media (hover: hover) and (pointer: fine) { .ps-r:hover { background: var(--wp-hover); } }
	.ps-h input, .ps-r input { width: 16px; height: 16px; accent-color: var(--acc); }
	.ps-off > :not(input) { opacity: 0.45; }
	.ps-r :global(.ps-art) { display: block; width: 36px; height: 36px; border-radius: 6px; object-fit: cover; }
	.ps-song { display: grid; min-width: 0; }
	.ps-song b { font-weight: 500; color: var(--wp-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ps-song span, .ps-fmt, .ps-size { color: var(--wp-text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ps-song span { font-size: 12.5px; }
	.ps-here { margin-left: 8px; padding: 1px 7px; border-radius: 999px; background: var(--wp-hover); color: var(--wp-text-2); font: 500 11.5px/1.4 var(--font-body); font-style: normal; vertical-align: 1px; }
	.ps-size { text-align: right; font-variant-numeric: tabular-nums; }
	.ps-bar { position: sticky; bottom: 0; z-index: 5; display: flex; align-items: center; justify-content: space-between; gap: 16px; margin: 8px -8px 0; padding: 12px 8px 16px; background: var(--wp-main); border-top: 1px solid var(--wp-line-2); }
</style>
