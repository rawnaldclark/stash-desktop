<!-- Elapsed, a seek track, remaining. The fill is one Web Animation (scaleX, compositor) pinned to player.position(); no per-frame JS. -->
<script lang="ts">
	import { untrack } from 'svelte';
	import { player } from '../app';
	import { mmss } from './format';
	import { S } from './state.svelte';

	let { class: cls = '' }: { class?: string } = $props();

	let track = $state<HTMLElement>();
	let fill = $state<HTMLElement>();
	let knob = $state<HTMLElement>();
	let drag = $state<number | null>(null);
	let anims: Animation[] = [];

	const dur = $derived(S.v.durationMs);
	const pos = $derived(drag === null ? S.v.positionMs : drag * dur);

	function pin(now: number, playing: boolean) {
		for (const a of anims) {
			if (playing) {
				a.playbackRate = 1;
				a.startTime = (document.timeline.currentTime as number) - now;
			} else {
				a.pause();
				a.currentTime = now;
			}
		}
	}

	// (Re)build when the song's length changes; reduced motion still moves the fill (it is the position, not decoration).
	$effect(() => {
		const d = dur;
		const f = fill;
		const k = knob;
		if (!f || !k || !(d > 0)) return;
		untrack(() => {
			const opts: KeyframeAnimationOptions = { duration: d, fill: 'both', easing: 'linear' };
			anims = [f.animate([{ transform: 'scaleX(0)' }, { transform: 'scaleX(1)' }], opts), k.animate([{ transform: 'translateX(0)' }, { transform: 'translateX(100%)' }], opts)];
			pin(player.position(), S.v.playing);
		});
		return () => {
			for (const a of anims) a.cancel();
			anims = [];
		};
	});

	// Events: play, pause, seek (the published position moves more than the animation says).
	$effect(() => {
		const playing = S.v.playing;
		const p = S.v.positionMs;
		untrack(() => {
			if (!anims.length || drag !== null) return;
			const ct = Number(anims[0].currentTime ?? 0);
			const now = player.position();
			const stateChanged = (anims[0].playState === 'running') !== playing;
			if (stateChanged || Math.abs(ct - now) > 120 || !playing) pin(now, playing);
			void p;
		});
	});

	function frac(e: PointerEvent) {
		const r = track!.getBoundingClientRect();
		return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
	}
	function down(e: PointerEvent) {
		if (!(dur > 0)) return;
		track!.setPointerCapture(e.pointerId);
		drag = frac(e);
		for (const a of anims) {
			a.pause();
			a.currentTime = drag * dur;
		}
	}
	function move(e: PointerEvent) {
		if (drag === null) return;
		drag = frac(e);
		for (const a of anims) a.currentTime = drag * dur;
	}
	function up(e: PointerEvent) {
		if (drag === null) return;
		const f = frac(e);
		drag = null;
		player.seek(f * dur);
		pin(f * dur, S.v.playing);
	}
	function key(e: KeyboardEvent) {
		const step = e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? -5000 : e.key === 'ArrowRight' || e.key === 'ArrowUp' ? 5000 : e.key === 'PageDown' ? -30000 : e.key === 'PageUp' ? 30000 : 0;
		if (e.key === 'Home') player.seek(0);
		else if (e.key === 'End') player.seek(Math.max(0, dur - 1000));
		else if (step) player.seek(Math.max(0, Math.min(dur, player.position() + step)));
		else return;
		e.preventDefault();
		e.stopPropagation();
	}
</script>

<div class="prg {cls}">
	<span class="tm tm-e">{mmss(pos)}</span>
	<div
		class="trk"
		class:drag={drag !== null}
		bind:this={track}
		role="slider"
		tabindex="0"
		aria-label="Seek"
		aria-valuemin="0"
		aria-valuemax={Math.round(dur / 1000)}
		aria-valuenow={Math.round(pos / 1000)}
		aria-valuetext="{mmss(pos)} of {mmss(dur)}"
		onpointerdown={down}
		onpointermove={move}
		onpointerup={up}
		onpointercancel={() => (drag = null)}
		onkeydown={key}
	>
		<i class="trk-bg"></i>
		<i class="pf" bind:this={fill}></i>
		<div class="knob-w"><i class="pd" bind:this={knob}></i></div>
	</div>
	<span class="tm tm-r">-{mmss(Math.max(0, dur - pos))}</span>
</div>
