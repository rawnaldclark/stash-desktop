<!-- Settings › Scrobbling: the services the listener's plays go to (ListenBrainz, or a server that speaks its language),
     what each took and what waits for it, and one added by its token. The core checks a token with the service before
     keeping it, and never gives it back. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { many } from './folders.svelte';

	const LISTENBRAINZ = 'https://api.listenbrainz.org';

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

	const name = (t: core.ScrobbleTarget) => {
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
		const sent = t.sent ? `${many(t.sent, 'play', 'plays')} sent${t.sent_at_ms ? `, the last ${ago(t.sent_at_ms)}` : ''}` : 'Nothing sent yet';
		return [t.user ? `As ${t.user}` : '', sent].filter(Boolean).join(' · ');
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

	onMount(() => {
		void read();
		// What was sent and what waits changes as songs play.
		const timer = setInterval(() => void read(), 10_000);
		const back = () => void read();
		window.addEventListener('focus', back);
		return () => {
			clearInterval(timer);
			window.removeEventListener('focus', back);
		};
	});
</script>

<div class="set-card">
	<div class="set-row set-col">
		{#if targets?.length}
			<ol class="src-list">
				{#each targets as t (t.id)}
					<li class="src sc-row" class:off={!t.enabled}>
						<span class="tr-t">
							<b>{name(t)}</b>
							<i>{line(t)}</i>
							{#if !t.enabled}<i>Off: what you play now is not kept for it.</i>{/if}
							{#if t.waiting}<i class="src-use">{many(t.waiting, 'play', 'plays')} waiting to be sent</i>{/if}
							{#if t.dropped}<i>{many(t.dropped, 'play', 'plays')} it would not take, let go</i>{/if}
							{#if t.refused}
								<i class="src-problem">It refused the token. Paste it again to go on; what you play waits meanwhile.</i>
							{:else if t.trouble}
								<i class="src-problem">Trouble {ago(t.trouble.at_ms)}: {t.trouble.message}.</i>
							{/if}
						</span>
						<span class="src-acts">
							<button type="button" class="sw" role="switch" aria-checked={t.enabled} aria-label="{name(t)}: on" title={t.enabled ? 'On' : 'Off'} onclick={() => act(() => core.setScrobblingEnabled(t.id, !t.enabled))}><i></i></button>
							{#if t.trouble && !t.refused && t.enabled}<button type="button" class="btn ghost" onclick={() => act(() => core.scrobbleAgain(t.id))}>Try again</button>{/if}
							<button type="button" class="btn ghost" aria-label="Remove {name(t)}" onclick={() => act(() => core.removeScrobbling(t.id))}>Remove</button>
						</span>
					</li>
					{#if t.refused}
						<li class="sc-again">
							<form class="dev-rename" onsubmit={(e) => giveAgain(e, t)}>
								<input class="dev-input" type="password" placeholder="The token again" bind:value={again[t.id]} aria-label="{name(t)}: the token again" autocomplete="off" spellcheck="false" />
								<button type="submit" class="btn" disabled={!(again[t.id] ?? '').trim()}>Give</button>
							</form>
							{#if againWrong[t.id]}<p class="set-sub set-trouble" role="alert">{againWrong[t.id]}.</p>{/if}
						</li>
					{/if}
				{/each}
			</ol>
		{:else if targets}
			<p class="set-p">
				Stash can send what you play to ListenBrainz, a free and open record of listening that also makes recommendations; your listens are public there. A song is sent once it counts as played (half of it, or four minutes), and what you play with no connection waits here and goes later.
			</p>
		{/if}
		{#if targets && (adding || !targets.length)}
			<form class="sc-add" onsubmit={add}>
				<label class="sc-field">
					<span class="set-sub">Your user token, from listenbrainz.org › Settings{elsewhere ? ' (or the one your server gives)' : ''}</span>
					<input class="dev-input" type="password" bind:value={token} placeholder="Token" autocomplete="off" spellcheck="false" />
				</label>
				<label class="sc-check"><input type="checkbox" bind:checked={elsewhere} /><span class="set-sub">Another server that speaks ListenBrainz’s language (Maloja, Koito and others you run yourself)</span></label>
				{#if elsewhere}
					<label class="sc-field">
						<span class="set-sub">Its address</span>
						<input class="dev-input" type="url" bind:value={url} placeholder="https://…/apis/listenbrainz" autocomplete="off" spellcheck="false" />
					</label>
				{/if}
				<div class="chips">
					<button type="submit" class="btn" disabled={busy || !token.trim() || (elsewhere && !url.trim())}>{busy ? 'Checking…' : 'Connect'}</button>
					{#if targets.length}<button type="button" class="btn ghost" onclick={() => ((adding = false), (wrong = ''))}>Cancel</button>{/if}
				</div>
				{#if wrong}<p class="set-sub set-trouble" role="alert">{wrong}.</p>{/if}
			</form>
		{:else if targets?.length}
			<div class="chips"><button type="button" class="btn ghost" onclick={() => (adding = true)}>Add another service</button></div>
		{/if}
	</div>
</div>

<style>
	/* A service's row: its words, then its switch and buttons (no number: services are not asked in turn). */
	.sc-row { grid-template-columns: minmax(0, 1fr) auto; }
	.sc-row .tr-t i { white-space: normal; overflow-wrap: anywhere; }
	.sc-add { display: grid; gap: 12px; }
	.sc-field { display: grid; gap: 6px; }
	.sc-field .dev-input { flex: none; width: min(100%, 460px); }
	.sc-check { display: flex; align-items: center; gap: 8px; }
	.sc-again { list-style: none; display: grid; gap: 6px; margin: -4px 0 6px; }
</style>
