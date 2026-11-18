<!-- Settings › YouTube Music: the account the listener signed in with, as in Stash for Android. Signing in opens Google's
     own sign-in in a window of its own; cookies copied from a browser work too. Its rows sit in the page's card. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { artUrl } from '../core';
	import { ui } from '../ui/state.svelte';
	import { T } from './youtube.svelte';

	let pasting = $state(false);
	let cookies = $state('');
	let busy = $state(false);
	let wrong = $state('');

	const a = $derived(T.account);
	const who = $derived(a?.account ?? null);

	onMount(() => void T.refreshAccount());
	// While Google's window is open, look every second and a half for the sign-in to land.
	const signing = $derived(!!a?.signing);
	$effect(() => {
		if (!signing) return;
		const look = setInterval(() => void T.refreshAccount(), 1500);
		return () => clearInterval(look);
	});

	async function act(ask: () => Promise<core.TubeAccount>) {
		busy = true;
		wrong = '';
		try {
			T.took(await ask());
			return true;
		} catch (e) {
			wrong = core.said(e);
			return false;
		} finally {
			busy = false;
		}
	}
	const signInWindow = () => act(core.tubeSignInWindow);
	async function paste(e: SubmitEvent) {
		e.preventDefault();
		if (await act(() => core.tubeSignInWith(cookies))) {
			cookies = '';
			pasting = false;
		}
	}
</script>

{#if !T.on}
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name">Account</span>
			<span class="set-sub">Turn on YouTube Music in Add-ons first</span>
		</div>
	</div>
{:else if a?.signed_in}
	<div class="set-row">
		<div class="yt-who">
			{#if who?.photo}<img class="yt-photo" src={artUrl(who.photo)} alt="" />{/if}
			<span class="tr-t">
				<b>{who?.name ?? 'Signed in'}</b>
				{#if who?.handle}<i>{who.handle}</i>{/if}
				{#if a.spent}<i class="src-problem">YouTube signed you out. Sign in again.</i>{/if}
			</span>
		</div>
		<div class="set-ctl">
			{#if a.spent && a.window}<button type="button" class="btn" disabled={busy || ui.offline || a.signing} onclick={signInWindow}>Sign in again</button>{/if}
			<button type="button" class="btn ghost" disabled={busy} onclick={() => act(core.tubeSignOut)}>Sign out</button>
		</div>
	</div>
{:else if a}
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name">Account</span>
			{#if a.signing}
				<span class="set-sub" role="status">Finish signing in in the new window</span>
			{:else}
				<span class="set-sub">Brings in your playlists and likes</span>
			{/if}
			{#if a.trouble}<span class="set-sub set-trouble" role="alert">{a.trouble}</span>{/if}
		</div>
		{#if !a.signing}
			<div class="set-ctl">
				<button type="button" class="btn ghost" aria-expanded={pasting} onclick={() => (pasting = !pasting)}>Use cookies</button>
				{#if a.window}<button type="button" class="btn" disabled={busy || ui.offline} onclick={signInWindow}>Sign in</button>{/if}
			</div>
		{/if}
	</div>
{/if}
{#if pasting && a && !a.signed_in}
	<form class="set-row set-col yt-paste" onsubmit={paste}>
		<label class="set-sub" for="yt-cookies">Paste the Cookie header of a music.youtube.com request (F12 › Network), or a cookies.txt file</label>
		<textarea id="yt-cookies" class="dev-input yt-cookies" bind:value={cookies} rows="4" placeholder="SAPISID=…; SID=…; …" autocomplete="off" spellcheck="false"></textarea>
		<div class="chips">
			<button type="submit" class="btn" disabled={busy || ui.offline || !cookies.trim()}>{busy ? 'Checking…' : 'Sign in'}</button>
			<button type="button" class="btn ghost" onclick={() => ((pasting = false), (cookies = ''))}>Cancel</button>
		</div>
	</form>
{/if}
{#if wrong}<div class="set-row"><p class="set-sub set-trouble" role="alert">{wrong}</p></div>{/if}

<style>
	.yt-who { display: flex; align-items: center; gap: 12px; flex: 1 1 200px; min-width: 0; }
	.yt-who .tr-t { flex: 1; min-width: 0; }
	.yt-who .tr-t i { white-space: normal; }
	.yt-photo { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; flex: none; }
	.yt-paste .set-sub { max-width: none; }
	.yt-cookies { flex: none; height: auto; min-height: 84px; padding: 8px 10px; font: 400 12px/1.4 var(--font-mono, monospace); resize: vertical; }
</style>
