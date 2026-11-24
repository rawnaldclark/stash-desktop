<!-- Columns over an ordered scale (months, decades): one hue, a clean axis, the highest value on its cap, and every
     value on hover and keyboard focus (each column is focusable and names its value for screen readers too). -->
<script lang="ts">
	import { niceTop, tick } from './stats';

	type Point = { short: string; full: string; value: number };
	let { points, unit, label }: { points: Point[]; unit: (n: number) => string; label: string } = $props();

	const top = $derived(niceTop(Math.max(0, ...points.map((p) => p.value))));
	const highest = $derived(points.reduce((best, p, i) => (p.value > (points[best]?.value ?? -1) ? i : best), 0));
	let shown = $state<number | null>(null);
</script>

<div class="co" role="group" aria-label={label}>
	<div class="co-axis" aria-hidden="true">
		<span style:bottom="100%">{tick(top)}</span>
		<span style:bottom="50%">{tick(top / 2)}</span>
		<span style:bottom="0">0</span>
	</div>
	<div class="co-plot">
		<i class="co-grid top"></i>
		<i class="co-grid mid"></i>
		{#each points as p, i (p.full)}
			<!-- Focusable so its value is reachable by keyboard as well as by hovering; it names its value itself. -->
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<div
				class="co-c"
				class:on={shown === i}
				role="img"
				tabindex="0"
				aria-label="{p.full}: {unit(p.value)}"
				onpointerenter={() => (shown = i)}
				onpointerleave={() => (shown = null)}
				onfocus={() => (shown = i)}
				onblur={() => (shown = null)}
			>
				<div class="co-slot">
					{#if p.value > 0}
						<b style:height="{(p.value / top) * 100}%">
							{#if i === highest && shown === null}<span class="co-cap">{p.value.toLocaleString()}</span>{/if}
						</b>
					{/if}
					{#if shown === i}
						<span class="co-tip" class:end={i >= points.length - 2} class:start={i < 2} style:bottom="{Math.min(100, (p.value / top) * 100)}%"><b>{p.value.toLocaleString()}</b> {p.full}</span>
					{/if}
				</div>
				<span class="co-l">{p.short}</span>
			</div>
		{/each}
	</div>
</div>

<style>
	.co { display: grid; grid-template-columns: auto 1fr; gap: 8px; padding-top: 18px; }
	.co-axis { position: relative; height: 150px; min-width: 34px; color: var(--wp-text-2); font: 400 11.5px/1 var(--font-body); font-variant-numeric: tabular-nums; text-align: right; }
	.co-axis span { position: absolute; right: 0; transform: translateY(50%); }
	.co-plot { position: relative; display: flex; align-items: stretch; height: 172px; }
	/* Recessive hairlines: the baseline and two clean ticks. */
	.co-plot::after { content: ''; position: absolute; left: 0; right: 0; top: 150px; height: 1px; background: var(--wp-line-2); }
	.co-grid { position: absolute; left: 0; right: 0; height: 1px; background: var(--wp-line); }
	.co-grid.top { top: 0; }
	.co-grid.mid { top: 75px; }
	.co-c { flex: 1 1 0; display: grid; grid-template-rows: 150px auto; justify-items: center; min-width: 0; border-radius: 6px; outline: none; cursor: default; }
	.co-slot { position: relative; width: 100%; display: flex; justify-content: center; align-items: flex-end; }
	/* A column: at most 24px, square on the baseline, 4px rounded at its value. */
	.co-slot > b { position: relative; width: min(24px, 60%); border-radius: 4px 4px 0 0; background: var(--st-bar); }
	.co-c.on .co-slot > b, .co-c:focus-visible .co-slot > b { background: var(--st-bar-on); }
	.co-c:focus-visible { box-shadow: inset 0 0 0 2px var(--st-bar); }
	.co-cap { position: absolute; bottom: calc(100% + 4px); left: 50%; transform: translateX(-50%); color: var(--wp-text); font: 600 12px/1 var(--font-body); font-variant-numeric: tabular-nums; white-space: nowrap; }
	.co-tip { position: absolute; left: 50%; transform: translate(-50%, -8px); z-index: 2; padding: 6px 9px; border-radius: 8px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 6px 18px rgb(0 0 0 / .18); color: var(--wp-text-2); font: 400 12.5px/1.2 var(--font-body); white-space: nowrap; pointer-events: none; }
	/* Near either edge the tip opens inwards, so it never runs off the card. */
	.co-tip.end { left: auto; right: 0; transform: translateY(-8px); }
	.co-tip.start { left: 0; transform: translateY(-8px); }
	.co-tip b { color: var(--wp-text); font-weight: 700; font-variant-numeric: tabular-nums; }
	.co-l { margin-top: 8px; color: var(--wp-text-2); font: 400 12px/1 var(--font-body); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 100%; }
</style>
