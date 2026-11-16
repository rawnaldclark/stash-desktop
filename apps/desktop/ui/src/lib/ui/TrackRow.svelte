<!-- One song in a list: play (the row), like, and the "more" menu. Right-click, the menu key and a long press open the same menu. -->
<script lang="ts">
	import type { LibraryItem } from '../api';
	import { albumHref, albumLink, artistHref, artistLink, sameCopy } from '../identity';
	import Cover from './Cover.svelte';
	import Icon from './Icon.svelte';
	import LikeButton from './LikeButton.svelte';
	import { mmss } from './format';
	import { P, pickedMenu, rowLists, type PickList } from './pick.svelte';
	import { openSongMenu, songMenu, type SongMenuArgs } from './song-menu';
	import { L, S, ui, type MenuState } from './state.svelte';
	import { NF } from '../desktop/unfound.svelte';
	import { player } from '../app';

	let {
		item,
		num,
		cover = true,
		album = false,
		handle = false,
		onplay,
		extra = {},
		tag,
		hit,
		pick,
		at
	}: { item: LibraryItem; num?: number; cover?: boolean; album?: boolean; /** a drag handle for reordering (the list wires it with the reorder action) */ handle?: boolean; onplay: () => void; extra?: Partial<MenuState>; /** a small label before the artist (what an add-on says a track's best quality is: "Lossless") */ tag?: string; /** what the row does, when it is not playing the song */ hit?: string; /** the list the row is in, so its songs can be picked (Ctrl or Shift+click, Ctrl+A) */ pick?: PickList; /** the row's place in that list */ at?: number } = $props();
	/** No add-on could find it: drawn dimmed (Library › Not found fixes it). */
	const off = $derived(NF.has(item));
	/** Offline, a song that is not on this computer is drawn dimmed, and is not played. */
	const away = $derived(ui.offline && item.here === false && !hit);
	/** The listener chose not to play it: drawn dimmed; a click asks before it plays this one song by itself. */
	const blocked = $derived(hit ? undefined : L.blockedOf(item));

	/** The row is in a list whose songs can be picked, and is picked. */
	const pickable = $derived(!!pick && at !== undefined && !hit);
	const picked = $derived(pickable && P.has(pick!, at!, item));
	/** A picked song's menu acts on every song picked (two or more), read as it opens. */
	const menuArgs = $derived<SongMenuArgs>(picked && P.count > 1 ? { load: async () => pickedMenu() ?? { item, extra } } : { item, extra });
	let rowEl = $state<HTMLElement>();
	$effect(() => {
		if (rowEl && pick) rowLists.set(rowEl, pick);
	});

	/** Ctrl (Cmd) picks the song or lets it go; Shift picks every song from the last one Ctrl picked (Ctrl+Shift adds them). */
	function pickBy(e: MouseEvent | KeyboardEvent): boolean {
		if (!pickable || !(e.ctrlKey || e.metaKey || e.shiftKey)) return false;
		if (e.shiftKey) void P.range(pick!, at!, e.ctrlKey || e.metaKey);
		else P.toggle(pick!, at!, item);
		return true;
	}
	/** Ctrl+Space (Shift+Space) on the row's button picks it, as a click with the key would; the space's own click is eaten. */
	let spaceTaken = false;
	function hitKeyDown(e: KeyboardEvent) {
		if (e.key !== ' ' || e.repeat || !pickBy(e)) return;
		e.preventDefault();
		spaceTaken = true;
	}
	function hitKeyUp(e: KeyboardEvent) {
		if (e.key !== ' ' || !spaceTaken) return;
		e.preventDefault();
		spaceTaken = false;
	}

	// The same row of the library, not only the same song: the one song kept as two files is two rows, and only the copy playing lights up.
	const current = $derived(!!S.v.current && sameCopy(item, S.v.current));
	const playing = $derived(current && S.v.playing);
	// The names link to the album / artist page only when the song has one (a song of the library); otherwise they stay plain text.
	const artistTo = $derived(artistLink(item));
	const albumTo = $derived(albumLink(item));

	// The names that link away keep their click (the router handles it); anywhere else on the row plays the song.
	function rowClick(e: MouseEvent) {
		if ((e.target as Element | null)?.closest('a')) return;
		if (pickBy(e)) return;
		// A plain click plays the song, as it always does, and lets the picked songs go.
		P.clear();
		if (away) {
			ui.toast('Not on this computer, and Stash is offline');
			return;
		}
		if (blocked) {
			const why = blocked === 'artist' ? `${item.artist}’s songs` : `“${item.title}”`;
			ui.toast(`You chose not to play ${why}`, { label: 'Play it', run: () => void player.playItems([item], 0) });
			return;
		}
		onplay();
	}
</script>

<div class="trk-row" class:cur={current} class:off class:away class:blk={!!blocked} class:pk={picked} class:has-num={num !== undefined} class:has-cv={cover} class:has-al={album} class:has-rd={handle} use:songMenu={menuArgs} bind:this={rowEl} role="presentation" title={off ? 'Not found on your add-ons' : away ? 'Not on this computer: Stash is offline' : blocked ? 'You chose not to play this' : undefined}>
	{#if handle}<button type="button" class="rd-h" aria-label="Reorder {item.title}" title="Drag to reorder, or press Arrow Up or Down" aria-keyshortcuts="ArrowUp ArrowDown"><Icon name="drag_indicator" /></button>{/if}
	<!-- A click anywhere on the row plays it (except on the names that link away); the button inside is the keyboard and screen-reader way in: its Enter or Space click bubbles to the row. -->
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- A click with Shift or Ctrl picks songs: the text between is not selected. -->
	<div class="trk-main" role="presentation" onclick={rowClick} onmousedown={(e) => pickable && (e.shiftKey || e.ctrlKey || e.metaKey) && e.preventDefault()}>
		<button type="button" class="trk-hit" onkeydown={hitKeyDown} onkeyup={hitKeyUp} aria-label={hit ?? `Play ${item.title} by ${item.artist}${off ? ' (not found on your add-ons)' : away ? ' (not on this computer)' : blocked ? ' (you chose not to play it)' : ''}${picked ? ' (selected)' : ''}`}></button>
		{#if num !== undefined}
			<span class="tr-n">{#if current}<i class="eq" class:still={!playing} aria-hidden="true"><b></b><b></b><b></b></i>{:else}{num}{/if}</span>
		{/if}
		{#if cover}<Cover artwork={item.artwork} seed={item.title} min={80} class="cv trk-cv" />{/if}
		<span class="tr-t"><b>{item.title}</b><i>{#if tag}<span class="fmt tr-tag">{tag}</span>{/if}{#if artistTo}<a class="nl" href={artistHref(artistTo)}>{item.artist}</a>{:else}{item.artist}{/if}</i></span>
		{#if album}<span class="tr-al">{#if albumTo && item.album}<a class="nl" href={albumHref(albumTo)}>{item.album}</a>{:else}{item.album ?? ''}{/if}</span>{/if}
		<span class="tr-d">{item.durationMs ? mmss(item.durationMs) : ''}</span>
	</div>
	<span class="trk-acts">
		<LikeButton {item} class="tr-h" />
		<button type="button" class="more" aria-label="More for {item.title}" aria-haspopup="menu" onclick={(e) => menuArgs && openSongMenu(menuArgs, e.currentTarget)}>
			<Icon name="more_horiz" />
		</button>
	</span>
</div>
