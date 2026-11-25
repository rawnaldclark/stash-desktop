<!-- Every key the window answers to (`?`, or Keyboard shortcuts in the command palette), from `keys.ts`: what is listed
     here is what the window does. The command palette's look; Escape or a click outside closes it and gives focus back. -->
<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import * as core from '../core';
	import { KEY_GROUPS } from './keys';
	import { ui } from './state.svelte';

	let box = $state<HTMLElement>();
	let link = $state<HTMLElement>();
	const before = document.activeElement as HTMLElement | null;
	const close = (refocus = true) => {
		ui.keys = false;
		if (refocus) void tick().then(() => before?.focus?.());
	};
	// While it is open the keys are the sheet's: Space does not pause the song behind it, Tab stays inside.
	function key(e: KeyboardEvent) {
		e.stopPropagation();
		if (e.key === 'Escape' || e.key === '?') {
			e.preventDefault();
			close();
		} else if (e.key === 'Tab') {
			e.preventDefault();
			(document.activeElement === link ? box : link)?.focus();
		}
	}
	/** Settings › Shortcuts is there only where this computer can hold keys system-wide (as Settings itself asks). */
	let anyApp = $state(false);
	onMount(() => {
		box?.focus();
		core.shortcuts().then(
			(s) => (anyApp = s.available),
			() => {}
		);
	});
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="pal-veil" role="presentation" onclick={() => close()}></div>
<div class="ks" role="dialog" aria-modal="true" aria-labelledby="ks-h" tabindex="-1" bind:this={box} onkeydown={key}>
	<div class="ks-top">
		<h2 id="ks-h">Keyboard shortcuts</h2>
		<span class="kbd">Esc</span>
	</div>
	<div class="ks-body">
		{#each KEY_GROUPS as g (g.title)}
			<section class="ks-g" aria-label={g.title}>
				<h3 class="caps">{g.title}</h3>
				<dl>
					{#each g.rows as r (r.does)}
						<div class="ks-r">
							<dt>
								{#each r.keys as combo, i (i)}
									{#if i}<span class="ks-or">or</span>{/if}
									<span class="ks-combo">{#each combo as part, j (j)}{#if j}<span class="ks-plus">+</span>{/if}<kbd class="kbd">{part}</kbd>{/each}</span>
								{/each}
							</dt>
							<dd>{r.does}</dd>
						</div>
					{/each}
				</dl>
			</section>
		{/each}
	</div>
	{#if anyApp}
		<p class="ks-foot">Keys that work in any app, even with Stash in the background, are yours to choose in <button type="button" class="lnk" bind:this={link} onclick={() => { close(false); void goto('/settings#shortcuts'); }}>Settings › Shortcuts</button>.</p>
	{/if}
</div>

<style>
	.pal-veil { position: fixed; inset: 0; z-index: 80; background: rgb(0 0 0 / .32); }
	.ks { position: fixed; z-index: 81; left: 50%; top: 8vh; transform: translateX(-50%); width: min(820px, calc(100vw - 32px)); max-height: 84vh; display: flex; flex-direction: column; border-radius: 14px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); overflow: hidden; color: var(--wp-text); outline: none; }
	.ks-top { display: flex; align-items: center; justify-content: space-between; gap: 12px; height: 56px; padding: 0 18px; border-bottom: 1px solid var(--wp-line); flex: none; }
	.ks-top h2 { margin: 0; font: 600 16px/1.2 var(--font-display); color: var(--wp-text); }
	.ks-body { display: grid; grid-template-columns: repeat(auto-fit, minmax(340px, 1fr)); gap: 8px 28px; padding: 10px 18px 6px; overflow-y: auto; }
	.ks-g h3 { margin: 12px 0 6px; }
	/* One column of keys per group, so what they do lines up under each other. */
	.ks-g dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); column-gap: 16px; margin: 0; }
	.ks-r { display: grid; grid-column: 1 / -1; grid-template-columns: subgrid; align-items: center; min-height: 34px; padding: 3px 0; border-bottom: 1px solid var(--wp-line); }
	.ks-r:last-child { border-bottom: 0; }
	.ks-r dt { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; }
	.ks-r dd { margin: 0; color: var(--wp-text-2); font: 400 13.5px/1.35 var(--font-body); }
	.ks-combo { display: inline-flex; align-items: center; gap: 3px; }
	.ks-plus, .ks-or { color: var(--wp-text-2); font: 400 12px/1 var(--font-body); }
	.ks-foot { margin: 0; padding: 12px 18px 14px; border-top: 1px solid var(--wp-line); color: var(--wp-text-2); font: 400 13px/1.4 var(--font-body); flex: none; }
	@media (prefers-reduced-motion: no-preference) { .ks { animation: ks-in 140ms var(--ease-out-expo, ease-out) both; } }
	@keyframes ks-in { from { opacity: 0; transform: translate(-50%, -6px); } }
</style>
