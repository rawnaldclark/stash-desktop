<!-- Settings › Playback › Not played: the songs and artists the listener chose not to play (from a song's menu), each with
     "Play again". Stash never plays them on its own; a song chosen by itself still plays. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { notPlayed } from '../app';
	import * as core from '../core';
	import { ui } from '../ui/state.svelte';

	let rows = $state<core.Blocked[] | null>(null);
	let busy = $state(false);

	onMount(() => {
		core.blocks().then(
			(r) => (rows = r),
			() => (rows = [])
		);
	});

	async function again(b: core.Blocked) {
		busy = true;
		try {
			rows = await notPlayed(b.kind, { title: b.title, artist: b.artist }, false, b.key);
			ui.toast(`${b.kind === 'artist' ? b.artist : `“${b.title}”`} plays again`);
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			busy = false;
		}
	}
</script>

<section class="set-part" id="not-played" aria-labelledby="h-not-played">
	<h3 class="set-h" id="h-not-played">Not played</h3>
	<div class="set-card" hidden={rows === null}>
		{#if rows !== null && rows.length === 0}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">Nothing here</span>
					<span class="set-sub">Choose “Don’t play” in a song’s menu</span>
				</div>
			</div>
		{:else if rows}
			{#each rows as b (`${b.kind}:${b.key}`)}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name">{b.kind === 'artist' ? b.artist : b.title}</span>
						<span class="set-sub">{b.kind === 'artist' ? 'All songs' : b.artist}</span>
					</div>
					<button type="button" class="btn ghost" disabled={busy} aria-label="Play {b.kind === 'artist' ? b.artist : b.title} again" onclick={() => void again(b)}>Play again</button>
				</div>
			{/each}
		{/if}
	</div>
</section>
