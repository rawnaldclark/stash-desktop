<!-- Settings › Sources: where music beyond the listener's own files comes from. Add-ons (each added by its link, or Stash's
     own Tidal from Stash's offer; the order is the listener's), Stash's Tidal once added (its daily allowance), YouTube
     Music (the account, restricted songs), and the Mornye extensions. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { sources } from '../../lib/app';
	import * as core from '../../lib/core';
	import { trouble } from '../../lib/desktop/folders.svelte';
	import MornyeExtensions from '../../lib/desktop/MornyeExtensions.svelte';
	import StashTidal from '../../lib/desktop/StashTidal.svelte';
	import TubeAccount from '../../lib/desktop/TubeAccount.svelte';
	import TubeFallback from '../../lib/desktop/TubeFallback.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import { L, ui } from '../../lib/ui/state.svelte';
	import type { SourceStatus } from '../../lib/api';
	import { prefs } from './prefs.svelte';

	let link = $state('');
	let adding = $state(false);
	let addonTrouble = $state<string | null>(null);

	async function addonChange(run: () => Promise<void>) {
		addonTrouble = null;
		try {
			await run();
		} catch (e) {
			addonTrouble = trouble(e);
		}
	}
	async function add(e: SubmitEvent) {
		e.preventDefault();
		const wanted = link.trim();
		if (wanted === '' || adding) return;
		adding = true;
		addonTrouble = null;
		try {
			await sources.add(wanted);
			link = '';
		} catch (x) {
			addonTrouble = x instanceof core.Refused ? `Couldn’t add that link: ${trouble(x)}` : trouble(x);
		} finally {
			adding = false;
		}
	}
	/** How Stash's own Tidal stands: offered (when this build has it) until it is added, then its allowance. */
	let tidal = $state<core.TidalStatus | null>(null);
	/** Until this time, it is read often: it was just added, or a claim just went through. */
	let settleUntil = 0;
	async function askTidal() {
		try {
			tidal = await core.get<core.TidalStatus>('/api/tidal');
			// The check's window is open: asked often until it closes. Passing it adds Tidal in the core.
			if (tidal.checking) settleUntil = Math.max(settleUntil, Date.now() + 3_000);
			if (tidal.added && !tidalAdded) await sources.reload();
		} catch {
			/* the core is not answering */
		}
	}
	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const loop = async () => {
			await askTidal();
			if (alive) timer = setTimeout(loop, Date.now() < settleUntil ? 1500 : 30_000);
		};
		void loop();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});
	async function tidalChanged() {
		settleUntil = Date.now() + 15_000;
		await askTidal();
	}
	const tidalAdded = $derived(L.sources.some((a) => a.id === core.STASH_TIDAL));
	let addingTidal = $state(false);
	async function addTidal() {
		if (addingTidal) return;
		addingTidal = true;
		addonTrouble = null;
		try {
			await sources.addTidal();
			await tidalChanged();
		} catch (x) {
			addonTrouble = `Couldn’t add Tidal: ${trouble(x)}`;
		} finally {
			addingTidal = false;
		}
	}
	/** A new version of an add-on, shown until it is taken or put off. */
	let offer = $state<{ id: string; version: string; changes: core.AddonChange[]; token: string | null } | null>(null);
	/** The add-on being asked for a new version, or being updated. */
	let checking = $state<string | null>(null);
	/** What the last check said, when there was nothing to take. */
	let addonNote = $state<string | null>(null);
	async function check(a: SourceStatus) {
		if (checking) return;
		checking = a.id;
		addonTrouble = null;
		addonNote = null;
		offer = null;
		try {
			const found = await sources.check(a.id);
			if (found.changes.length) offer = { id: a.id, ...found };
			else addonNote = `${a.name} is up to date`;
		} catch (x) {
			addonTrouble = `Couldn’t check ${a.name}: ${trouble(x)}`;
		} finally {
			checking = null;
		}
	}
	async function takeOffer() {
		if (!offer || !offer.token || checking) return;
		const { id, token } = offer;
		checking = id;
		addonTrouble = null;
		try {
			await sources.update(id, token);
			offer = null;
		} catch (x) {
			offer = null;
			addonTrouble = trouble(x);
		} finally {
			checking = null;
		}
	}
	function move(index: number, by: number) {
		// The listener's own add-ons: what is built in is asked after them, whatever their order.
		const ids = L.sources.filter((a) => !a.builtIn).map((a) => a.id);
		const [id] = ids.splice(index, 1);
		ids.splice(index + by, 0, id);
		void addonChange(() => sources.order(ids));
	}
	/** Said only when an add-on does one of the two. */
	const partial = (a: SourceStatus) => {
		const search = a.resources.includes('search');
		const play = a.resources.includes('stream');
		return search === play ? '' : search ? 'Search only' : 'Playback only';
	};
	const whole = (n: number) => n.toLocaleString();
	const ago = (ms: number) => {
		const minutes = Math.floor((Date.now() - ms) / 60_000);
		return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : `${Math.floor(minutes / 60)} h ago`;
	};
	function useLine(id: string): string {
		const u = prefs.used[id];
		if (!u) return '';
		const parts = [u.streams ? `${whole(u.streams)} played` : '', u.downloads ? `${whole(u.downloads)} saved` : ''].filter(Boolean);
		const today = parts.length ? `Today: ${parts.join(', ')}` : 'Not used today';
		return u.found_at_ms ? `${today} · last song ${ago(u.found_at_ms)}` : today;
	}
	/** Its last trouble, when that came after the last song it gave. */
	const troubleOf = (id: string) => {
		const u = prefs.used[id];
		return u?.trouble && u.trouble.at_ms > (u.found_at_ms ?? 0) ? u.trouble : null;
	};
	/** Where it is, when that is nearer than the internet: it may send Stash only as near as that. */
	const where = (a: SourceStatus) => (a.reach === 'home' ? 'on your network' : a.reach === 'here' ? 'on this computer' : '');
	const detail = (a: SourceStatus) =>
		(a.builtIn ? [a.id.startsWith('mornye.') ? 'Mornye extension' : 'Built in', partial(a)] : a.byStash ? ['By Stash', partial(a)] : [a.version ? `v${a.version}` : '', a.host ?? '', where(a), partial(a)])
			.filter((p) => p !== '')
			.join(' · ');
	/** How many of the sources are the listener's own add-ons (they come first in the list). */
	const own = $derived(L.sources.filter((a) => !a.builtIn).length);
