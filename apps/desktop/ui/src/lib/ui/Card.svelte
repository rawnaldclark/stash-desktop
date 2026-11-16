<!-- A card in a shelf: a cover, a mosaic or the Likes tile, a title and a line under it. A link when `href` is given (`onclick` then runs before the router follows it), else a button.
     A card that plays something and has a menu (`song`, or `menu` for a group) is a box with an invisible full-size play button and a "more"
     button on the picture's corner (on hover or focus with a mouse, always on touch), plus right-click, the menu key and a long press. -->
<script lang="ts">
	import type { Artwork, LibraryItem } from '../api';
	import Cover from './Cover.svelte';
	import Icon from './Icon.svelte';
	import { openSongMenu, songMenu, type SongMenuArgs } from './song-menu';
	import Tile from './Tile.svelte';

	let {
		title,
		sub,
		href,
		onclick,
		artwork,
		items,
		liked = false,
		label,
		song,
		menu,
		ini,
		kept = false
	}: {
		title: string;
		sub?: string;
		href?: string;
		onclick?: () => void;
		artwork?: Artwork[];
		items?: LibraryItem[];
		liked?: boolean;
		label?: string;
		/** a song's card: the song menu */
		song?: LibraryItem;
		/** any other menu (a group of songs: `{ item, extra: { group } }`, or `{ load }` for songs fetched when it opens); wins over `song` */
		menu?: SongMenuArgs;
		/** initials for the placeholder when there is no picture */
		ini?: string;
		/** a list kept on this computer: a mark before the line under the title */
		kept?: boolean;
	} = $props();

	const args = $derived<SongMenuArgs>(menu ?? (song ? { item: song } : null));
</script>

{#snippet inner()}
	{#if items || liked}
		<Tile {items} {liked} seed={title} />
	{:else}
		<Cover {artwork} seed={title} min={240} class="cv" {ini} />
	{/if}
	<b>{title}</b>
	{#if sub || kept}<i>{#if kept}<Icon name="download" class="kept-mk" label="On this computer" />{/if}{sub ?? ''}</i>{/if}
{/snippet}

{#if href && args}
	<div class="card card-m" use:songMenu={args}>
		{@render inner()}
		<a class="card-hit" {href} aria-label={label ?? title} {onclick}></a>
		<button type="button" class="more card-more" aria-label="More for {title}" aria-haspopup="menu" onclick={(e) => openSongMenu(args, e.currentTarget)}>
			<Icon name="more_horiz" />
		</button>
	</div>
{:else if href}
	<a class="card" {href} aria-label={label} {onclick}>{@render inner()}</a>
{:else if args}
	<div class="card card-m" use:songMenu={args}>
		{@render inner()}
		<button type="button" class="card-hit" {onclick} aria-label={label ?? title}></button>
		<button type="button" class="more card-more" aria-label="More for {title}" aria-haspopup="menu" onclick={(e) => openSongMenu(args, e.currentTarget)}>
			<Icon name="more_horiz" />
		</button>
	</div>
{:else}
	<button type="button" class="card" {onclick} aria-label={label}>{@render inner()}</button>
{/if}
