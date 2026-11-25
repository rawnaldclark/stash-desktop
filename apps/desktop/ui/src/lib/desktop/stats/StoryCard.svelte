<!-- One card of Library › Statistics, modeled on the cards of Namida's "Your year": the main surface tinted with the
     album's colour (16%), two soft circles of it in opposite corners (10% and 7%), 24px corners, a soft shadow, a title
     in the display face and a quieter line under it. `tools`: what sits at the right of the title (a button). -->
<script lang="ts">
	import type { Snippet } from 'svelte';

	let { title, sub, wide = false, tools, children }: { title?: string; sub?: string; wide?: boolean; tools?: Snippet; children: Snippet } = $props();
</script>

<section class="sc" class:wide aria-label={title}>
	<i class="sc-o top" aria-hidden="true"></i>
	<i class="sc-o low" aria-hidden="true"></i>
	{#if title}
		<header class="sc-h">
			<div class="sc-ht">
				<h2>{title}</h2>
				{#if sub}<p>{sub}</p>{/if}
			</div>
			{#if tools}{@render tools()}{/if}
		</header>
	{/if}
	{@render children()}
</section>

<style>
	.sc { position: relative; isolation: isolate; overflow: hidden; min-width: 0; padding: 22px 24px 24px; border-radius: 24px; background: color-mix(in srgb, var(--acc) 16%, var(--wp-main)); box-shadow: 0 4px 12px rgb(0 0 0 / .16); }
	.sc.wide { grid-column: 1 / -1; }
	.sc-o { position: absolute; z-index: -1; border-radius: 50%; pointer-events: none; }
	.sc-o.top { top: -60px; right: -60px; width: 220px; height: 220px; background: color-mix(in srgb, var(--acc) 10%, transparent); }
	.sc-o.low { bottom: -80px; left: -40px; width: 200px; height: 200px; background: color-mix(in srgb, var(--acc) 7%, transparent); }
	.sc-h { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px 16px; margin: 0 0 18px; }
	.sc-ht { min-width: 0; }
	.sc-h h2 { margin: 0; font: 700 22px/1.2 var(--font-display); letter-spacing: -.02em; color: var(--wp-text); }
	.sc-h p { margin: 4px 0 0; font: 400 13.5px/1.4 var(--font-body); color: var(--wp-text-2w); }
</style>
