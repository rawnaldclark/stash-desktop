<!-- Settings › Not played: the songs and artists the listener chose not to play (from a song's menu), each with "Play
     again". Stash never plays them on its own; a song chosen by itself still plays. -->
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
			rows = await notPlayed(b.kind, { title: b.title, artist: b.artist }, false);
			ui.toast(`${b.kind === 'artist' ? b.artist : `“${b.title}”`} plays again`);
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			busy = false;
		}
	}
</script>

<section class="set-sec" id="not-played" aria-labelledby="h-not-played">
	<h2 class="set-h" id="h-not-played">Not played</h2>
	<div class="set-card">
		{#if rows !== null && rows.length === 0}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">Nothing yet</span>
					<span class="set-sub">Choose “Don’t play this song” or “Don’t play” an artist from a song’s menu, and Stash leaves them out of what it plays on its own: autoplay, and lists played through. A song you pick by itself still plays.</span>
				</div>
			</div>
		{:else if rows}
			{#each rows as b (`${b.kind}:${b.key}`)}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name">{b.kind === 'artist' ? b.artist : b.title}</span>
						<span class="set-sub">{b.kind === 'artist' ? 'Every song by this artist' : b.artist}</span>
					</div>
					<button type="button" class="btn ghost" disabled={busy} onclick={() => void again(b)}>Play again</button>
				</div>
			{/each}
		{/if}
	</div>
</section>
