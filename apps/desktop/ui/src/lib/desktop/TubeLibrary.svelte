<!-- The library of the YouTube Music account the listener signed in with: their playlists (Liked music first), the
     albums they saved, and the artists of their songs. Each opens its page; nothing is copied into Stash's library. -->
<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { tubeCard } from '../app';
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { ui } from '../ui/state.svelte';
	import { T } from './youtube.svelte';

	const PARTS: [core.TubeSaved, string][] = [
		['playlists', 'Playlists'],
		['albums', 'Albums'],
		['artists', 'Artists']
	];
	type Part = { cards: ReturnType<typeof tubeCard>[] | null; trouble: string | null };
	let parts = $state<Record<core.TubeSaved, Part>>({
		playlists: { cards: null, trouble: null },
		albums: { cards: null, trouble: null },
		artists: { cards: null, trouble: null }
	});

	async function load(kind: core.TubeSaved) {
		parts[kind] = { cards: parts[kind].cards, trouble: null };
		try {
			parts[kind] = { cards: (await core.tubeLibrary(kind)).map(tubeCard).filter((c) => c.href), trouble: null };
		} catch (e) {
			parts[kind] = { cards: null, trouble: core.said(e) };
		}
	}

	onMount(() => void T.refreshAccount());
	// Read once each time the listener is signed in and online (a boolean: asking for the account again is no change).
	const ready = $derived(T.signedIn && !ui.offline);
	$effect(() => {
		if (ready) untrack(() => PARTS.forEach(([kind]) => void load(kind)));
	});
	const initials = (name: string) => name.slice(0, 2).toUpperCase();
</script>

{#if !T.on}
	<p class="s-hint">YouTube Music is switched off. Switch it on in <a class="lnk" href="/settings#addons">Settings</a>.</p>
{:else if T.account && !T.signedIn}
	<p class="s-hint">
		{T.account.spent ? 'YouTube no longer takes your sign-in.' : 'Sign in to YouTube Music to see your playlists, liked songs, albums and artists here.'}
		<a class="lnk" href="/settings#youtube">{T.account.spent ? 'Sign in again' : 'Sign in'}</a>
	</p>
{:else if ui.offline}
	<p class="s-hint">Stash is offline: your YouTube Music library shows when it is online.</p>
{:else if T.signedIn}
	{#each PARTS as [kind, label] (kind)}
		{@const part = parts[kind]}
		<h2 class="vh2 tube-part">{label}</h2>
		{#if part.trouble}
			<p class="s-hint" role="alert">{part.trouble} <button type="button" class="lnk" onclick={() => void load(kind)}>Try again</button></p>
		{:else if part.cards === null}
			<p class="s-hint">Loading…</p>
		{:else if part.cards.length}
			<div class="cards">
				{#each part.cards as c (c.key)}
					<Card title={c.title} sub={c.sub} artwork={c.artwork} href={c.href} ini={c.round ? initials(c.title) : undefined} />
				{/each}
			</div>
		{:else}
			<p class="s-hint">None yet.</p>
		{/if}
	{/each}
{/if}

<style>
	.tube-part { margin-top: 24px; }
	.tube-part:first-of-type { margin-top: 0; }
</style>
