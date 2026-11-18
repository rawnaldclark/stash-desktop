<!-- Settings › YouTube Music › restricted songs: yt-dlp, for the songs YouTube's apps will not play to Stash. Stash fetches
     its own copy when asked (from yt-dlp's releases on GitHub, with Deno when the computer has no runtime for it), or uses
     the system's. Its row sits in the page's YouTube Music card. -->
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
	/** What a fetch brings: yt-dlp (about 18 MB), and Deno (about 45 MB) when there is no runtime for it. */
	const size = $derived(f?.with_deno ? 63 : 18);
	/** Which copy is in use, for the name's tooltip. */
	const which = $derived(f?.found ? `yt-dlp ${f.found.version ?? ''}${f.found.own ? '' : ' (on this computer)'}`.trim() : undefined);
</script>

{#if f}
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name" id="ytdlp-l" title={which}>Play restricted songs</span>
			{#if f.fetching}
				<span class="set-sub" role="status">Downloading… {mb(f.fetching.got)}{f.fetching.total ? ` of ${mb(f.fetching.total)}` : ''} MB</span>
			{:else if f.found || f.can_fetch}
				<span class="set-sub">For songs YouTube won’t play here</span>
			{:else}
				<span class="set-sub">Not available for this computer</span>
			{/if}
			{#if f.found && !f.found.runtime && !f.fetching}<span class="set-sub set-trouble">Incomplete. Update to fix it.</span>{/if}
			{#if f.trouble}<span class="set-sub set-trouble" role="alert">{f.trouble}</span>{/if}
			{#if wrong}<span class="set-sub set-trouble" role="alert">{wrong}</span>{/if}
		</div>
		<div class="set-ctl">
			{#if f.found}
				{#if f.can_fetch && !f.fetching}<button type="button" class="btn ghost" disabled={ui.offline} onclick={() => act(core.fetchTubeFallback)}>Update</button>{/if}
				<button type="button" class="sw" role="switch" aria-checked={f.on} aria-labelledby="ytdlp-l" onclick={() => act(() => core.setTubeFallback(!f!.on))}><i></i></button>
			{:else if f.can_fetch && !f.fetching}
				<button type="button" class="btn ghost" disabled={ui.offline} onclick={() => act(core.fetchTubeFallback)}>Download ({size} MB)</button>
			{/if}
		</div>
	</div>
{/if}
