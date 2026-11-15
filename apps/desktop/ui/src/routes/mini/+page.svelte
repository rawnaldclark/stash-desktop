<!-- The mini player: a small window of its own that stays above the others (the desktop shell opens it at `index.html?view=mini`).
     The song, its cover, where it is, and the controls; drawn tall, the cover fills it. It has no frame: its background moves it
     (`data-tauri-drag-region`, the one thing this window may ask of the shell), and Escape or the corner button brings the full
     window back. -->
<script lang="ts">
	import { player } from '../../lib/app';
	import * as core from '../../lib/core';
	import Cover from '../../lib/ui/Cover.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import LikeButton from '../../lib/ui/LikeButton.svelte';
	import ProgressBar from '../../lib/ui/ProgressBar.svelte';
	import { S } from '../../lib/ui/state.svelte';

	const s = $derived(S.v);
	const playLabel = $derived(s.playing ? 'Pause' : 'Play');
	const full = () => void core.showView('full').catch(() => {});

	$effect(() => {
		document.title = s.current ? `${s.current.title} · ${s.current.artist} | Stash` : 'Stash';
	});
</script>

<svelte:window
	onkeydown={(e) => {
		if (e.key === 'Escape') {
			e.preventDefault();
			full();
		}
	}}
/>

<div class="mini" data-tauri-drag-region>
	<span class="mini-cv" data-tauri-drag-region>
		<Cover artwork={s.current?.artwork} seed={s.current?.title ?? 'stash'} min={480} class="cv" eager />
	</span>
	<p class="mini-t" data-tauri-drag-region>
		{#if s.current}
			<b dir="auto">{s.current.title}</b><i dir="auto">{s.current.artist}</i>
		{:else}
			<b>Nothing playing</b><i>Play something in Stash</i>
		{/if}
	</p>
	<div class="mini-c" role="group" aria-label="Playback">
		<button type="button" class="bar-ib" aria-label="Previous" title="Previous" disabled={!s.current} onclick={() => player.prev()}><Icon name="skip_previous" /></button>
		<button type="button" class="play" class:busy={s.buffering} aria-label={playLabel} title={playLabel} disabled={!s.current} onclick={() => player.toggle()}>
			<Icon name={s.playing ? 'pause' : 'play_arrow'} />
		</button>
		<button type="button" class="bar-ib" aria-label="Next" title="Next" disabled={!s.current} onclick={() => player.next()}><Icon name="skip_next" /></button>
		{#if s.current}<LikeButton item={s.current} class="bar-ib mini-like" />{/if}
	</div>
	<button type="button" class="bar-ib mini-x" aria-label="Back to the full window" title="Back to the full window (Esc)" onclick={full}><Icon name="open_in_full" /></button>
	{#if s.current}<div class="mini-p"><ProgressBar /></div>{/if}
</div>

<style>
	.mini {
		position: absolute;
		inset: 0;
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		grid-template-rows: minmax(0, 1fr) auto auto;
		grid-template-areas: 'cv t x' 'cv c c' 'cv p p';
		column-gap: 12px;
		padding: 10px 10px 8px;
		overflow: hidden;
		user-select: none;
		background: linear-gradient(120deg, color-mix(in srgb, var(--np-dom) 60%, var(--wp-main)), var(--wp-main) 75%);
		box-shadow: inset 0 0 0 1px var(--wp-ring);
	}
	.mini-cv {
		grid-area: cv;
		align-self: center;
		height: min(calc(100vh - 18px), 160px);
		aspect-ratio: 1;
		border-radius: 8px;
		overflow: hidden;
		background: var(--wp-field);
	}
	.mini-cv :global(.cv) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
		pointer-events: none;
	}
	.mini-t {
		grid-area: t;
		display: grid;
		align-content: end;
		min-width: 0;
		padding-top: 2px;
	}
	.mini-t b,
	.mini-t i {
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
		pointer-events: none;
	}
	.mini-t b {
		font: 600 14px/1.3 var(--font-body);
		color: var(--wp-text);
	}
	.mini-t i {
		font: 400 13px/1.3 var(--font-body);
		color: var(--wp-text-2w);
	}
	.mini-c {
		grid-area: c;
		display: flex;
		align-items: center;
		gap: 6px;
		padding-top: 4px;
	}
	.mini-c .play {
		width: 32px;
		height: 32px;
	}
	.mini-c :global(.mini-like) {
		margin-left: auto;
	}
	.mini-x {
		grid-area: x;
		align-self: start;
		margin: -4px -4px 0 0;
	}
	.mini-p {
		grid-area: p;
		padding-top: 2px;
	}
	/* Room for the times only when the window is wide. */
	.mini-p :global(.prg) {
		grid-template-columns: 0 minmax(0, 1fr) 0;
		column-gap: 0;
	}
	.mini-p :global(.tm) {
		visibility: hidden;
	}
	@media (min-width: 520px) {
		.mini-p :global(.prg) {
			grid-template-columns: 36px minmax(0, 1fr) 36px;
			column-gap: 8px;
		}
		.mini-p :global(.tm) {
			visibility: visible;
		}
	}

	/* Drawn tall (the listener made the window so): the cover fills it, the song and the controls lie over its foot. */
	@media (min-height: 220px) {
		.mini {
			grid-template-columns: minmax(0, 1fr) auto;
			grid-template-rows: minmax(0, 1fr) auto auto auto;
			grid-template-areas: 'x x' 't t' 'c c' 'p p';
			padding: 14px;
		}
		.mini-cv {
			position: absolute;
			inset: 0;
			height: auto;
			aspect-ratio: auto;
			border-radius: 0;
			z-index: 0;
		}
		.mini-cv::after {
			content: '';
			position: absolute;
			inset: 0;
			background: linear-gradient(to bottom, rgb(0 0 0 / 0.35), transparent 30%, transparent 45%, rgb(0 0 0 / 0.8));
		}
		.mini-t,
		.mini-c,
		.mini-x,
		.mini-p {
			position: relative;
			z-index: 1;
		}
		.mini-x {
			justify-self: end;
			margin: 0;
		}
		.mini-t b {
			font-size: 16px;
			color: #fff;
		}
		.mini-t i {
			color: rgb(255 255 255 / 0.8);
		}
		/* Over the cover the buttons are white, whatever the theme. */
		.mini :global(.bar-ib) {
			color: rgb(255 255 255 / 0.85);
		}
		.mini-p :global(.prg) {
			color: rgb(255 255 255 / 0.8);
		}
		.mini-c {
			justify-content: center;
			gap: 12px;
		}
		.mini-c .play {
			width: 44px;
			height: 44px;
		}
		.mini-c :global(.mini-like) {
			margin-left: 0;
		}
	}
</style>
