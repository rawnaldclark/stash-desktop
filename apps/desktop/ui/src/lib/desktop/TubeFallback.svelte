<!-- Settings › YouTube Music › yt-dlp: for the songs YouTube's apps will not play to Stash. Stash fetches its own copy when
     asked (from yt-dlp's releases on GitHub, with Deno when the computer has no runtime for it), or uses the system's. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { ui } from '../ui/state.svelte';

	let f = $state<core.TubeFallback | null>(null);
	let wrong = $state('');
	const look = () =>
		core.tubeFallback().then(
			(got) => (f = got),
			() => {}
		);
	onMount(() => void look());
	// While Stash fetches it, look every second.
	const fetching = $derived(!!f?.fetching);
	$effect(() => {
		if (!fetching) return;
		const again = setInterval(() => void look(), 1000);
		return () => clearInterval(again);
	});

	async function act(ask: () => Promise<core.TubeFallback>) {
		wrong = '';
		try {
			f = await ask();
		} catch (e) {
			wrong = core.said(e);
		}
	}
	const mb = (bytes: number) => (bytes / (1024 * 1024)).toFixed(1);
	const RUNTIME: Record<string, string> = { deno: 'Deno', node: 'Node', quickjs: 'QuickJS', bun: 'Bun' };
</script>

{#if f}
<div class="set-card yt-more">
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name" id="ytdlp-l">When YouTube won’t play a song</span>
			<span class="set-sub">Some songs YouTube’s apps won’t play to Stash: ones that ask you to confirm your age, some label releases. yt-dlp, a free and open tool, can get those; Stash asks it only when its own way fails, and signed in, it asks as you.</span>
		</div>
		{#if f.found}
			<button type="button" class="sw" role="switch" aria-checked={f.on} aria-labelledby="ytdlp-l" onclick={() => act(() => core.setTubeFallback(!f!.on))}><i></i></button>
		{/if}
	</div>
	<div class="set-row set-col">
		{#if f.fetching}
			<p class="set-sub" role="status">Fetching {f.fetching.what}… {mb(f.fetching.got)}{f.fetching.total ? ` of ${mb(f.fetching.total)}` : ''} MB</p>
		{:else if f.found}
			<p class="set-sub">
				yt-dlp {f.found.version ?? ''} ({f.found.own ? 'Stash’s own copy' : 'the one on this computer'}){f.found.runtime ? `, with ${RUNTIME[f.found.runtime] ?? f.found.runtime}` : ''}.
				{#if !f.found.runtime}<b>It has no JavaScript runtime, which YouTube’s songs need: fetch Stash’s own copy to get Deno with it.</b>{/if}
			</p>
			{#if f.can_fetch}
				<div class="chips">
					<button type="button" class="btn ghost" disabled={ui.offline} onclick={() => act(core.fetchTubeFallback)}>{f.found.own ? 'Update' : 'Use Stash’s own copy'}</button>
				</div>
			{/if}
		{:else if f.can_fetch}
			<div class="chips">
				<button type="button" class="btn" disabled={ui.offline} onclick={() => act(core.fetchTubeFallback)}>Get yt-dlp</button>
			</div>
			<p class="set-sub">From yt-dlp’s releases on GitHub (about 18 MB){f.with_deno ? ', with Deno (about 45 MB), the JavaScript runtime it needs for YouTube' : ''}, each checked against the checksum its release publishes. Kept beside your library.</p>
		{:else}
			<p class="set-sub">yt-dlp has no build for this computer. Install it yourself and Stash will find it.</p>
		{/if}
		{#if f.trouble}<p class="set-sub set-trouble" role="alert">{f.trouble}</p>{/if}
		{#if wrong}<p class="set-sub set-trouble" role="alert">{wrong}</p>{/if}
	</div>
</div>
{/if}

<style>
	.yt-more { margin-top: 12px; }
</style>
