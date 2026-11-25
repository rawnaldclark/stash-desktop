<!-- A page of the library's own (History, Downloads, Statistics…) as a button in the sidebar, modeled on Namida's drawer
     tile (NamidaDrawerListTile): a card a little tinted with the album's colour (Namida: 14% of it over its card grey; here
     over Stash's raised surface), 8px corners, a 20px icon, 10px apart from its name in 15px medium. The page that is open
     is filled with the colour and lifted by a glow of it (39%, 8px, 3px down), its ink the album colour's own. Folded to
     the rail, the icon alone. `count`: a number it carries at its end (songs waiting on Not found). -->
<script lang="ts">
	import Icon from './Icon.svelte';
	import type { IconName } from './icons';

	let { href, icon, label, on = false, count }: { href: string; icon: IconName; label: string; on?: boolean; count?: number } = $props();
</script>

<a class="nt" class:on {href} title={count ? `${label} (${count.toLocaleString()})` : label} aria-current={on ? 'page' : undefined}>
	<Icon name={icon} />
	<span class="nt-t">{label}</span>
	{#if count}<span class="nt-c">{count.toLocaleString()}</span>{/if}
</a>

<style>
	.nt {
		display: flex; align-items: center; gap: 10px; min-width: 0; padding: 11px 10px; border-radius: 8px;
		background: color-mix(in srgb, var(--acc) 14%, var(--wp-field)); color: var(--wp-text);
		font: 500 15px/20px var(--font-body); text-decoration: none;
		transition: background-color 200ms var(--ease-standard), box-shadow 200ms var(--ease-standard), color 200ms var(--ease-standard);
	}
	.nt :global(.ic) { width: 20px; height: 20px; flex: none; color: inherit; }
	/* Namida fades a name too long for the tile rather than cutting it with dots. */
	.nt-t { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; mask-image: linear-gradient(90deg, #000 calc(100% - 18px), transparent); }
	.nt-c { flex: none; min-width: 22px; padding: 0 7px; border-radius: 11px; background: color-mix(in srgb, var(--acc) 30%, transparent); font: 600 12px/22px var(--font-body); text-align: center; font-variant-numeric: tabular-nums; }
	.nt.on { background: var(--acc); color: var(--acc-ink); box-shadow: 0 3px 8px .2px color-mix(in srgb, var(--acc) 39%, transparent); }
	.nt.on .nt-c { background: color-mix(in srgb, var(--acc-ink) 22%, transparent); }
	@media (hover: hover) and (pointer: fine) { .nt:not(.on):hover { background: color-mix(in srgb, var(--acc) 24%, var(--wp-field)); } }
	.nt:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
	@media (prefers-reduced-motion: reduce) { .nt { transition: none; } }
	/* The rail (folded by the listener, or a narrow window): the icon alone, centred. */
	:global(.wp-g.rail) .nt { width: 48px; justify-content: center; padding: 11px 0; }
	:global(.wp-g.rail) .nt-t, :global(.wp-g.rail) .nt-c { display: none; }
	@container wp (max-width: 1099.98px) {
		.nt { width: 48px; justify-content: center; padding: 11px 0; }
		.nt-t, .nt-c { display: none; }
	}
</style>
