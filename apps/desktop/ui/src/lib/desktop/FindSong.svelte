<!-- One song no add-on could find, being fixed: the add-ons' closest results for it (one search, made now that the listener
     asked), the words searched (which the listener can change), and "None of these". A pick is kept as that add-on's name for
     the song, so it plays from there from now on; nothing is played or saved to look. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { mmss } from '../ui/format';
	import { ui } from '../ui/state.svelte';
	import { NF } from './unfound.svelte';

	let { id, title, ondone }: { id: number; title: string; ondone: () => void } = $props();

	let q = $state('');
	let looking = $state(true);
	let found = $state<core.Candidates | null>(null);
	let failed = $state<string | null>(null);
	let busy = $state(false);
	let asking: AbortController | null = null;

	async function look(words?: string) {
		asking?.abort();
		const mine = (asking = new AbortController());
		looking = true;
		failed = null;
		try {
			const r = await core.candidates(id, words, mine.signal);
			if (mine !== asking) return;
			found = r;
			q = r.query;
		} catch (e) {
			if (mine !== asking || mine.signal.aborted) return;
			failed = core.said(e);
		} finally {
			if (mine === asking) looking = false;
		}
	}
	onMount(() => {
		void look();
		return () => asking?.abort();
	});

	const groups = $derived(found?.groups ?? []);
	const any = $derived(groups.some((g) => g.tracks.length > 0));

	async function pick(g: core.AddonGroup, t: core.FoundTrack) {
		busy = true;
		try {
			const u = await core.fixSong(id, g.addon.id, t.id);
			NF.take(u);
			ui.toast(u.saving ? `Fixed. “${title}” plays from ${g.addon.name} now, and is being saved.` : `Fixed. “${title}” plays from ${g.addon.name} now.`);
			ondone();
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			busy = false;
		}
	}
	async function none() {
		busy = true;
		try {
			NF.take(await core.setAside([id]));
			ondone();
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			busy = false;
		}
	}
</script>

<div class="nf-find" role="region" aria-label="Find {title} on your add-ons">
	<form
		class="nf-q"
		onsubmit={(e) => {
			e.preventDefault();
			void look(q.trim() || undefined);
		}}
	>
		<label class="nf-field"><Icon name="search" /><input type="search" aria-label="Search your add-ons" bind:value={q} maxlength="200" /></label>
		<button type="submit" class="btn ghost" disabled={looking}>Search</button>
	</form>
	{#if looking}
		<p class="nf-hint">Looking on your add-ons…</p>
	{:else if failed}
		<p class="nf-hint">{failed}</p>
	{:else if !groups.length}
		<p class="nf-hint">None of your add-ons can be searched.</p>
	{:else}
		{#each groups as g (g.addon.id)}
			{#if g.error}
				<p class="nf-hint">{g.addon.name}: {g.error.message}</p>
			{:else if g.tracks.length}
				<p class="caps nf-from">On {g.addon.name}</p>
				<ul class="nf-picks">
					{#each g.tracks as t, i (i)}
						<li>
							<button type="button" class="nf-pick" disabled={busy} onclick={() => pick(g, t)}>
								<span class="nf-pt"><b dir="auto">{t.title}</b><i dir="auto">{#if t.explicit}<span class="fmt">E</span>{/if}{t.artist}{t.album ? ` · ${t.album}` : ''}</i></span>
								<span class="nf-pd">{t.duration_ms ? mmss(t.duration_ms) : ''}</span>
							</button>
						</li>
					{/each}
				</ul>
			{/if}
		{/each}
		{#if !any && !groups.some((g) => g.error)}<p class="nf-hint">Nothing came up. Try other words.</p>{/if}
	{/if}
	<div class="nf-acts"><button type="button" class="btn ghost" disabled={busy} onclick={none}>None of these</button></div>
</div>

<style>
	.nf-find { margin: 4px 0 12px; padding: 12px 14px; border-radius: 12px; background: var(--wp-field); }
	.nf-q { display: flex; gap: 8px; align-items: center; }
	.nf-field { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; height: 38px; padding: 0 12px; border-radius: 10px; background: var(--wp-main); border: 1px solid var(--wp-line); color: var(--wp-text-2); }
	.nf-field:focus-within { outline: 2px solid var(--focus); outline-offset: 2px; }
	.nf-field :global(.ic) { width: 18px; height: 18px; flex: none; }
	.nf-field input { flex: 1; min-width: 0; height: 100%; border: 0; background: none; color: var(--wp-text); font: 400 14px/1 var(--font-body); outline: none; }
	.nf-hint { margin: 12px 2px 0; color: var(--wp-text-2); font: 400 13px/1.45 var(--font-body); }
	.nf-from { margin: 14px 2px 6px; }
	.nf-picks { display: grid; gap: 2px; }
	.nf-pick { display: flex; align-items: center; gap: 12px; width: 100%; padding: 8px 10px; border-radius: 8px; text-align: left; }
	@media (hover: hover) and (pointer: fine) { .nf-pick:hover:not(:disabled) { background: var(--wp-hover); } }
	.nf-pt { display: grid; min-width: 0; flex: 1; }
	.nf-pt b, .nf-pt i { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
	.nf-pt b { color: var(--wp-text); font: 600 14px/1.35 var(--font-body); }
	.nf-pt i { color: var(--wp-text-2); font: 400 13px/1.35 var(--font-body); }
	.nf-pt .fmt { margin-right: 6px; }
	.nf-pd { color: var(--wp-num); font: 400 13px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	.nf-acts { margin-top: 10px; }
</style>
