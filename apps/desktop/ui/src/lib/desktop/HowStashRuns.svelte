<!-- How Stash runs: on the listener's own music files only, or with streaming and discovery too. Asked once, the first
     time Stash opens (`first`: it covers the window until one is chosen), and changed in Settings › General after. -->
<script lang="ts">
	import { setMode } from '../app';
	import type { Mode } from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';

	let { first = false }: { first?: boolean } = $props();
	let busy = $state<Mode | null>(null);

	async function choose(mode: Mode) {
		if (busy || ui.mode === mode) return;
		busy = mode;
		try {
			await setMode(mode);
			if (!first) ui.toast(mode === 'local' ? 'Stash now plays only your own music files' : 'Streaming and discovery are back');
		} catch {
			ui.toast('That could not be changed just now. Try again.');
		} finally {
			busy = null;
		}
	}
</script>

{#snippet choices()}
	<div class="how-c" role="radiogroup" aria-label="How Stash runs">
		<button type="button" class="how-o" class:on={ui.mode === 'local'} role="radio" aria-checked={ui.mode === 'local'} disabled={busy !== null} onclick={() => void choose('local')}>
			<Icon name="folder" />
			<b>My own music files</b>
			<span>A player for the music on this computer. Nothing is streamed, nothing is looked up, and Stash never goes online.</span>
		</button>
		<button type="button" class="how-o" class:on={ui.mode === 'everything'} role="radio" aria-checked={ui.mode === 'everything'} disabled={busy !== null} onclick={() => void choose('everything')}>
			<Icon name="radio" />
			<b>Streaming and discovery too</b>
			<span>Your own files, plus YouTube Music, add-ons you choose, mixes, radio, lyrics and downloads.</span>
		</button>
	</div>
{/snippet}

{#if first}
	<div class="how-veil" role="presentation"></div>
	<div class="how" role="dialog" aria-modal="true" aria-labelledby="how-h">
		<h2 id="how-h">How do you want to use Stash?</h2>
		<p>Choose one to start. You can change it any time in Settings › General.</p>
		{@render choices()}
	</div>
{:else}
	{@render choices()}
{/if}

<style>
	.how-veil { position: fixed; inset: 0; z-index: 95; background: var(--wp-main); opacity: .94; }
	.how { position: fixed; z-index: 96; left: 50%; top: 14vh; transform: translateX(-50%); width: min(720px, calc(100vw - 32px)); display: grid; gap: 14px; }
	.how h2 { margin: 0; font: 600 28px/1.2 var(--font-display, var(--font-body)); }
	.how p { margin: 0; color: var(--wp-text-2); font: 400 15px/1.5 var(--font-body); }
	.how-c { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; }
	.how-o { display: grid; gap: 8px; align-content: start; justify-items: start; padding: 18px; border-radius: 16px; border: 1.5px solid var(--wp-line-2); background: var(--wp-main); color: var(--wp-text); text-align: left; transition: border-color var(--dur-short), background var(--dur-short); }
	.how-o :global(.ic) { width: 28px; height: 28px; color: var(--acc-text); }
	.how-o b { font: 600 17px/1.3 var(--font-display, var(--font-body)); }
	.how-o span { color: var(--wp-text-2); font: 400 14px/1.5 var(--font-body); }
	.how-o.on { border-color: var(--acc); background: var(--wp-hover); }
	.how-o:disabled { opacity: .6; }
	.how-o:focus-visible { outline: 2px solid var(--acc); outline-offset: 2px; }
	@media (hover: hover) and (pointer: fine) { .how-o:not(:disabled):hover { border-color: var(--acc); } }
</style>