</script>

<section class="set-part" id="addons" aria-labelledby="h-addons">
	<div class="set-cap">
		<h3 class="set-h" id="h-addons">Add-ons</h3>
		{#if own > 1}<span class="set-sub">Searched in this order</span>{/if}
	</div>
	<div class="set-card">
		{#if L.sources.length}
			<ol class="src-list">
				{#each L.sources as a, i (a.id)}
					<li class="src" class:off={!a.enabled}>
						<span class="src-n" aria-hidden="true">{i + 1}</span>
						<span class="tr-t">
							<b title={a.description ?? undefined}>{a.name}</b>
							<i>{detail(a)}</i>
							{#if useLine(a.id)}<i class="src-use">{useLine(a.id)}</i>{/if}
							{#if troubleOf(a.id)}<i class="src-problem">Problem {ago(troubleOf(a.id)!.at_ms)}: {troubleOf(a.id)!.message}</i>{/if}
							{#if a.problem}<i class="src-problem">Can’t be used: {a.problem}</i>{/if}
						</span>
						<span class="src-acts">
							{#if !a.builtIn}
								<button type="button" class="act-ib" disabled={i === 0} aria-label="Move {a.name} up" title="Move up" onclick={() => move(i, -1)}><Icon name="arrow_upward" /></button>
								<button type="button" class="act-ib" disabled={i >= own - 1} aria-label="Move {a.name} down" title="Move down" onclick={() => move(i, 1)}><Icon name="arrow_downward" /></button>
								{#if !a.byStash}<button type="button" class="btn ghost" aria-label="Check {a.name} for updates" title="Check for updates" disabled={checking !== null || ui.offline} onclick={() => check(a)}>{checking === a.id && offer?.id !== a.id ? 'Checking…' : 'Check'}</button>{/if}
								<button type="button" class="btn ghost" aria-label="Remove {a.name}" onclick={() => { if (offer?.id === a.id) offer = null; void addonChange(() => sources.remove(a.id)); }}>Remove</button>
							{/if}
							<button type="button" class="sw" role="switch" aria-checked={a.enabled} aria-label={a.name} title={a.enabled ? 'On' : 'Off'} onclick={() => addonChange(() => sources.setEnabled(a.id, !a.enabled))}><i></i></button>
						</span>
						{#if offer?.id === a.id}
							<div class="src-offer" role="group" aria-labelledby="offer-h">
								<b id="offer-h">{offer.version !== a.version ? `Version ${offer.version}` : `${a.name} has changed`}</b>
								<ul>
									{#each offer.changes as c (c.words)}<li class:gives={c.more}>{c.words}</li>{/each}
								</ul>
								{#if offer.changes.some((c) => c.more)}<p class="src-problem">This update asks for more access</p>{/if}
								<div class="src-offer-acts">
									<button type="button" class="btn" disabled={checking !== null} onclick={takeOffer}>{checking === a.id ? 'Updating…' : 'Update'}</button>
									<button type="button" class="btn ghost" onclick={() => (offer = null)}>Not now</button>
								</div>
							</div>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}
		{#if tidal?.available && !tidalAdded}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">Tidal, by Stash</span>
					<span class="set-sub">Lossless and Hi-Res. Free with a daily allowance, unlimited for supporters.</span>
				</div>
				<button type="button" class="btn" disabled={addingTidal || tidal.checking || ui.offline} onclick={addTidal}>{addingTidal || tidal.checking ? 'Adding…' : 'Add'}</button>
			</div>
			{#if tidal.checking}
				<p class="set-sub" role="status">Finish the quick check in the window that opened. It's asked once on this computer.</p>
			{:else if tidal.check_trouble}
				<p class="set-sub set-trouble" role="alert">{tidal.check_trouble}</p>
			{/if}
		{/if}
		<div class="set-row set-col">
			<form class="dev-rename" onsubmit={add}>
				<input class="dev-input" type="url" placeholder="Paste an add-on link" bind:value={link} aria-label="Add-on link" autocomplete="off" spellcheck="false" />
				<button type="submit" class="btn" disabled={adding || link.trim() === ''}>{adding ? 'Adding…' : 'Add'}</button>
			</form>
			{#if addonTrouble}<p class="set-sub set-trouble" role="alert">{addonTrouble}</p>{/if}
			{#if addonNote}<p class="set-sub" role="status">{addonNote}</p>{/if}
		</div>
	</div>
</section>

{#if tidal?.available && tidalAdded}<StashTidal t={tidal} onchange={tidalChanged} />{/if}

<section class="set-part" id="youtube" aria-labelledby="h-youtube">
	<h3 class="set-h" id="h-youtube">YouTube Music</h3>
	<div class="set-card">
		<TubeAccount />
		<TubeFallback />
	</div>
</section>

<section class="set-part" id="mornye" aria-labelledby="h-mornye">
	<h3 class="set-h" id="h-mornye">Mornye extensions</h3>
	<MornyeExtensions />
</section>
