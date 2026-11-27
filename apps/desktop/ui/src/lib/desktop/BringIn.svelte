<!-- The first time Stash opens: bringing in the music on the listener's phone. Until a phone's send is waiting, how to
     plug it in (and other ways to start). Then what the phone sent, cover by cover, all of it coming (tap one to leave
     it out): its downloads, its likes (one card per service they are liked on), plays, the listener's own playlists
     and Stash Mixes, and every playlist, album, mix and radio synced from a streaming service; and one switch for songs
     downloaded only (Stash then stays offline, as "My own music files" did). Bring them in takes the send: the library parts come in at once, the files after (the corner card
     says how they go), and Stash opens on Library. The mockup the owner picked: "A · Shelves". -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { setMode } from '../app';
	import { artUrl, get, post, said, type Mode } from '../core';
	import Cover from '../ui/Cover.svelte';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';
	import { folders } from './folders.svelte';
	import { firstPicks, flip, gb, isOn, LIKE_NAME, likeParts, lines, listLine, n, ownLists, setKeys, syncedLists, takeBody, tally, type PhoneLibrary, type Picks, type SentList } from './bring-in';
	import type { Offer } from './phone-songs';

	/** `first`: the first time Stash opens (how it runs is set by what is brought in); else a later send, from the corner card. */
	let { first = true }: { first?: boolean } = $props();

	/** What `/api/phone` says (it doesn't name the send: `/api/phone/songs` does). `at_ms`: when it last changed. */
	type Now = { state: string; phone: string | null; at_ms: number; with_library?: boolean };

	/** Songs on their way, but how Stash runs could not be set: set again (and only that) with Try again. */
	let owed = $state<Mode | null>(null);
	let now = $state<Now | null>(null);
	/** The `at_ms` of what `/api/phone` said when the send on screen was loaded: a change may be another send. */
	let seenAt = 0;
	/** The send on screen was loaded once its library was said to be there: not loaded again for it. */
	let libAsked = false;
	let offer = $state<Offer | null>(null);
	let lib = $state<PhoneLibrary | null>(null);
	let picks = $state<Picks | null>(null);
	let q = $state('');
	let busy = $state(false);
	let trouble = $state<string | null>(null);

	/** The send to choose from, and what is ticked of it. */
	async function load() {
		const sent = await get<Offer>('/api/phone/songs');
		let library: PhoneLibrary | null = null;
		try {
			library = await get<PhoneLibrary>('/api/phone/library');
		} catch {
			/* a phone too old to send its library, or one whose library could not be read: its songs alone */
		}
		const fresh = library && library.sent_at_ms === sent.sent_at_ms ? library : null;
		const again = !!offer && !!picks && offer.sent_at_ms === sent.sent_at_ms;
		// The same send's library, come after its songs: its parts are ticked, the songs' ticks stay as they were.
		const late = again && !lib && !!fresh;
		offer = sent;
		lib = fresh ?? (again ? lib : null);
		// The same send again, with nothing new to show: what is ticked stays as the listener left it.
		if (again && !late) return;
		if (late && picks) {
			picks = { ...firstPicks(sent, lib), downloads: picks.downloads, onlyHere: picks.onlyHere };
			return;
		}
		picks = firstPicks(sent, lib);
		// Stash offline for good (on the listener's own files): a song not downloaded could never play, so none comes.
		if (!first && ui.local) picks = { ...picks, onlyHere: true };
		q = '';
	}

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const ask = async () => {
			try {
				const told = await get<Now>('/api/phone');
				if (!alive) return;
				now = told;
				if (!busy) {
					// A send to load: the first, perhaps another (what is said changed), or this one's library, read only
					// after its songs were offered (the phone answered late).
					if (told.state === 'ready' && (!offer || told.at_ms !== seenAt || (!!told.with_library && !lib && !libAsked))) {
						await load();
						seenAt = told.at_ms;
						libAsked = !!told.with_library;
					} else if (told.state !== 'ready' && offer) {
						offer = null;
						lib = null;
						picks = null;
					}
				}
			} catch {
				/* the core is not answering: asked again */
			}
			if (alive) timer = setTimeout(ask, 2000);
		};
		void ask();
		return () => {
			alive = false;
			clearTimeout(timer);
			end();
		};
	});

	const plugged = $derived(!!now && now.state !== 'idle' && !!now.phone);
	const phone = $derived(offer?.phone ?? now?.phone ?? 'Your phone');
	const t = $derived(offer && picks ? tally(offer, lib, picks) : null);
	const bar = $derived(t && picks ? lines(t, picks) : null);
	const fresh = $derived(offer ? offer.songs.filter((s) => !s.here) : []);
	const own = $derived(ownLists(lib));
	const synced = $derived(syncedLists(lib));
	const found = $derived.by(() => {
		const k = q.trim().toLowerCase();
		return k ? synced.filter((p) => p.name.toLowerCase().includes(k)) : synced;
	});
	const added = $derived(picks ? synced.filter((p) => picks!.lists.includes(p.id)).length : 0);
	/** Of the synced lists, those on the computer already. */
	const syncedHere = $derived(synced.filter((p) => p.here).length);
	const likes = $derived(likeParts(lib));
	/** The covers of the send's albums (most songs first), or of its songs when the phone sent no library. */
	const albumArt = $derived.by(() => {
		const seen = new Set<string>();
		const out: string[] = [];
		const from = lib ? lib.albums.map((a) => a.art_url) : (offer?.songs ?? []).map((s) => s.art_url);
		for (const u of from) if (u && !seen.has(u)) (seen.add(u), out.push(u));
		return out;
	});
	/** The wall behind the heading: forty covers, repeated when there are fewer. */
	const wall = $derived(albumArt.length ? Array.from({ length: 40 }, (_, i) => albumArt[i % albumArt.length]) : []);
	const GHOST = Array.from({ length: 40 }, (_, i) => ({ d: (i * 389) % 3400, t: `hsl(${245 + ((i * 37) % 50)} 34% ${17 + ((i * 13) % 14)}%)` }));

	const toggle = (key: string) => {
		if (picks && !busy) picks = flip(picks, key);
	};
	/** Ticks, or leaves out, every one of `keys` (Select all, Select none). */
	const setAll = (keys: string[], on: boolean) => {
		if (picks && !busy) picks = setKeys(picks, keys, on);
	};
	const ownKeys = $derived([...(fresh.length ? ['downloads'] : []), ...likes.map((l) => `likes:${l.service}`), ...(lib?.plays ? ['plays'] : []), ...own.map((p) => p.id)]);
	const foundKeys = $derived(found.map((p) => p.id));

	// A drag across covers picks many at once, as on the Windows desktop (owner: "users need to be able to drag to
	// select multiple icons like the windows home screen, as well as deselect options"). The covers inside the box all
	// go one way: left out if the drag began on a cover that was coming, else ticked. Near the view's top or bottom
	// it scrolls. A drag that never moves 6 px is a tap.
	type Sweep = { on: boolean; keys: Set<string>; box: { left: number; top: number; width: number; height: number } };
	let sweep = $state<Sweep | null>(null);
	let root = $state<HTMLElement>();
	let drag: { x: number; y: number; px: number; py: number; on: boolean; scroller: HTMLElement | null; moved: boolean; raf: number } | null = null;
	/** Whether `key` shows as coming: as the drag under way would leave it, else as ticked. */
	const shownOn = (key: string) => (sweep?.keys.has(key) ? sweep.on : !!picks && isOn(picks, key));

	function down(e: PointerEvent) {
		if (e.button !== 0 || e.pointerType === 'touch' || busy || !picks) return;
		const el = e.target as HTMLElement;
		if (el.closest('input, label, .bi-sw, .bi-sel, .bi-take, .bi-alts, .bi-owed')) return;
		e.preventDefault();
		const card = el.closest<HTMLElement>('[data-key]');
		const scroller = (e.currentTarget as HTMLElement).closest<HTMLElement>('.main') ?? (document.scrollingElement as HTMLElement | null);
		const sy = scroller?.scrollTop ?? 0;
		drag = { x: e.clientX, y: e.clientY + sy, px: e.clientX, py: e.clientY, on: card?.dataset.key ? !isOn(picks, card.dataset.key) : true, scroller, moved: false, raf: 0 };
		window.addEventListener('pointermove', move);
		window.addEventListener('pointerup', up);
		window.addEventListener('pointercancel', cancel);
		window.addEventListener('keydown', escape);
	}
	function move(e: PointerEvent) {
		if (!drag) return;
		drag.px = e.clientX;
		drag.py = e.clientY;
		if (!drag.moved) {
			if (Math.hypot(e.clientX - drag.x, e.clientY + (drag.scroller?.scrollTop ?? 0) - drag.y) < 6) return;
			drag.moved = true;
			drag.raf = requestAnimationFrame(follow);
		}
		measure();
	}
	/** Near the view's top or bottom, scrolls it (faster the nearer the edge), the box keeping its start. */
	function follow() {
		if (!drag) return;
		const s = drag.scroller;
		if (s) {
			const r = s === document.scrollingElement ? { top: 0, bottom: innerHeight } : s.getBoundingClientRect();
			const by = drag.py < r.top + 64 ? drag.py - (r.top + 64) : drag.py > r.bottom - 64 ? drag.py - (r.bottom - 64) : 0;
			if (by) {
				s.scrollTop += Math.max(-28, Math.min(28, by / 2));
				measure();
			}
		}
		drag.raf = requestAnimationFrame(follow);
	}
	/** The box, and the covers it touches (in a shelf, only those the shelf shows). */
	function measure() {
		if (!drag || !root) return;
		const sy = drag.scroller?.scrollTop ?? 0;
		const x0 = Math.min(drag.x, drag.px), x1 = Math.max(drag.x, drag.px);
		const y0 = Math.min(drag.y, drag.py + sy), y1 = Math.max(drag.y, drag.py + sy);
		const keys = new Set<string>();
		for (const c of root.querySelectorAll<HTMLElement>('[data-key]')) {
			const b = c.getBoundingClientRect();
			const clip = c.closest('.bi-shelf')?.getBoundingClientRect();
			const left = Math.max(b.left, clip?.left ?? -Infinity), right = Math.min(b.right, clip?.right ?? Infinity);
			if (right > left && right >= x0 && left <= x1 && b.bottom + sy >= y0 && b.top + sy <= y1 && c.dataset.key) keys.add(c.dataset.key);
		}
		sweep = { on: drag.on, keys, box: { left: x0, top: y0 - sy, width: x1 - x0, height: y1 - y0 } };
	}
	function up() {
		const moved = !!drag?.moved;
		end();
		const done = sweep;
		sweep = null;
		if (!moved || !done || !picks) return;
		picks = setKeys(picks, [...done.keys], done.on);
		// The click that ends a drag is not a tap on the cover under it.
		const eat = (e: Event) => (e.stopPropagation(), e.preventDefault());
		window.addEventListener('click', eat, { capture: true, once: true });
		setTimeout(() => window.removeEventListener('click', eat, { capture: true }), 0);
	}
	function cancel() {
		end();
		sweep = null;
	}
	function escape(e: KeyboardEvent) {
		if (e.key === 'Escape') cancel();
	}
	function end() {
		if (drag?.raf) cancelAnimationFrame(drag.raf);
		drag = null;
		window.removeEventListener('pointermove', move);
		window.removeEventListener('pointerup', up);
		window.removeEventListener('pointercancel', cancel);
		window.removeEventListener('keydown', escape);
	}
	const art = (u: string) => [{ url: artUrl(u) ?? '' }];
	const initial = (name: string) => name.match(/[A-Za-z0-9]/)?.[0].toUpperCase() ?? '#';

	/**
	 * Takes the send as ticked and opens Library. The first time, also sets how Stash runs (offline for good with songs
	 * downloaded only); if that fails once the songs are on their way, only that is tried again.
	 */
	async function bring() {
		if (!offer || !picks || busy || t?.nothing) return;
		busy = true;
		trouble = null;
		const mode: Mode = picks.onlyHere ? 'local' : 'everything';
		try {
			await post('/api/phone/take', takeBody(offer, lib, picks));
		} catch (e) {
			trouble = said(e);
			busy = false;
			return;
		}
		if (first) owed = mode;
		await finish();
	}

	/** Sets how Stash runs, if that is owed, and opens Library. */
	async function finish() {
		busy = true;
		trouble = null;
		try {
			if (owed) await setMode(owed);
			owed = null;
			void goto('/library');
		} catch (e) {
			trouble = said(e);
		} finally {
			busy = false;
		}
	}

	/** Starts another way: on files of this computer's (a folder chosen next), with streaming, or from a backup file. */
	async function start(mode: Mode, then?: () => unknown) {
		if (busy) return;
		busy = true;
		trouble = null;
		try {
			await setMode(mode);
			await then?.();
		} catch (e) {
			trouble = said(e);
		} finally {
			busy = false;
		}
	}
