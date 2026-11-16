<script lang="ts">
	import '../lib/styles/tokens.css';
	import '../lib/styles/base.css';
	import '../lib/styles/shell.css';
	import '../lib/styles/views.css';
	import '../lib/styles/fullscreen.css';
	import { onMount, type Component } from 'svelte';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { player, problems, readOffline } from '../lib/app';
	import { measuring } from '../lib/core';
	import Header from '../lib/ui/Header.svelte';
	import NowPlaying from '../lib/ui/NowPlaying.svelte';
	import Panel from '../lib/ui/Panel.svelte';
	import PlayerBar from '../lib/ui/PlayerBar.svelte';
	import Sidebar from '../lib/ui/Sidebar.svelte';
	import CommandPalette from '../lib/ui/CommandPalette.svelte';
	import SmartEditor from '../lib/desktop/SmartEditor.svelte';
	import Toast from '../lib/ui/Toast.svelte';
	import PickBar from '../lib/ui/PickBar.svelte';
	import Wash from '../lib/ui/Wash.svelte';
	import { coverOf } from '../lib/ui/format';
	import { NF } from '../lib/desktop/unfound.svelte';
	import { DEFAULT_PALETTE, forScheme, loadPalette, paletteStyle, wordmarkInk, type Palette } from '../lib/ui/palette';
	import { S, ui } from '../lib/ui/state.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	let header = $state<{ focusSearch(): void }>();
	/** The song menu: its own chunk, fetched when the window is idle (or the first time a menu opens, if that is sooner). */
	let ContextMenu = $state<Component | null>(null);
	// A load cut short (a navigation) is retried by the next open; it must not surface as an unhandled rejection.
	const loadMenu = () => void import('../lib/ui/ContextMenu.svelte').then((m) => (ContextMenu = m.default as Component)).catch(() => {});
	$effect(() => {
		if (ui.menu && !ContextMenu) loadMenu();
	});
	let palette = $state<Palette>(DEFAULT_PALETTE);
	let mainEl = $state<HTMLElement>();
	// The scrolling column is <main>, not the window, so Kit's own scroll reset does not reach it: a new page starts at the top.
	afterNavigate((nav) => {
		if (nav.type !== 'popstate' && mainEl) mainEl.scrollTop = 0;
	});

	const np = $derived(page.url.pathname === '/now');
	/** The mini player's window: the page alone, none of the full window around it. */
	const mini = $derived(page.url.pathname === '/mini');
	const size = $derived(ui.size);
	const hasBar = $derived(!!S.v.current);
	const panelCollapsed = $derived(!ui.panelOpen && !ui.sheetMode);

	$effect(() => {
		ui.np = np;
		if (np) ui.sheet = false;
	});
	// The header box shows the query only on the Search page (and while the window measures itself, where it filters the list in place).
	$effect(() => {
		if (!measuring && !page.url.pathname.startsWith('/search')) ui.q = '';
	});

	// Album colour: extracted when the cover can be read; otherwise the Stash-purple default stays.
	let token = 0;
	const cover = $derived(coverOf(S.v.current, 200)); // a derived, so the effect below runs per song, not per state tick
	$effect(() => {
		const url = cover;
		const mine = ++token;
		if (!url) {
			palette = DEFAULT_PALETTE;
			return;
		}
		void loadPalette(url).then((p) => {
			if (mine === token) palette = p ?? DEFAULT_PALETTE;
		});
	});

	// The wordmark takes the song's mood colour while it plays (a contrast-checked tint of its plain ink), and its plain ink otherwise.
	const wmInk = $derived(wordmarkInk(S.v.playing && S.v.current ? palette : null, ui.light));

	const docTitle = $derived(S.v.current ? `${S.v.current.title} · ${S.v.current.artist} | Stash` : 'Stash');
	$effect(() => {
		// A measured window names itself when it is done ("Stash (measured)", Library › Songs): the title is left alone.
		if (!measuring) document.title = docTitle;
	});

	// The last song that could not be played, and why: said each time one could not be (the same message twice too).
	let lastError: string | null = null;
	let lastErrors = 0;
	$effect(() => {
		const e = S.v.error;
		const n = S.v.errors ?? 0;
		if (e && (e !== lastError || n !== lastErrors)) {
			const said = /[.!?]$/.test(e) ? e : `${e}.`;
			ui.toast(said);
			// A song that could not be played may now be known not to play: then the message offers to find it.
			const id = S.v.errorId;
			void NF.refresh().then(() => {
				if (id == null || !NF.marked.has(id) || ui.toastText !== said) return;
				ui.toast(said, { label: 'Find it', run: () => void NF.show(id) });
			});
		}
		lastError = e;
		lastErrors = n;
	});
	// The core has no audio engine: said once, with its reason (Settings › Sound keeps saying it).
	let saidNoSound = false;
	$effect(() => {
		if (S.v.available || saidNoSound) return;
		saidNoSound = true;
		ui.toast(`No sound: ${S.v.reason ?? 'the player is not available'}.`);
	});
	// Something the listener asked for was refused: the core's own words.
	$effect(() =>
		problems.subscribe((p) => {
			if (p) ui.toast(p.text);
		})
	);

	onMount(() => {
		if (!measuring) void NF.refresh();
		void readOffline();
		// Measuring: the whole library's list is what is measured.
		if (measuring && !(page.url.pathname === '/library' && page.url.searchParams.get('tab') === 'songs')) void goto('/library?tab=songs', { replaceState: true });
		const idle = (window as Window & { requestIdleCallback?: (f: () => void, o?: { timeout: number }) => number }).requestIdleCallback;
		const later = () => {
			void import('../lib/ui/UpNext.svelte').catch(() => {}); // warm: the panel's Up next opens without a wait
			if (!ContextMenu) loadMenu();
		};
		if (!measuring) {
			if (idle) idle(later, { timeout: 4000 });
			else setTimeout(later, 1500);
		}
		const root = document.querySelector('.wp') as HTMLElement;
		const ro = new ResizeObserver(() => {
			ui.w = root.clientWidth;
			ui.h = root.clientHeight;
		});
		ro.observe(root);
		ui.w = root.clientWidth;
		ui.h = root.clientHeight;
		const rm = matchMedia('(prefers-reduced-motion: reduce)');
		const lt = matchMedia('(prefers-color-scheme: light)');
		const sync = () => {
			ui.reduced = rm.matches;
			ui.systemLight = lt.matches;
		};
		sync();
		rm.addEventListener('change', sync);
		lt.addEventListener('change', sync);
		return () => {
			ro.disconnect();
			rm.removeEventListener('change', sync);
			lt.removeEventListener('change', sync);
		};
	});

	function typing(t: EventTarget | null) {
		const el = t as HTMLElement | null;
		if (!el) return false;
		const tag = el.tagName;
		return tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable || (tag === 'INPUT' && !['range', 'checkbox', 'radio', 'button'].includes((el as HTMLInputElement).type));
	}
	function interactive(t: EventTarget | null) {
		const el = t as HTMLElement | null;
		if (!el) return false;
		return ['BUTTON', 'A', 'INPUT', 'SUMMARY', 'SELECT'].includes(el.tagName) || ['slider', 'menuitem', 'tab', 'switch', 'checkbox'].includes(el.getAttribute('role') ?? '');
	}

	function onkey(e: KeyboardEvent) {
		// Ctrl+K (Cmd+K on a Mac): the command palette, from anywhere.
		if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k' && !mini) {
			e.preventDefault();
			ui.palette = !ui.palette;
			return;
		}
		if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
		if (e.key === '/' && !typing(e.target)) {
			e.preventDefault();
			header?.focusSearch();
			return;
		}
		if (typing(e.target) || ui.menu) return;
		if (e.key === ' ' || e.code === 'Space') {
			if (interactive(e.target)) return;
			e.preventDefault();
			player.toggle();
		} else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !interactive(e.target)) {
			e.preventDefault();
			if (e.shiftKey) {
				if (e.key === 'ArrowRight') player.next();
				else player.prev();
			} else player.seek(Math.max(0, Math.min(S.v.durationMs, player.position() + (e.key === 'ArrowRight' ? 5000 : -5000))));
		} else if ((e.key === 'ArrowUp' || e.key === 'ArrowDown') && e.shiftKey) {
			e.preventDefault();
			player.setVolume(Math.max(0, Math.min(1, S.v.volume + (e.key === 'ArrowUp' ? 0.05 : -0.05))));
		}
	}
</script>

<svelte:window onkeydown={onkey} />

{#if mini}
	<div class="wp stage wp-frame" style={paletteStyle(palette, ui.light)}>{@render children()}</div>
{:else}
<div class="wp stage wp-frame" style="{paletteStyle(palette, ui.light)};--wm-ink:{wmInk}" data-size={size}>
	<a class="skip" href="#main">Skip to content</a>
	<div class="wp-g" class:np class:pn-none={panelCollapsed && !np} class:has-bar={hasBar}>
		{#if !np}<Sidebar />{/if}
		{#if !np}<Header bind:this={header} />{/if}
		<main class="main" id="main" tabindex="-1" bind:this={mainEl}>
			<div class="main-in">
				<Wash palette={forScheme(palette, ui.light)} />
				<div class="views">{@render children()}</div>
			</div>
		</main>
		<Panel />
		{#if np}<NowPlaying />{/if}
		{#if !np}<PlayerBar />{/if}
	</div>
	{#if ContextMenu}<ContextMenu />{/if}
	<PickBar />
	<Toast />
	{#if ui.palette}<CommandPalette />{/if}
	{#if ui.smart}<SmartEditor />{/if}
</div>
{/if}
