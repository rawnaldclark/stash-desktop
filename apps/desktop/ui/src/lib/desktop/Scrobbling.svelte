<!-- Settings › Scrobbling: the services the listener's plays go to (ListenBrainz, or a server that speaks its language,
     and Last.fm when this build has it), what each took and what waits for it, and one added by its token or, for
     Last.fm, by signing in on Last.fm's page. The core checks a token with the service before keeping it, and never
     gives it back. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { many } from './folders.svelte';
	import { finishLastfm, lastfmAvailable, startLastfm } from './lastfm';

	const LISTENBRAINZ = 'https://api.listenbrainz.org';
	const LASTFM = 'lastfm';
	/** How often Last.fm is asked whether the listener allowed Stash, and for how long. */
	const ASK_EVERY = 2_000;
	const ASK_FOR = 5 * 60_000;

	let targets = $state<core.ScrobbleTarget[] | null>(null);
	/** The form that adds a service: open on its own when there is none yet. */
	let adding = $state(false);
	let token = $state('');
	let elsewhere = $state(false);
	let url = $state('');
	let busy = $state(false);
	let wrong = $state('');
	/** A refused service's token, given again, by service. */
	let again = $state<Record<number, string>>({});
	let againWrong = $state<Record<number, string>>({});
	/** Last.fm: this build has it; a sign-in is being opened; the listener is being waited for on Last.fm's page. */
	let lastfm = $state(false);
	let opening = $state(false);
	let waiting = $state(false);
	let lastfmWrong = $state('');
	/** Each sign-in's own number: an answer to one that was cancelled or begun again is not heard. */
	let round = 0;
	let asking: ReturnType<typeof setTimeout> | undefined;
	/** Last.fm is offered while the listener has none (one signed out is connected again in its own row). */
	const offerLastfm = $derived(lastfm && !!targets && !targets.some((t) => t.kind === LASTFM));

	const name = (t: core.ScrobbleTarget) => {
		if (t.kind === LASTFM) return 'Last.fm';
		if (t.url === LISTENBRAINZ) return 'ListenBrainz';
		try {
			return new URL(t.url).host;
		} catch {
			return t.url;
		}
	};
	const ago = (ms: number) => {
		const minutes = Math.floor((Date.now() - ms) / 60_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes} min ago`;
		const hours = Math.floor(minutes / 60);
		return hours < 48 ? `${hours} h ago` : `${Math.floor(hours / 24)} days ago`;
	};
	function line(t: core.ScrobbleTarget): string {
		const sent = t.sent ? `${many(t.sent, 'play', 'plays')} sent${t.sent_at_ms ? ` · last ${ago(t.sent_at_ms)}` : ''}` : 'Nothing sent yet';
		return [t.user ?? '', sent].filter(Boolean).join(' · ');
	}

	async function read() {
		try {
			targets = (await core.scrobbling()).targets;
		} catch {
			/* the core is not answering: the list stays as it was */
		}
	}
	async function act(run: () => Promise<{ targets: core.ScrobbleTarget[] }>) {
		try {
			targets = (await run()).targets;
		} catch {
			await read();
		}
	}
	async function add(e: SubmitEvent) {
		e.preventDefault();
		if (busy || !token.trim()) return;
		busy = true;
		wrong = '';
		try {
			targets = (await core.addScrobbling(token.trim(), elsewhere ? url.trim() : undefined)).targets;
			token = '';
			url = '';
			elsewhere = false;
			adding = false;
		} catch (err) {
			wrong = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
	async function giveAgain(e: SubmitEvent, t: core.ScrobbleTarget) {
		e.preventDefault();
		const given = (again[t.id] ?? '').trim();
		if (!given) return;
		againWrong[t.id] = '';
		try {
			targets = (await core.addScrobbling(given, t.url)).targets;
			again[t.id] = '';
		} catch (err) {
			againWrong[t.id] = err instanceof Error ? err.message : String(err);
		}
	}

	/** Stops waiting for Last.fm (cancelled, done, or given up). */
	function stopWaiting() {
		round++;
		waiting = false;
		clearTimeout(asking);
		asking = undefined;
	}
	/** Opens Last.fm's page, where the listener allows Stash, then asks every 2 s whether they did, for up to 5 min. */
	async function connectLastfm() {
		if (opening || waiting) return;
		stopWaiting();
		const mine = round;
		opening = true;
		lastfmWrong = '';
		try {
			const page = await startLastfm();
			// In a browser (no shell to open it): the page opens what the core answered, only a Last.fm page.
			if (!page.opened && page.url.startsWith('https://www.last.fm/')) window.open(page.url, '_blank', 'noopener');
		} catch (err) {
			lastfmWrong = err instanceof Error ? err.message : String(err);
			return;
		} finally {
			opening = false;
		}
		if (mine !== round) return;
		waiting = true;
		const until = Date.now() + ASK_FOR;
		const ask = async () => {
			if (mine !== round) return;
			try {
				const said = await finishLastfm();
				if (mine !== round) return;
				if ('targets' in said) {
					targets = said.targets;
					adding = false;
					stopWaiting();
					return;
				}
			} catch (err) {
				if (mine !== round) return;
				// Over (or never begun): said at once. Anything else (no answer for a moment): asked again.
				if (err instanceof core.Refused && err.status !== 502) {
					lastfmWrong = err.message;
					stopWaiting();
					return;
				}
			}
			if (Date.now() >= until) {
				lastfmWrong = 'Stash was not allowed in time. Connect again.';
				stopWaiting();
				return;
			}
			asking = setTimeout(() => void ask(), ASK_EVERY);
		};
		asking = setTimeout(() => void ask(), ASK_EVERY);
	}

	onMount(() => {
		void read();
		lastfmAvailable().then(
			(said) => (lastfm = said.available),
			() => {
				/* the core is not answering: Last.fm is not offered */
			}
		);
		// What was sent and what waits changes as songs play.
		const timer = setInterval(() => void read(), 10_000);
		const back = () => void read();
		window.addEventListener('focus', back);
		return () => {
			clearInterval(timer);
			window.removeEventListener('focus', back);
			stopWaiting();
		};
	});
</script>

<!-- Connecting Last.fm: the button, or the wait for the listener on Last.fm's page with Cancel. -->
{#snippet connect(label: string)}
	<div class="chips">
		{#if waiting}
			<span class="set-sub" role="status">Waiting for you to allow Stash on Last.fm…</span>
			<button type="button" class="btn ghost" onclick={stopWaiting}>Cancel</button>
		{:else}
			<button type="button" class="btn" disabled={opening} onclick={connectLastfm}>{opening ? 'Opening Last.fm…' : label}</button>
		{/if}
	</div>
	{#if lastfmWrong}<p class="set-sub set-trouble" role="alert">{lastfmWrong}</p>{/if}
{/snippet}

<!-- One card: a row for each service, then the services that can be added (or the button that shows them). -->
<div class="set-card">
	{#if targets?.length}
		<ol class="src-list">
			{#each targets as t (t.id)}
				<li class="src sc-row" class:off={!t.enabled}>
					<span class="tr-t">
						<b>{name(t)}</b>
						<i>{line(t)}</i>
						{#if t.waiting}<i class="src-use">{many(t.waiting, 'play', 'plays')} waiting</i>{/if}
						{#if t.dropped}<i>{many(t.dropped, 'play', 'plays')} rejected</i>{/if}
						{#if t.refused}
							<i class="src-problem">{t.kind === LASTFM ? 'Signed out of Last.fm. Connect again.' : 'Token rejected. Enter it again.'}</i>
						{:else if t.trouble}
							<i class="src-problem">Problem {ago(t.trouble.at_ms)}: {t.trouble.message}</i>
						{/if}
					</span>
					<span class="src-acts">
						{#if t.trouble && !t.refused && t.enabled}<button type="button" class="btn ghost" onclick={() => act(() => core.scrobbleAgain(t.id))}>Try again</button>{/if}
						<button type="button" class="btn ghost" aria-label="Remove {name(t)}" onclick={() => act(() => core.removeScrobbling(t.id))}>Remove</button>
						<button type="button" class="sw" role="switch" aria-checked={t.enabled} aria-label={name(t)} title={t.enabled ? 'On' : 'Off'} onclick={() => act(() => core.setScrobblingEnabled(t.id, !t.enabled))}><i></i></button>
					</span>
				</li>
				{#if t.refused && t.kind === LASTFM}
					{#if lastfm}<li class="sc-again">{@render connect('Connect again')}</li>{/if}
				{:else if t.refused}
					<li class="sc-again">
						<form class="dev-rename" onsubmit={(e) => giveAgain(e, t)}>
							<input class="dev-input" type="password" placeholder="Token" bind:value={again[t.id]} aria-label="{name(t)} token" autocomplete="off" spellcheck="false" />
							<button type="submit" class="btn" disabled={!(again[t.id] ?? '').trim()}>Save</button>
						</form>
						{#if againWrong[t.id]}<p class="set-sub set-trouble" role="alert">{againWrong[t.id]}</p>{/if}
					</li>
				{/if}
			{/each}
		</ol>
	{/if}
	{#if targets && offerLastfm && (adding || !targets.length || waiting)}
		<div class="set-row set-col sc-add">
			<div class="set-lab">
				<span class="set-name">Last.fm</span>
				<span class="set-sub">Share what you play on your Last.fm profile.</span>
			</div>
			{@render connect('Connect Last.fm')}
		</div>
	{/if}
	{#if targets && (adding || !targets.length)}
		<form class="set-row set-col sc-add" onsubmit={add}>
			{#if !targets.length || offerLastfm}
				<div class="set-lab">
					<span class="set-name">ListenBrainz</span>
					<span class="set-sub">Share what you play. Listens there are public.</span>
				</div>
			{/if}
			<label class="sc-field">
				<span class="set-sub">{elsewhere ? 'Your server’s token' : 'User token, from listenbrainz.org › Settings'}</span>
				<input class="dev-input" type="password" bind:value={token} placeholder="Token" autocomplete="off" spellcheck="false" />
			</label>
			<label class="sc-check"><input type="checkbox" bind:checked={elsewhere} /><span class="set-sub">Use my own server</span></label>
			{#if elsewhere}
				<label class="sc-field">
					<span class="set-sub">Server address</span>
					<input class="dev-input" type="url" bind:value={url} placeholder="https://…/apis/listenbrainz" autocomplete="off" spellcheck="false" />
				</label>
			{/if}
			<div class="chips">
				<button type="submit" class="btn" disabled={busy || !token.trim() || (elsewhere && !url.trim())}>{busy ? 'Checking…' : 'Connect'}</button>
				{#if targets.length}<button type="button" class="btn ghost" onclick={() => ((adding = false), (wrong = ''))}>Cancel</button>{/if}
			</div>
			{#if wrong}<p class="set-sub set-trouble" role="alert">{wrong}</p>{/if}
		</form>
	{:else if targets?.length}
		<div class="set-row sc-more"><button type="button" class="btn ghost" onclick={() => (adding = true)}>Add a service</button></div>
	{/if}
</div>

<style>
	/* A service's row: its words, then its buttons and switch (no number: services are not asked in turn). */
	.sc-row { grid-template-columns: minmax(0, 1fr) auto; padding-left: 20px; }
	.sc-row .tr-t i { white-space: normal; overflow-wrap: anywhere; }
	.sc-add { gap: 14px; }
	.sc-field { display: grid; gap: 6px; }
	.sc-field .dev-input { flex: none; width: min(100%, 460px); }
	.sc-check { display: flex; align-items: center; gap: 8px; }
	.sc-again { list-style: none; display: grid; gap: 6px; padding: 0 16px 14px 20px; }
	.sc-more { justify-content: flex-start; }
</style>
