<!-- Settings › YouTube Music: the account the listener signed in with, as in Stash for Android. Signing in opens Google's
     own sign-in in a window of its own; cookies copied from a browser work too. -->
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

<div class="set-card">
	<div class="set-row set-col">
		{#if !T.on}
			<p class="set-p">YouTube Music is switched off (Add-ons). Switch it on to sign in.</p>
		{:else if a?.signed_in}
			<div class="yt-who">
				{#if who?.photo}<img class="yt-photo" src={artUrl(who.photo)} alt="" />{/if}
				<span class="tr-t">
					<b>{who?.name ?? 'Signed in'}</b>
					{#if who?.handle}<i>{who.handle}</i>{/if}
					{#if a.spent}<i class="src-problem">YouTube no longer takes this sign-in (you may have signed out on the web). Sign in again to see your library.</i>{/if}
				</span>
				<span class="src-acts">
					{#if a.spent && a.window}<button type="button" class="btn" disabled={busy || ui.offline || a.signing} onclick={signInWindow}>Sign in again</button>{/if}
					<button type="button" class="btn ghost" disabled={busy} onclick={() => act(core.tubeSignOut)}>Sign out</button>
				</span>
			</div>
			<p class="set-sub">Your playlists, liked songs, albums and artists are under Library › YouTube Music, and Home shows the shelves YouTube Music makes for you. Searches are still sent signed out, so they stay out of your YouTube history.</p>
		{:else if a}
			<p class="set-p">Sign in to bring in your YouTube Music account: your playlists and liked songs, the albums and artists you saved, and the home page YouTube Music makes for you. Searching and playing work without it.</p>
			{#if a.signing}
				<p class="set-sub" role="status">Finish signing in in the window that opened. It closes by itself when you are in.</p>
			{:else}
				<div class="chips">
					{#if a.window}<button type="button" class="btn" disabled={busy || ui.offline} onclick={signInWindow}>Sign in with Google</button>{/if}
					<button type="button" class="btn ghost" aria-expanded={pasting} onclick={() => (pasting = !pasting)}>Paste cookies instead</button>
				</div>
			{/if}
			{#if a.trouble}<p class="set-sub set-trouble" role="alert">{a.trouble}</p>{/if}
		{/if}
		{#if pasting && a && !a.signed_in}
			<form class="yt-paste" onsubmit={paste}>
				<span class="set-sub">Sign in at music.youtube.com in your browser, open its developer tools (F12) › Network, click any request to music.youtube.com and copy the value of its <b>Cookie</b> header. A cookies.txt file’s text works too.</span>
				<textarea class="dev-input yt-cookies" bind:value={cookies} rows="4" placeholder="SAPISID=…; SID=…; …" aria-label="Your YouTube cookies" autocomplete="off" spellcheck="false"></textarea>
				<div class="chips">
					<button type="submit" class="btn" disabled={busy || ui.offline || !cookies.trim()}>{busy ? 'Checking…' : 'Sign in'}</button>
					<button type="button" class="btn ghost" onclick={() => ((pasting = false), (cookies = ''))}>Cancel</button>
				</div>
			</form>
		{/if}
		{#if wrong}<p class="set-sub set-trouble" role="alert">{wrong}</p>{/if}
		{#if T.on}
			<p class="set-sub">Your sign-in is kept on this computer, in Stash’s library, and is sent only to YouTube. Signing out deletes it here; to end the session itself, sign out of YouTube on the web.</p>
		{/if}
	</div>
</div>

<style>
	.yt-who { display: flex; align-items: center; gap: 12px; }
	.yt-who .tr-t { flex: 1; min-width: 0; }
	.yt-photo { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; flex: none; }
	.yt-paste { display: flex; flex-direction: column; gap: 8px; }
	.yt-cookies { flex: none; height: auto; min-height: 84px; padding: 8px 10px; font: 400 12px/1.4 var(--font-mono, monospace); resize: vertical; }
</style>
