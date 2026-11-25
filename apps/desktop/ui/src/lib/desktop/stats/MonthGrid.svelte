<!-- The last twelve months of Library › Statistics, modeled on Namida's month cells ("Your year"): each month is the
     cover of the song played most in it with the month on a chip at its corner (10px corners, the chip 6px), the song's
     name under it and the month's plays; a thin bar under them compares the month with the busiest one. A month with no
     plays is an empty tile. Choosing a month plays its song. -->
<script lang="ts">
	import { artUrl, type StatMonth } from '../../core';
	import Icon from '../../ui/Icon.svelte';
	import { monthNames, playsWord } from './stats';

	let { months, onplay }: { months: StatMonth[]; onplay: (id: number) => void } = $props();
	const busiest = $derived(Math.max(1, ...months.map((m) => m.plays)));
	const thisYear = new Date().getFullYear();
	/** "Nov", or "Dec ’25" for a month of another year. */
	const chip = (month: string) => {
		const year = Number(month.slice(0, 4));
		const short = monthNames(month).short;
		return year && year !== thisYear ? `${short} ’${String(year).slice(2)}` : short;
	};
</script>

<div class="mg-wrap">
	<ol class="mg">
		{#each months as m (m.month)}
			{@const top = m.top}
			{@const full = monthNames(m.month).full}
			<li>
				{#if top}
					<button
						type="button"
						class="mg-c"
						title="{full}: {playsWord(m.plays)}. Played most: {top.title} by {top.artist} ({playsWord(top.plays)}). Choose to play it."
						aria-label="{full}: {playsWord(m.plays)}. Play the song played most, {top.title} by {top.artist}"
						onclick={() => onplay(top.id)}
					>
						<span class="mg-pic">
							<Icon name="album" />
							{#if artUrl(top.art)}<img src={artUrl(top.art)} alt="" loading="lazy" decoding="async" crossorigin="anonymous" onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')} />{/if}
							<span class="mg-m">{chip(m.month)}</span>
						</span>
						<b>{top.title}</b>
						<i>{playsWord(m.plays)}</i>
						<span class="mg-bar" aria-hidden="true"><s style:width="{Math.max(4, (m.plays / busiest) * 100)}%"></s></span>
					</button>
				{:else}
					<div class="mg-c none" role="img" aria-label="{full}: no plays">
						<span class="mg-pic"><span class="mg-m">{chip(m.month)}</span></span>
						<b>No plays</b>
						<i>&nbsp;</i>
						<span class="mg-bar" aria-hidden="true"></span>
					</div>
				{/if}
			</li>
		{/each}
	</ol>
</div>

<style>
	.mg-wrap { container-type: inline-size; }
	.mg { display: grid; grid-template-columns: repeat(6, minmax(0, 1fr)); gap: 14px 12px; margin: 0; padding: 0; list-style: none; }
	@container (max-width: 760px) { .mg { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
	@container (max-width: 480px) { .mg { grid-template-columns: repeat(3, minmax(0, 1fr)); } }
	.mg-c { display: grid; gap: 2px; width: 100%; padding: 6px; border-radius: 14px; text-align: left; color: var(--wp-text); transition: background-color 150ms var(--ease-standard); }
	.mg-pic { position: relative; display: grid; place-items: center; aspect-ratio: 1; margin-bottom: 4px; overflow: hidden; border-radius: 10px; background: color-mix(in srgb, var(--wp-text) 6%, transparent); color: var(--wp-text-2); }
	.mg-pic img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
	.mg-pic :global(.ic) { width: 34%; height: 34%; }
	.mg-m { position: absolute; top: 4px; left: 4px; padding: 2px 6px; border-radius: 6px; background: color-mix(in srgb, var(--wp-main) 85%, transparent); color: var(--wp-text); font: 700 11px/1.35 var(--font-body); }
	.mg-c b, .mg-c i { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.mg-c b { font: 600 12.5px/1.3 var(--font-body); }
	.mg-c i { font: 400 12px/1.3 var(--font-body); color: var(--wp-text-2w); font-variant-numeric: tabular-nums; }
	.mg-c.none b { color: var(--wp-text-2w); font-weight: 500; }
	.mg-bar { display: block; height: 3px; margin-top: 4px; border-radius: 2px; background: color-mix(in srgb, var(--acc) 18%, transparent); }
	.mg-bar s { display: block; height: 100%; border-radius: 2px; background: var(--acc); }
	@media (hover: hover) and (pointer: fine) { button.mg-c:hover { background: var(--wp-hover); } }
	button.mg-c:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
</style>
