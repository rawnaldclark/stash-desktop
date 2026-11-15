<!-- The right panel: Lyrics | Up next. Docked on L and M, the right column of the big lyrics, a bottom sheet under compact Now playing. -->
<script lang="ts">
	import type { Component } from 'svelte';
	import type { QueueItem } from '../api';
	import Icon from './Icon.svelte';
	import { S, ui } from './state.svelte';

	let Lyrics = $state<Component<{ item: QueueItem }> | null>(null);
	/** Up next (with drag and swipe): its own chunk, fetched when the window is idle (the layout) or the first time it shows. */
	let Next = $state<Component | null>(null);

	const cur = $derived(S.v.current);
	const np = $derived(ui.np);
	const sheetMode = $derived(ui.sheetMode);
	const sheetOpen = $derived(sheetMode && ui.sheet);
	const closed = $derived(!sheetMode && !np && !ui.panelOpen);
	const showUpNext = $derived(sheetMode ? ui.sheet : ui.panelTab === 'next');
	const lyricsOn = $derived(!sheetMode && ui.panelOpen && ui.panelTab === 'lyrics');
	/** Where the lyric column is shown: docked (not Now playing), the right column (Now playing, wide), or the middle of compact Now playing. */
	const lyricsShown = $derived(!!cur && (np ? (ui.twoCol ? ui.panelTab === 'lyrics' : true) : lyricsOn));

	$effect(() => {
		// a load cut short (a navigation) must not surface as an unhandled rejection; the next time lyrics show it is asked again
		if (lyricsShown && !Lyrics) void import('./lyrics/LyricsColumn.svelte').then((m) => (Lyrics = m.default as Component<{ item: QueueItem }>)).catch(() => {});
	});

	$effect(() => {
		if (showUpNext && !Next) void import('./UpNext.svelte').then((m) => (Next = m.default as Component)).catch(() => {});
	});

	// Esc closes the sheet
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape' && sheetOpen) {
			ui.sheet = false;
			e.stopPropagation();
		}
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<aside class="panel" class:sheet-open={sheetOpen} inert={closed || (sheetMode && !sheetOpen)} aria-label="Lyrics and Up next" onkeydown={key}>
	<div class="pn-head">
		{#if sheetMode}
			<h2 class="pn-h">Up next</h2>
		{:else}
			<div class="seg" role="tablist" aria-label="Side panel">
				<button type="button" role="tab" id="tab-lyrics" aria-selected={ui.panelTab === 'lyrics'} aria-controls="pn-body" class="seg-i" onclick={() => ui.setTab('lyrics')}>Lyrics</button>
				<button type="button" role="tab" id="tab-next" aria-selected={ui.panelTab === 'next'} aria-controls="pn-body" class="seg-i" onclick={() => ui.setTab('next')}>Up next</button>
			</div>
		{/if}
		<button type="button" class="pn-close" aria-label="Close panel" title="Close" onclick={() => (sheetMode ? (ui.sheet = false) : (ui.panelOpen = false))}><Icon name="close" /></button>
		<i class="pn-handle" aria-hidden="true"></i>
	</div>
	<div class="pn-body" id="pn-body" role={sheetMode ? undefined : 'tabpanel'} aria-labelledby={sheetMode ? undefined : ui.panelTab === 'lyrics' ? 'tab-lyrics' : 'tab-next'}>
		{#if showUpNext}<div class="pn-queue">{#if Next}{@const N = Next}<N />{/if}</div>{:else if !cur && !sheetMode}<p class="q-empty pn-idle">Lyrics show up here while a song plays.</p>{/if}
	</div>
</aside>

{#if lyricsShown && cur && Lyrics}
	{@const Col = Lyrics}
	<div class="pn-lyrics">
		<p class="pn-title" aria-hidden="true">{cur.title} · {cur.artist}</p>
		{#key cur.qid}
			<Col item={cur} />
		{/key}
	</div>
{/if}
