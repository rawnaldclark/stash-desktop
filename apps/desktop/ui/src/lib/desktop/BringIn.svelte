<!-- The first time Stash opens: bringing in the music on the listener's phone. Until a phone's send is waiting, how to
     plug it in (and other ways to start). Then what the phone sent, cover by cover: its downloads, Liked songs, plays
     and the listener's own playlists coming already (tap one to leave it out), each playlist synced from a streaming
     service to add with +, and one switch for songs downloaded only (Stash then stays offline, as "My own music
     files" did). Bring them in takes the send: the library parts come in at once, the files after (the corner card
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
	import { firstPicks, flip, gb, lines, listLine, n, ownLists, syncedLists, takeBody, tally, type PhoneLibrary, type Picks, type SentList } from './bring-in';
	import type { Offer } from './phone-songs';

	/** `first`: the first time Stash opens (how it runs is set by what is brought in); else a later send, from the corner card. */
	let { first = true }: { first?: boolean } = $props();

	type Now = { state: string; phone: string | null; sent_at_ms: number | null; with_library?: boolean };

	/** Songs on their way, but how Stash runs could not be set: set again (and only that) with Try again. */
	let owed = $state<Mode | null>(null);
	let now = $state<Now | null>(null);
	let offer = $state<Offer | null>(null);
	let lib = $state<PhoneLibrary | null>(null);
	let picks = $state<Picks | null>(null);
	let q = $state('');
	let all = $state(false);
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
		offer = sent;
		lib = library && library.sent_at_ms === sent.sent_at_ms ? library : null;
		picks = firstPicks(sent, lib);
		// Stash offline for good (on the listener's own files): a song not downloaded could never play, so none comes.
		if (!first && ui.local) picks = { ...picks, onlyHere: true };
		q = '';
		all = false;
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
					// A new send; or this one's library, read only after its songs were offered (the phone answered late).
					const late = told.state === 'ready' && !!told.with_library && !lib && told.sent_at_ms === offer?.sent_at_ms;
					if (told.state === 'ready' && told.sent_at_ms && (told.sent_at_ms !== offer?.sent_at_ms || late)) await load();
					else if (told.state !== 'ready' && offer) {
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
	const shown = $derived(all || q.trim() ? found : found.slice(0, 14));
	const added = $derived(picks ? synced.filter((p) => picks!.lists.includes(p.id)).length : 0);
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
	{@const on = !!picks?.lists.includes(p.id)}
	<button type="button" class="bi-card" class:on aria-pressed={on} disabled={busy} onclick={() => toggle(p.id)}>
		<span class="tile bi-art">{@render cover(p.covers, p.name)}</span>
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

<section class="bi" aria-labelledby="bi-h">
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
						<span>·</span> <b>{n(lib.likes.songs)}</b> liked <span>·</span> <b>{n(lib.playlists.length)}</b> playlists <span>·</span> <b>{n(lib.plays)}</b> plays
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

		<div class="bi-sec"><div><h2>Coming with you</h2><p>{lib ? 'Your downloads, likes, plays and playlists. Tap one to leave it out.' : 'Tap it to leave it out.'}</p></div></div>
		<div class="bi-shelf">
			{#if fresh.length}
				<button type="button" class="bi-card" class:on={picks.downloads} aria-pressed={picks.downloads} disabled={busy} onclick={() => toggle('downloads')}>
					<span class="tile bi-art">{@render cover(albumArt.slice(0, 4), 'downloads')}</span>
					{@render badge(picks.downloads)}
					<b>All your downloads</b>
					<i>{n(fresh.length)} songs · {gb(fresh.reduce((sum, s) => sum + s.bytes, 0))}</i>
				</button>
			{/if}
			{#if lib && lib.likes.songs}
				<button type="button" class="bi-card" class:on={picks.likes} aria-pressed={picks.likes} disabled={busy} onclick={() => toggle('likes')}>
					<span class="tile bi-art"><span class="tile-liked"><Icon name="favorite" /></span></span>
					{@render badge(picks.likes)}
					<b>Liked songs</b>
					<i>{picks.onlyHere ? `${n(lib.likes.downloaded)} downloaded songs` : `${n(lib.likes.songs)} songs · ${n(lib.likes.downloaded)} downloaded`}</i>
				</button>
			{/if}
			{#if lib && lib.plays}
				<button type="button" class="bi-card" class:on={picks.plays} aria-pressed={picks.plays} disabled={busy} onclick={() => toggle('plays')}>
					<span class="tile bi-art"><span class="bi-stat"><b>{n(lib.plays)}</b><i>plays</i></span></span>
					{@render badge(picks.plays)}
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
					<p>{added ? `${added} of ${n(synced.length)} added. Their downloaded songs come in as files${picks.onlyHere ? '.' : ', the rest stream.'}` : `${n(synced.length)} playlists synced on your phone. Add the ones you want.`}</p>
				</div>
				<label class="bi-find"><Icon name="search" /><input id="bi-find" type="search" placeholder="Find a playlist" aria-label="Find a playlist" bind:value={q} /></label>
			</div>
			<div class="bi-grid">
				{#each shown as p (p.id)}{@render listCard(p)}{/each}
			</div>
			{#if !found.length}<p class="bi-none">No synced playlist called “{q.trim()}”.</p>{/if}
			{#if !all && !q.trim() && found.length > shown.length}
				<button type="button" class="bi-more" onclick={() => (all = true)}>Show all {n(found.length)}</button>
			{/if}
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
	.bi-more { display: block; margin: 12px auto 0; height: 38px; padding: 0 18px; border-radius: 999px; border: 1px solid var(--wp-line-2); color: var(--wp-text); font: 600 13.5px/1 var(--font-body); }

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
		.bi-more:hover { background: var(--wp-hover); }
		.bi-go:not(:disabled):hover { filter: brightness(1.1); }
		.bi-sw:not(:disabled):hover { color: var(--wp-text); }
	}
	@keyframes bi-drift { from { transform: rotate(-7deg) translateX(0); } to { transform: rotate(-7deg) translateX(-7%); } }
	@keyframes bi-breathe { 0%, 100% { opacity: .3; } 50% { opacity: .75; } }
	@keyframes bi-spin { to { transform: rotate(360deg); } }
	@keyframes bi-in { from { opacity: 0; translate: 0 6px; } }
	@media (prefers-reduced-motion: reduce) { .bi-wall, .bi-ghost, .bi-spin, .bi-take-in { animation: none; } }
</style>
