<!-- A ranked list of Library › Statistics with its pictures, modeled on Namida's top lists: the first one large (its
     art, its name, its count big), then a row each (Namida's StatsTopTrackTile: the place, a 42px picture with 8px
     corners, the name over its second line, the count at the end). Songs play from the one chosen (`onplay`), the
     list after it following; an artist opens (`href`). Artists' pictures are round. -->
<script lang="ts" module>
	export type TopEntry = { label: string; detail?: string; value: number; art?: string | null; href?: string };
</script>

<script lang="ts">
	import { artUrl } from '../../core';
	import Icon from '../../ui/Icon.svelte';

	let {
		entries,
		round = false,
		unit,
		onplay
	}: { entries: TopEntry[]; round?: boolean; unit: [string, string]; onplay?: (i: number) => void } = $props();

	const word = (n: number) => (n === 1 ? unit[0] : unit[1]);
	const said = (e: TopEntry, i: number) => `${i + 1}. ${e.label}${e.detail ? `, ${e.detail}` : ''}: ${e.value.toLocaleString()} ${word(e.value)}`;
</script>

{#snippet pic(e: TopEntry, big: boolean)}
	<span class="tl-pic" class:big class:round>
		<Icon name={round ? 'person' : 'album'} />
		{#if artUrl(e.art)}<img src={artUrl(e.art)} alt="" loading="lazy" decoding="async" crossorigin="anonymous" onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')} />{/if}
	</span>
{/snippet}

{#snippet hero(e: TopEntry)}
	{@render pic(e, true)}
	<span class="tl-ht">
		<span class="tl-one">#1</span>
		<b>{e.label}</b>
		{#if e.detail}<i>{e.detail}</i>{/if}
		<span class="tl-big"><strong>{e.value.toLocaleString()}</strong> {word(e.value)}</span>
	</span>
{/snippet}

{#snippet row(e: TopEntry, i: number)}
	<span class="tl-n" aria-hidden="true">{i + 1}</span>
	{@render pic(e, false)}
	<span class="tl-t"><b>{e.label}</b>{#if e.detail}<i>{e.detail}</i>{/if}</span>
	<span class="tl-v">{e.value.toLocaleString()}</span>
{/snippet}

{#if entries.length}
	{@const first = entries[0]}
	{#if first.href}
		<a class="tl-hero" href={first.href} aria-label={said(first, 0)}>{@render hero(first)}</a>
	{:else}
		<button type="button" class="tl-hero" aria-label="Play {said(first, 0)}" title="Play" onclick={() => onplay?.(0)}>{@render hero(first)}</button>
	{/if}
	{#if entries.length > 1}
		<ol class="tl-list">
			{#each entries.slice(1) as e, j (j)}
				<li>
					{#if e.href}
						<a class="tl-r" href={e.href} aria-label={said(e, j + 1)}>{@render row(e, j + 1)}</a>
					{:else}
						<button type="button" class="tl-r" aria-label="Play {said(e, j + 1)}" title="Play" onclick={() => onplay?.(j + 1)}>{@render row(e, j + 1)}</button>
					{/if}
				</li>
			{/each}
		</ol>
	{/if}
{/if}

<style>
	.tl-hero { display: flex; align-items: center; gap: 20px; width: 100%; margin: 0 0 14px; padding: 6px; border-radius: 16px; text-align: left; color: var(--wp-text); text-decoration: none; transition: background-color 150ms var(--ease-standard); }
	.tl-pic { position: relative; display: grid; place-items: center; width: 42px; height: 42px; flex: none; overflow: hidden; border-radius: 8px; background: color-mix(in srgb, var(--acc) 18%, var(--wp-field)); color: var(--wp-text-2); }
	.tl-pic.big { width: 132px; height: 132px; border-radius: 16px; box-shadow: 0 8px 24px rgb(0 0 0 / .28); }
	.tl-pic.round { border-radius: 50%; }
	/* The picture lies over its icon: one still on its way, or that never comes, leaves the icon showing. */
	.tl-pic img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
	.tl-pic :global(.ic) { width: 40%; height: 40%; }
	.tl-ht { display: grid; justify-items: start; gap: 4px; min-width: 0; }
	.tl-one { padding: 2px 8px; border-radius: 6px; background: var(--acc); color: var(--acc-ink); font: 700 12px/1.4 var(--font-body); }
	.tl-ht b { display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; font: 700 22px/1.2 var(--font-display); letter-spacing: -.015em; overflow-wrap: anywhere; }
	.tl-ht i { overflow: hidden; max-width: 100%; text-overflow: ellipsis; white-space: nowrap; font: 400 14px/1.3 var(--font-body); color: var(--wp-text-2w); }
	.tl-big { margin-top: 6px; color: var(--wp-text-2w); font: 600 12.5px/1 var(--font-body); }
	.tl-big strong { margin-right: 4px; color: var(--wp-text); font: 700 26px/1 var(--font-display); font-variant-numeric: tabular-nums; }
	.tl-list { display: grid; gap: 2px; margin: 0; padding: 0; list-style: none; }
	.tl-r { display: flex; align-items: center; gap: 10px; width: 100%; padding: 4px 6px; border-radius: 10px; text-align: left; color: var(--wp-text); text-decoration: none; transition: background-color 150ms var(--ease-standard); }
	.tl-n { width: 22px; flex: none; text-align: center; font: 700 12px/1 var(--font-body); color: var(--wp-text); opacity: .5; font-variant-numeric: tabular-nums; }
	.tl-t { display: grid; flex: 1; min-width: 0; }
	.tl-t b, .tl-t i { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.tl-t b { font: 600 14px/1.3 var(--font-body); }
	.tl-t i { font: 400 12.5px/1.3 var(--font-body); color: var(--wp-text-2w); }
	.tl-v { flex: none; padding: 0 6px; font: 700 13.5px/1 var(--font-body); font-variant-numeric: tabular-nums; }
	@media (hover: hover) and (pointer: fine) { .tl-hero:hover, .tl-r:hover { background: var(--wp-hover); } }
	.tl-hero:focus-visible, .tl-r:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
</style>
