<!-- A ranked or ordered list as bars: a name, a bar on a shared scale, and its figure, per row, bars aligned. Every
     value is printed beside its bar, so nothing waits on hovering. Rows that lead somewhere are links. With `whole`,
     each bar is a meter against that whole, its unfilled part a lighter step of the same hue. -->
<script lang="ts">
	type Row = { label: string; value: number; figure: string; href?: string; muted?: boolean; detail?: string };
	let { rows, label, whole }: { rows: Row[]; label: string; whole?: number } = $props();
	const max = $derived(whole ?? Math.max(1, ...rows.map((r) => r.value)));
</script>

<ul class="bl" aria-label={label}>
	{#each rows as r (r.label)}
		<li>
			<svelte:element this={r.href ? 'a' : 'div'} class="bl-r" class:muted={r.muted} href={r.href}>
				<span class="bl-n" title={r.detail ? `${r.label} · ${r.detail}` : r.label}>{r.label}{#if r.detail}<small>{r.detail}</small>{/if}</span>
				<span class="bl-t" class:meter={whole !== undefined} aria-hidden="true"><i style:width={r.value > 0 ? `max(3px, ${(r.value / max) * 100}%)` : '0'}></i></span>
				<span class="bl-v">{r.figure}</span>
			</svelte:element>
		</li>
	{/each}
</ul>

<style>
	.bl { margin: 0; padding: 0; list-style: none; display: grid; gap: 2px; }
	.bl-r { display: grid; grid-template-columns: minmax(0, 11fr) minmax(60px, 10fr) auto; align-items: center; gap: 12px; min-height: 32px; padding: 0 8px; margin: 0 -8px; border-radius: 8px; color: var(--wp-text); text-decoration: none; font: 400 14px/1.3 var(--font-body); }
	.bl-n { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.bl-n small { display: block; overflow: hidden; text-overflow: ellipsis; color: var(--wp-text-2); font-size: 12.5px; }
	.bl-r:has(small) { min-height: 42px; }
	.muted .bl-n { color: var(--wp-text-2); font-style: italic; }
	/* The bar: square where it starts, a 4px rounded end where its value is. */
	.bl-t { height: 10px; }
	.bl-t.meter { border-radius: 0 4px 4px 0; background: var(--st-quiet); }
	.bl-t i { display: block; height: 100%; border-radius: 0 4px 4px 0; background: var(--st-bar); }
	.muted .bl-t i { background: var(--st-quiet); }
	.bl-v { min-width: 72px; text-align: right; color: var(--wp-text-2); font-size: 13px; font-variant-numeric: tabular-nums; white-space: nowrap; }
	@media (hover: hover) and (pointer: fine) { a.bl-r:hover { background: var(--wp-hover); } }
	a.bl-r:focus-visible { outline: 2px solid var(--st-bar); outline-offset: -2px; }
</style>