</script>

{#snippet cover(covers: string[], seed: string)}
	{#if covers.length >= 4}
		<span class="mos">{#each covers.slice(0, 4) as c, i (i)}<Cover artwork={art(c)} {seed} min={60} />{/each}</span>
	{:else if covers.length}
		<Cover artwork={art(covers[0])} {seed} />
	{:else}
		<Cover {seed} ini={initial(seed)} />
	{/if}
{/snippet}

{#snippet badge(on: boolean)}
	<span class="bi-pk" aria-hidden="true"><Icon name={on ? 'check' : 'add'} /></span>
{/snippet}

{#snippet listCard(p: SentList)}
	{@const on = shownOn(p.id)}
	<button type="button" class="bi-card" class:on data-key={p.id} aria-pressed={on} disabled={busy} onclick={() => toggle(p.id)}>
		<span class="tile bi-art">
			{@render cover(p.covers, p.name)}
			{#if p.here}<span class="bi-here">On your computer</span>{/if}
		</span>
		{@render badge(on)}
		<b>{p.name}</b>
		<i>{listLine(p, !!picks?.onlyHere)}</i>
	</button>
{/snippet}

{#snippet others(title: string, line: string)}
	<div class="bi-sec"><div><h2>{title}</h2><p>{line}</p></div></div>
	<div class="bi-alts">
		<button type="button" class="bi-alt" disabled={busy} onclick={() => void start('local', () => folders.add())}>
			<span class="bi-alt-ic" aria-hidden="true"><Icon name="folder" /></span>
			<span><b>Use music files on this computer</b><i>Pick a folder. Stash stays offline.</i></span>
		</button>
		<button type="button" class="bi-alt" disabled={busy} onclick={() => void start('everything')}>
			<span class="bi-alt-ic" aria-hidden="true"><Icon name="radio" /></span>
			<span><b>Start with streaming</b><i>Search, play and discover. Add your own music later.</i></span>
		</button>
		<button type="button" class="bi-alt" disabled={busy} onclick={() => void start('everything', () => goto('/settings?c=library'))}>
			<span class="bi-alt-ic" aria-hidden="true"><Icon name="library_add" /></span>
			<span><b>Import a backup file</b><i>A library file saved from Stash on your phone or the web.</i></span>
		</button>
	</div>
{/snippet}

{#snippet sel(keys: string[], of: string)}
	{@const coming = picks ? keys.filter((k) => isOn(picks!, k)).length : 0}
	<!-- Never disabled, which would drop the keyboard's place: one with nothing to do says so and does nothing. -->
	<div class="bi-sel" role="group" aria-label="Select {of}">
		<button type="button" aria-disabled={busy || coming === keys.length} onclick={() => coming < keys.length && setAll(keys, true)}>Select all</button>
		<button type="button" aria-disabled={busy || !coming} onclick={() => coming > 0 && setAll(keys, false)}>Select none</button>
	</div>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section class="bi" aria-labelledby="bi-h" bind:this={root} onpointerdown={down}>
	{#if owed}
		<div class="bi-owed" role="alert">
			<span>Your music is coming in, but Stash couldn’t finish setting up{trouble ? `: ${trouble}` : '.'}</span>
			<button type="button" class="bi-go" disabled={busy} onclick={() => void finish()}>{busy ? 'Trying…' : 'Try again'}</button>
		</div>
	{:else if !offer || !picks}
		<div class="bi-hero">
			<div class="bi-wall" aria-hidden="true">
				{#each GHOST as g, i (i)}<span class="bi-ghost" style="--d:{g.d}ms;background:{g.t}"></span>{/each}
			</div>
			<div class="bi-in">
				<p class="bi-eye">
					{#if plugged}
						<Icon name="smartphone" />{now?.phone} plugged in{#if now?.state === 'preparing'}<span class="bi-dot"></span>getting its songs ready{/if}
					{:else}
						<span class="bi-spin" aria-hidden="true"></span>Looking for your phone
					{/if}
				</p>
				<h1 id="bi-h" class="bi-h">Bring your music from your phone</h1>
				<p class="bi-p">Your songs, likes, playlists and plays from Stash on your phone, all here.</p>
			</div>
		</div>
		<ol class="bi-steps">
			<li class:done={plugged}><span class="k">{#if plugged}<Icon name="check" />{:else}1{/if}</span><b>Plug it in</b><span>Connect your phone to this computer with a USB cable.</span></li>
			<li class:done={plugged}><span class="k">{#if plugged}<Icon name="check" />{:else}2{/if}</span><b>Choose File transfer</b><span>On the phone, tap the USB notification, then File transfer.</span></li>
			<li><span class="k">3</span><b>Tap Send to computer</b><span>In Stash on the phone: Settings › Library &amp; Storage › Send to computer.</span></li>
		</ol>
		{#if first}{@render others('No phone with Stash?', 'Start another way. You can bring your phone’s music in later.')}{/if}
		{#if trouble}<p class="bi-trouble" role="alert">{trouble}</p>{/if}
	{:else}
		<div class="bi-hero">
			<div class="bi-wall" aria-hidden="true">
				{#each wall as u, i (i)}<Cover artwork={art(u)} seed={u} class="cv bi-wc" />{:else}{#each GHOST as g, i (i)}<span class="bi-ghost still" style="background:{g.t}"></span>{/each}{/each}
			</div>
			<div class="bi-in">
				<p class="bi-eye"><Icon name="smartphone" />{phone}<span class="bi-dot ok"></span>plugged in</p>
				<h1 id="bi-h" class="bi-h">Your music is on your phone</h1>
				<p class="bi-p bi-counts">
					<b>{n(offer.songs.length)}</b> songs downloaded
					{#if lib}
						<span>·</span> <b>{n(lib.likes.songs)}</b> liked <span>·</span> <b>{n(lib.playlists.length)}</b> playlists and mixes <span>·</span> <b>{n(lib.plays)}</b> plays
					{/if}
				</p>
				{#if lib && !first && ui.local}
					<p class="bi-old">Stash plays only your own files, so only songs you downloaded come in.</p>
				{:else if lib}
					<button type="button" role="switch" class="bi-sw" class:on={picks.onlyHere} aria-checked={picks.onlyHere} disabled={busy} onclick={() => picks && (picks = { ...picks, onlyHere: !picks.onlyHere })}>
						<span class="bi-sw-k" aria-hidden="true"></span>{picks.onlyHere ? `Only songs you downloaded. Nothing streams${first ? ', and Stash stays offline' : ''}.` : 'Bring only the songs I downloaded'}
					</button>
				{:else}
					<p class="bi-old">{phone} sent its songs only. To bring your likes, playlists and plays too, update Stash on the phone and send again.</p>
				{/if}
			</div>
		</div>

		<div class="bi-sec">
			<div><h2>Coming with you</h2><p>{lib ? 'Your downloads, likes, plays and playlists. Tap one to leave it out, or drag across several.' : 'Tap it to leave it out.'}</p></div>
			{#if ownKeys.length > 1}{@render sel(ownKeys, 'what comes with you')}{/if}
		</div>
		<div class="bi-shelf">
			{#if fresh.length}
				<button type="button" class="bi-card" class:on={shownOn('downloads')} data-key="downloads" aria-pressed={shownOn('downloads')} disabled={busy} onclick={() => toggle('downloads')}>
					<span class="tile bi-art">{@render cover(albumArt.slice(0, 4), 'downloads')}</span>
					{@render badge(shownOn('downloads'))}
					<b>All your downloads</b>
					<i>{n(fresh.length)} songs · {gb(fresh.reduce((sum, s) => sum + s.bytes, 0))}</i>
				</button>
			{/if}
			{#each likes as l (l.service)}
				{@const on = shownOn(`likes:${l.service}`)}
				<button type="button" class="bi-card" class:on data-key="likes:{l.service}" aria-pressed={on} disabled={busy} onclick={() => toggle(`likes:${l.service}`)}>
					<span class="tile bi-art"><span class="tile-liked bi-lk-{l.service}"><Icon name="favorite" /></span></span>
					{@render badge(on)}
					<b>{LIKE_NAME[l.service] ?? 'Liked songs'}</b>
					<i>{picks.onlyHere ? `${n(l.downloaded)} downloaded songs` : `${n(l.songs)} songs · ${n(l.downloaded)} downloaded`}</i>
				</button>
			{/each}
			{#if lib && lib.plays}
				<button type="button" class="bi-card" class:on={shownOn('plays')} data-key="plays" aria-pressed={shownOn('plays')} disabled={busy} onclick={() => toggle('plays')}>
					<span class="tile bi-art"><span class="bi-stat"><b>{n(lib.plays)}</b><i>plays</i></span></span>
					{@render badge(shownOn('plays'))}
					<b>Your plays</b>
					<i>For your stats and mixes</i>
				</button>
			{/if}
			{#each own as p (p.id)}{@render listCard(p)}{/each}
		</div>

		{#if synced.length}
			<div class="bi-sec">
				<div>
					<h2>From your streaming services</h2>
					<p>
						{added === synced.length ? `All ${n(synced.length)} coming` : added ? `${n(added)} of ${n(synced.length)} coming` : `${n(synced.length)} synced on your phone, none coming`}{syncedHere ? `, ${n(syncedHere)} on your computer already (ticked, they're updated)` : ''}.
						{added ? `Their downloaded songs come in as files${picks.onlyHere ? '.' : ', the rest stream.'}` : ''} Tap one, or drag across many at once.
					</p>
				</div>
				<div class="bi-tools">
					{#if foundKeys.length}{@render sel(foundKeys, q.trim() ? 'the playlists found' : 'every streaming playlist')}{/if}
					<label class="bi-find"><Icon name="search" /><input id="bi-find" type="search" placeholder="Find a playlist" aria-label="Find a playlist" bind:value={q} /></label>
				</div>
			</div>
			<div class="bi-grid">
				{#each found as p (p.id)}{@render listCard(p)}{/each}
			</div>
			{#if !found.length}<p class="bi-none">No synced playlist called “{q.trim()}”.</p>{/if}
		{/if}

		{#if lib && lib.albums.length}
			<div class="bi-sec"><div><h2>Albums you downloaded</h2><p>{picks.downloads ? 'They come in with your downloads and play offline.' : 'Not coming: you left All your downloads out.'}</p></div></div>
			<div class="bi-shelf" class:off={!picks.downloads}>
				{#each lib.albums as a, i (i)}
					<div class="bi-card still">
						<span class="tile bi-art">{#if a.art_url}<Cover artwork={art(a.art_url)} seed={a.title} />{:else}<Cover seed={a.title} ini={initial(a.title)} />{/if}</span>
						<b>{a.title}</b>
						<i>{a.artist}</i>
					</div>
				{/each}
			</div>
		{/if}

		{#if first}{@render others('Or start another way', 'Leave this send on the phone for now. You can bring it in later.')}{/if}

		{#if bar && !owed}
			<div class="bi-take">
				<div class="bi-take-in" role="region" aria-label="What comes in">
					<span class="bi-take-t" aria-live="polite"><b>{bar.top}</b><i>{trouble ?? bar.sub}</i></span>
					<button type="button" class="bi-go" disabled={busy || !!t?.nothing} onclick={() => void bring()}>
						<Icon name="download" />{busy ? 'Bringing them in…' : 'Bring them in'}
					</button>
				</div>
			</div>
		{/if}
	{/if}
	{#if sweep}<div class="bi-box" style="left:{sweep.box.left}px;top:{sweep.box.top}px;width:{sweep.box.width}px;height:{sweep.box.height}px" aria-hidden="true"></div>{/if}
</section>

<style>
	.bi { padding-bottom: 120px; }
	.bi-hero { position: relative; height: 318px; margin: -24px -24px 0; overflow: hidden; isolation: isolate; }
	.bi-wall { position: absolute; top: -46%; left: -12%; width: 124%; display: grid; grid-template-columns: repeat(10, minmax(0, 1fr)); gap: 10px; transform: rotate(-7deg); opacity: .8; animation: bi-drift 90s linear infinite alternate; }
	.bi-wall :global(.bi-wc) { width: 100%; aspect-ratio: 1; height: auto; border-radius: 6px; box-shadow: 0 10px 24px -10px rgb(0 0 0 / .7); }
	.bi-ghost { aspect-ratio: 1; border-radius: 6px; animation: bi-breathe 3.4s ease-in-out infinite; animation-delay: var(--d); }
	.bi-ghost.still { animation: none; opacity: .5; }
	.bi-hero::after { content: ""; position: absolute; inset: 0; z-index: 1; background: linear-gradient(to top, var(--wp-main) 4%, color-mix(in oklab, var(--wp-main) 82%, transparent) 42%, color-mix(in oklab, var(--wp-main) 30%, transparent)), linear-gradient(to right, color-mix(in oklab, var(--wp-main) 90%, transparent) 10%, color-mix(in oklab, var(--wp-main) 15%, transparent) 75%); }
	.bi-in { position: absolute; z-index: 2; left: 32px; right: 32px; bottom: 24px; display: grid; justify-items: start; gap: 12px; }
	.bi-eye { display: inline-flex; align-items: center; gap: 8px; height: 30px; margin: 0; padding: 0 13px 0 10px; border-radius: 999px; background: rgb(255 255 255 / .1); backdrop-filter: blur(10px); font: 600 12.5px/1 var(--font-body); color: var(--wp-text); }
	.bi-eye :global(.ic) { width: 16px; height: 16px; }
	.bi-dot { width: 7px; height: 7px; margin-left: 2px; border-radius: 50%; background: var(--wp-text-2); }
	.bi-dot.ok { background: #34d399; box-shadow: 0 0 0 3px rgb(52 211 153 / .22); }
	.bi-spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid color-mix(in oklab, var(--acc-text) 30%, transparent); border-top-color: var(--acc-text); animation: bi-spin .9s linear infinite; }
	.bi-h { max-width: 15ch; margin: 0; font: 700 52px/1 var(--font-display); letter-spacing: -.04em; text-wrap: balance; color: var(--wp-text); }
	.bi-p { margin: 0; color: var(--wp-text-2); font: 400 15px/1.45 var(--font-body); font-variant-numeric: tabular-nums; }
	.bi-counts b { color: var(--wp-text); font-weight: 600; }
	.bi-counts span { margin: 0 4px; color: var(--wp-num); }
	.bi-old { margin: 0; color: var(--wp-text-2); font: 400 13.5px/1.4 var(--font-body); }
	.bi-sw { display: inline-flex; align-items: center; gap: 10px; padding: 4px 0; color: var(--wp-text-2); font: 500 13.5px/1.35 var(--font-body); text-align: left; }
	.bi-sw.on { color: var(--wp-text); }
	.bi-sw-k { position: relative; flex: none; width: 36px; height: 20px; border-radius: 999px; background: rgb(255 255 255 / .2); transition: background-color var(--dur-short); }
	.bi-sw-k::after { content: ""; position: absolute; top: 3px; left: 3px; width: 14px; height: 14px; border-radius: 50%; background: #fff; transition: transform 300ms cubic-bezier(.3, 1.5, .5, 1); }
	.bi-sw.on .bi-sw-k { background: var(--acc); }
	.bi-sw.on .bi-sw-k::after { transform: translateX(16px); }

	.bi-sec { display: flex; align-items: flex-end; justify-content: space-between; flex-wrap: wrap; gap: 10px 16px; margin: 32px 0 12px; }
	.bi-sec h2 { margin: 0; font: 700 22px/1.2 var(--font-display); letter-spacing: -.015em; color: var(--wp-text); }
	.bi-sec p { margin: 4px 0 0; color: var(--wp-text-2); font: 400 13.5px/1.4 var(--font-body); }
	.bi-find { display: flex; align-items: center; gap: 8px; width: 250px; max-width: 100%; height: 38px; padding: 0 12px; border-radius: 999px; background: var(--wp-hover); color: var(--wp-text-2); }
	.bi-find :global(.ic) { width: 18px; height: 18px; flex: none; }
	.bi-find input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--wp-text); font: 400 13.5px/1 var(--font-body); }
	.bi-find:has(input:focus-visible) { outline: 2px solid var(--acc); outline-offset: 2px; }

	.bi-shelf { display: flex; gap: 4px; margin: 0 -12px; padding-bottom: 6px; overflow-x: auto; scrollbar-width: none; overscroll-behavior-x: contain; scroll-snap-type: x proximity; }
	.bi-shelf::-webkit-scrollbar { display: none; }
	.bi-shelf > .bi-card { flex: 0 0 164px; scroll-snap-align: start; }
	.bi-shelf.off { opacity: .32; filter: grayscale(.7); }
	.bi-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 4px; margin: 0 -12px; }
	.bi-card { position: relative; display: grid; grid-template-columns: minmax(0, 1fr); gap: 2px; align-content: start; min-width: 0; padding: 12px; border-radius: 12px; text-align: left; color: var(--wp-text); transition: background-color var(--dur-short); }
	.bi-card:disabled { cursor: default; }
	.bi-art { width: 100%; aspect-ratio: 1; margin-bottom: 8px; border-radius: 8px; outline: 2px solid transparent; outline-offset: 3px; transition: outline-color 200ms, transform 350ms var(--ease-out-expo); }
	.bi-art :global(.cv) { width: 100%; height: 100%; }
	.bi-card.on .bi-art { outline-color: var(--acc); }
	.bi-art { position: relative; }
	.bi-here { position: absolute; left: 6px; bottom: 6px; max-width: calc(100% - 12px); padding: 3px 7px; border-radius: 999px; overflow: clip; text-overflow: ellipsis; white-space: nowrap; background: rgb(0 0 0 / .72); color: #fff; font: 600 10.5px/1.2 var(--font-body); pointer-events: none; }
	.bi-card:not(.still):active .bi-art { transform: scale(.97); }
	.bi-card b { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 600 14px/1.3 var(--font-body); }
	.bi-card i { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 400 12px/1.35 var(--font-body); font-style: normal; color: var(--wp-text-2); }
	.bi-pk { position: absolute; top: 18px; right: 18px; display: grid; place-items: center; width: 30px; height: 30px; border-radius: 50%; background: rgb(6 6 12 / .66); color: #fff; backdrop-filter: blur(8px); box-shadow: 0 2px 10px rgb(0 0 0 / .45); transition: background-color var(--dur-short), transform 300ms cubic-bezier(.3, 1.5, .5, 1); }
	.bi-pk :global(.ic) { width: 18px; height: 18px; }
	.bi-card.on .bi-pk { background: var(--acc); transform: scale(1.08); }
	.bi-stat { display: grid; place-items: center; align-content: center; gap: 4%; width: 100%; height: 100%; container-type: inline-size; color: #fff; background: radial-gradient(120% 90% at 20% 0%, color-mix(in oklab, var(--acc) 55%, transparent), transparent 60%), linear-gradient(160deg, color-mix(in oklab, var(--np-dom) 70%, black), var(--wp-main)); }
	.bi-stat b { font: 700 21cqi/1 var(--font-display); font-variant-numeric: tabular-nums; }
	.bi-stat i { font: 600 9cqi/1 var(--font-body); font-style: normal; letter-spacing: .08em; text-transform: uppercase; color: var(--acc-text); }
	.bi-none { color: var(--wp-text-2); }
	.bi-lk-stash { background: linear-gradient(135deg, #8b5cf6, #2e1b6e); }
	.bi-lk-spotify { background: linear-gradient(135deg, #4a1fe0, #8fd0c5); }
	.bi-lk-youtube { background: linear-gradient(135deg, #ff2d55, #4d0718); }
	.bi-lk-soundcloud { background: linear-gradient(135deg, #ff7a1a, #6b2200); }

	.bi-steps { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; margin: 28px 0 0; padding: 0; list-style: none; }
	.bi-steps li { display: grid; grid-template-columns: 32px minmax(0, 1fr); column-gap: 12px; row-gap: 3px; align-content: start; padding: 16px; border-radius: 14px; background: rgb(255 255 255 / .035); border: 1px solid var(--wp-line); }
	.bi-steps .k { grid-row: span 2; display: grid; place-items: center; width: 32px; height: 32px; border-radius: 50%; background: var(--acc-wash); color: var(--acc-text); font: 700 15px/1 var(--font-display); }
	.bi-steps .k :global(.ic) { width: 18px; height: 18px; }
	.bi-steps li.done .k { background: var(--acc); color: #fff; }
	.bi-steps b { font: 600 15px/1.3 var(--font-body); color: var(--wp-text); }
	.bi-steps span:last-child { color: var(--wp-text-2); font: 400 13.5px/1.45 var(--font-body); }
	.bi-alts { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 8px; margin: 0 -8px; }
	.bi-alt { display: flex; align-items: center; gap: 14px; padding: 10px; border-radius: 12px; text-align: left; color: var(--wp-text); transition: background-color var(--dur-short); }
	.bi-alt-ic { display: grid; place-items: center; width: 48px; height: 48px; flex: none; border-radius: 10px; background: var(--wp-field); color: var(--wp-text-2); }
	.bi-alt-ic :global(.ic) { width: 22px; height: 22px; }
	.bi-alt span:last-child { display: grid; gap: 2px; min-width: 0; }
	.bi-alt b { font: 600 14px/1.3 var(--font-body); }
	.bi-alt i { font: 400 12.5px/1.35 var(--font-body); font-style: normal; color: var(--wp-text-2); }
	.bi-trouble { margin-top: 12px; color: #f87171; }
	.bi-box { position: fixed; z-index: 60; pointer-events: none; border: 1px solid var(--acc); border-radius: 4px; background: color-mix(in oklab, var(--acc) 18%, transparent); }
	.bi-tools { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; }
	.bi-sel { display: flex; gap: 6px; }
	.bi-sel button { height: 32px; padding: 0 13px; border-radius: 999px; border: 1px solid var(--wp-line-2); color: var(--wp-text); font: 600 12.5px/1 var(--font-body); white-space: nowrap; transition: background-color var(--dur-short); }
	.bi-sel button[aria-disabled='true'] { border-color: var(--wp-line); color: var(--wp-num); cursor: default; }
	.bi-owed { display: flex; align-items: center; flex-wrap: wrap; gap: 12px; margin: 0 0 16px; padding: 12px 12px 12px 16px; border-radius: 12px; background: rgb(251 191 36 / .12); color: var(--wp-text); font: 500 14px/1.4 var(--font-body); }
	.bi-owed span { flex: 1; min-width: 240px; }

	.bi-take { position: fixed; z-index: 45; left: 0; right: 0; bottom: calc(var(--bar-clear, 100px) + 16px); display: flex; justify-content: center; pointer-events: none; padding: 0 24px; }
	.bi-take-in { pointer-events: auto; display: flex; align-items: center; gap: 14px; min-width: 0; max-width: 100%; padding: 8px 8px 8px 20px; border-radius: 999px; background: color-mix(in oklab, var(--wp-field) 92%, transparent); backdrop-filter: saturate(150%) blur(18px); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); animation: bi-in 180ms var(--ease-out-expo) both; }
	.bi-take-t { display: grid; min-width: 0; }
	.bi-take-t b { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 600 14px/1.3 var(--font-body); font-variant-numeric: tabular-nums; color: var(--wp-text); }
	.bi-take-t i { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 400 12.5px/1.35 var(--font-body); font-style: normal; color: var(--wp-text-2); font-variant-numeric: tabular-nums; }
	.bi-go { display: inline-flex; align-items: center; gap: 8px; flex: none; height: 44px; padding: 0 22px; border-radius: 999px; background: var(--acc); color: var(--acc-ink, #fff); font: 600 14.5px/1 var(--font-body); white-space: nowrap; transition: filter var(--dur-short), transform 200ms cubic-bezier(.3, 1.5, .5, 1); }
	.bi-go :global(.ic) { width: 18px; height: 18px; }
	.bi-go:active:not(:disabled) { transform: scale(.97); }
	.bi-go:disabled { background: rgb(255 255 255 / .1); color: var(--wp-num); cursor: default; }

	@media (hover: hover) and (pointer: fine) {
		.bi-card:not(.still):not(:disabled):hover, .bi-alt:not(:disabled):hover { background: var(--wp-hover); }
		.bi-card:not(:disabled):hover .bi-pk { transform: scale(1.12); }
		.bi-go:not(:disabled):hover { filter: brightness(1.1); }
		.bi-sw:not(:disabled):hover { color: var(--wp-text); }
		.bi-sel button:not([aria-disabled='true']):hover { background: var(--wp-hover); }
	}
	@keyframes bi-drift { from { transform: rotate(-7deg) translateX(0); } to { transform: rotate(-7deg) translateX(-7%); } }
	@keyframes bi-breathe { 0%, 100% { opacity: .3; } 50% { opacity: .75; } }
	@keyframes bi-spin { to { transform: rotate(360deg); } }
	@keyframes bi-in { from { opacity: 0; translate: 0 6px; } }
	@media (prefers-reduced-motion: reduce) { .bi-wall, .bi-ghost, .bi-spin, .bi-take-in { animation: none; } }
</style>
