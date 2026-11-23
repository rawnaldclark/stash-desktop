<!-- The bar shown while songs are picked in a list (Ctrl or Shift+click, Ctrl+A): how many, Play, Add to playlist, the menu for
     all of them, and Clear. Escape lets them go, and so does going to another page. Ctrl+A picks the whole list: the one
     holding the focus, the one songs are picked in, or the page's first. -->
<script lang="ts">
	import { afterNavigate } from '$app/navigation';
	import { library, player } from '../app';
	import Icon from './Icon.svelte';
	import { P, pickedMenu, rowLists, type PickList } from './pick.svelte';
	import { ui } from './state.svelte';

	const shown = $derived(P.count > 0 && P.list !== null);

	afterNavigate(() => P.clear());

	// Toasts rise above the bar while it shows.
	$effect(() => {
		const root = document.documentElement;
		root.style.setProperty('--pick-clear', shown ? '60px' : '0px');
		return () => root.style.removeProperty('--pick-clear');
	});

	/** The list Ctrl+A picks: the one with the focus, the one songs are picked in, or the first on the page. */
	function listForAll(): PickList | null {
		const row = (document.activeElement as HTMLElement | null)?.closest('.trk-row');
		const focused = row ? rowLists.get(row) : undefined;
		if (focused) return focused;
		if (P.list) return P.list;
		for (const el of document.querySelectorAll('#main .trk-row')) {
			const list = rowLists.get(el);
			if (list) return list;
		}
		return null;
	}

	function onkey(e: KeyboardEvent) {
		if (e.defaultPrevented || ui.menu || ui.palette || ui.smart || ui.deleting) return;
		const t = e.target as HTMLElement | null;
		if (t?.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])')) return;
		if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'a') {
			const list = listForAll();
			if (!list) return;
			e.preventDefault();
			void P.all(list);
		} else if (e.key === 'Escape' && shown) {
			e.preventDefault();
			P.clear();
		} else if (e.key === 'Delete' && shown && !e.ctrlKey && !e.altKey && !e.metaKey) {
			e.preventDefault();
			void remove();
		}
	}

	function play() {
		const list = P.list;
		if (!list) return;
		void player.playItems(P.sorted().items, 0, { from: list.from });
	}
	/** Delete: the question first, about the picked songs that are in the library. */
	async function remove() {
		const { items } = P.sorted();
		if (!items.length) return;
		const ids = await library.songRows(items).catch(() => null);
		if (ids === null) return;
		if (!ids.length) {
			ui.toast(items.length === 1 ? 'That song is not in your library' : 'None of those are in your library');
			return;
		}
		ui.deleting = { what: { ids }, title: items.length === 1 ? items[0].title : '' };
	}
	function toPlaylist(btn: HTMLElement) {
		const menu = pickedMenu();
		if (!menu) return;
		const r = btn.getBoundingClientRect();
		ui.openMenu({ ...menu.extra, x: r.left, y: r.top - 8, item: menu.item, anchor: btn, view: 'playlists', up: true });
	}
	function more(btn: HTMLElement) {
		const menu = pickedMenu();
		if (!menu) return;
		const r = btn.getBoundingClientRect();
		ui.openMenu({ ...menu.extra, x: r.left, y: r.top - 8, item: menu.item, anchor: btn, up: true });
	}
</script>

<svelte:window onkeydown={onkey} />

{#if shown}
	<div class="pick-wrap">
		<div class="pick-bar" role="toolbar" aria-label="Selected songs">
			<span class="pick-n" aria-live="polite">{P.count.toLocaleString()} selected</span>
			<button type="button" class="pick-b" onclick={play}><Icon name="play_arrow" /><span>Play</span></button>
			<button type="button" class="pick-b" aria-haspopup="menu" onclick={(e) => toPlaylist(e.currentTarget)}><Icon name="playlist_add" /><span>Add to playlist</span></button>
			<button type="button" class="pick-b pick-del" title="Delete from your library (Del)" onclick={() => void remove()}><Icon name="delete" /><span>Delete</span></button>
			<button type="button" class="pick-b pick-i" aria-label="More for the selected songs" aria-haspopup="menu" title="More" onclick={(e) => more(e.currentTarget)}><Icon name="more_horiz" /></button>
			<button type="button" class="pick-b pick-i" aria-label="Clear the selection" title="Clear (Esc)" onclick={() => P.clear()}><Icon name="close" /></button>
		</div>
	</div>
{/if}

<style>
	.pick-wrap { position: fixed; z-index: 45; left: 0; right: 0; bottom: calc(var(--bar-clear, 100px) + 16px); display: flex; justify-content: center; pointer-events: none; }
	.pick-bar { pointer-events: auto; display: flex; align-items: center; gap: 2px; max-width: calc(100vw - 32px); padding: 4px 4px 4px 16px; border-radius: 999px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); font: 500 13px/1 var(--font-body); color: var(--wp-text); animation: pick-in 180ms var(--ease-out-expo) both; }
	@keyframes pick-in { from { opacity: 0; translate: 0 6px; } }
	@media (prefers-reduced-motion: reduce) { .pick-bar { animation: none; } }
	.pick-n { margin-right: 8px; font-weight: 600; font-variant-numeric: tabular-nums; white-space: nowrap; }
	.pick-b { display: inline-flex; align-items: center; gap: 6px; height: 36px; padding: 0 12px; border-radius: 999px; color: var(--wp-text); white-space: nowrap; transition: background-color var(--dur-short); }
	.pick-b :global(.ic) { width: 20px; height: 20px; flex: none; }
	.pick-i { width: 36px; padding: 0; justify-content: center; color: var(--wp-text-2); }
	.pick-del { color: #d84343; }
	@media (hover: hover) and (pointer: fine) { .pick-b:hover { background: var(--wp-hover); color: var(--wp-text); } }
	@media (max-width: 520px) { .pick-b:not(.pick-i) span { display: none; } .pick-b:not(.pick-i) { width: 36px; padding: 0; justify-content: center; } }
</style>
