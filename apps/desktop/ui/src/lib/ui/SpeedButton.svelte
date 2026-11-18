<!-- Playback speed and Tuning, as Android's speed sheet has them: speeds from 0.25x to 2x (or any from 0.1x to 4x), and
     the A songs are heard at (440 as recorded, 432, 444, or any from 392 to 494 Hz). Speed keeps the pitch; Tuning keeps
     the speed. The button says what is not as recorded ("1.25x", "A432"). -->
<script lang="ts">
	import { player } from '../app';
	import Icon from './Icon.svelte';
	import { S } from './state.svelte';
	import { SPEEDS, TUNINGS, MIN_HZ, MAX_HZ, MIN_SPEED, MAX_SPEED, speedLabel, speedMark, tuningHeading, tuningMark, readSpeed, readTuning } from './speed';

	let { class: cls = 'bar-ib', up = true }: { class?: string; up?: boolean } = $props();
	let open = $state(false);
	let btn = $state<HTMLButtonElement>();
	let panel = $state<HTMLElement>();
	let pos = $state({ x: 0, y: 0 });
	let speedText = $state('');
	let hzText = $state('');

	const speed = $derived(S.v.speed ?? 1);
	const hz = $derived(S.v.tuning ?? 440);
	const mark = $derived([speedMark(speed), tuningMark(hz)].filter(Boolean).join(' · '));
	const customSpeed = $derived(!SPEEDS.includes(speed));
	const customHz = $derived(!TUNINGS.includes(hz));
	const speedEntry = $derived(readSpeed(speedText));
	const hzEntry = $derived(readTuning(hzText));

	/** The panel lives beside the menus, in the window's frame (its colours are set there): the player bar clips what is
	 *  inside it, and makes `fixed` its own. */
	function above(node: HTMLElement) {
		(btn?.closest('.wp-frame') ?? document.body).appendChild(node);
		return { destroy: () => node.remove() };
	}
	function place() {
		if (!btn || !panel) return;
		const b = btn.getBoundingClientRect();
		const p = panel.getBoundingClientRect();
		const x = Math.max(8, Math.min(b.right - p.width, window.innerWidth - p.width - 8));
		const y = up ? b.top - p.height - 8 : b.bottom + 8;
		pos = { x, y: Math.max(8, Math.min(y, window.innerHeight - p.height - 8)) };
	}
	function toggle(e: MouseEvent) {
		e.stopPropagation();
		open = !open;
		if (open) {
			speedText = customSpeed ? String(speed) : '';
			hzText = customHz ? String(hz) : '';
		}
	}
	function close(back = false) {
		open = false;
		if (back) btn?.focus();
	}
	$effect(() => {
		if (!open || !panel) return;
		place();
		(panel.querySelector<HTMLElement>('[aria-pressed="true"]') ?? panel.querySelector<HTMLElement>('button'))?.focus();
		const outside = (e: PointerEvent) => {
			if (!panel?.contains(e.target as Node) && !btn?.contains(e.target as Node)) close();
		};
		const keys = (e: KeyboardEvent) => {
			if (e.key === 'Escape') {
				e.stopPropagation();
				close(true);
			}
		};
		const again = () => place();
		document.addEventListener('pointerdown', outside, true);
		document.addEventListener('keydown', keys, true);
		window.addEventListener('resize', again);
		return () => {
			document.removeEventListener('pointerdown', outside, true);
			document.removeEventListener('keydown', keys, true);
			window.removeEventListener('resize', again);
		};
	});
	function setSpeedText(e: SubmitEvent) {
		e.preventDefault();
		if (speedEntry.value !== null) player.setSpeed(speedEntry.value);
	}
	function setHzText(e: SubmitEvent) {
		e.preventDefault();
		if (hzEntry.value !== null) player.setTuning(hzEntry.value);
	}
</script>

<button type="button" bind:this={btn} class="{cls} spd-b" class:open class:on={!!mark} aria-label={mark ? `Playback speed: ${mark}` : 'Playback speed'} title={mark ? `Playback speed · ${mark}` : 'Playback speed'} aria-haspopup="dialog" aria-expanded={open} onclick={toggle}>
	<Icon name="speed" />{#if mark}<span class="spd-mk">{mark}</span>{/if}
</button>

{#if open}
	<div class="spd-p" use:above role="dialog" aria-label="Playback speed and tuning" bind:this={panel} style="left: {pos.x}px; top: {pos.y}px">
		<div class="spd-h"><b>Playback speed</b><span>{speedLabel(speed)}</span></div>
		<div class="spd-g" role="group" aria-label="Speed">
			{#each SPEEDS as v (v)}
				<button type="button" class="chip" class:on={speed === v} aria-pressed={speed === v} onclick={() => player.setSpeed(v)}>{speedLabel(v)}</button>
			{/each}
		</div>
		<form class="spd-c" onsubmit={setSpeedText}>
			<label for="spd-in">Custom</label>
			<input id="spd-in" class="dev-input" inputmode="decimal" placeholder="1.1" bind:value={speedText} aria-invalid={speedEntry.wrong} aria-describedby="spd-say" />
			<span class="spd-x">x</span>
			<button type="submit" class="btn btn-q" disabled={speedEntry.value === null}>Set</button>
		</form>
		<p class="spd-say" id="spd-say" class:wrong={speedEntry.wrong}>{speedEntry.wrong ? `A speed from ${MIN_SPEED}x to ${MAX_SPEED}x` : 'The pitch stays as it is.'}</p>

		<div class="spd-h spd-h2"><b>{tuningHeading(hz)}</b></div>
		<div class="spd-g spd-t" role="group" aria-label="Tuning">
			{#each TUNINGS as v (v)}
				<button type="button" class="chip" class:on={hz === v} aria-pressed={hz === v} onclick={() => player.setTuning(v)}>{v}</button>
			{/each}
			{#if customHz}<span class="chip on" aria-current="true">{hz}</span>{/if}
		</div>
		<form class="spd-c" onsubmit={setHzText}>
			<label for="hz-in">Custom</label>
			<input id="hz-in" class="dev-input" inputmode="decimal" placeholder="431.5" bind:value={hzText} aria-invalid={hzEntry.wrong} aria-describedby="hz-say" />
			<span class="spd-x">Hz</span>
			<button type="submit" class="btn btn-q" disabled={hzEntry.value === null}>Set</button>
		</form>
		<p class="spd-say" id="hz-say" class:wrong={hzEntry.wrong}>{hzEntry.wrong ? `Any value from ${MIN_HZ} to ${MAX_HZ} Hz` : 'The speed stays as it is.'}</p>
	</div>
{/if}

<style>
	.spd-b { position: relative; }
	.spd-b.on { color: var(--acc-text); width: auto; padding: 0 8px; gap: 4px; display: inline-flex; align-items: center; }
	.spd-mk { font: 600 11px/1 var(--font-body); white-space: nowrap; }
	.spd-p { position: fixed; z-index: 60; width: 320px; padding: 16px; border-radius: 16px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); color: var(--wp-text); }
	.spd-h { display: flex; justify-content: space-between; align-items: baseline; margin-bottom: 10px; }
	.spd-h b { font: 600 15px/1.2 var(--font-body); }
	.spd-h span { color: var(--wp-text-2); font: 500 13px/1 var(--font-body); }
	.spd-h2 { margin-top: 18px; padding-top: 14px; border-top: 1px solid var(--wp-line-2); }
	.spd-g { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
	.spd-g .chip { justify-content: center; padding: 0 6px; }
	.spd-t { grid-template-columns: repeat(3, 1fr); }
	.spd-c { display: flex; align-items: center; gap: 8px; margin-top: 10px; }
	.spd-c label { color: var(--wp-text-2); font: 500 13px/1 var(--font-body); }
	.spd-c input { flex: 1; min-width: 0; height: 32px; }
	.spd-x { color: var(--wp-text-2); font: 500 13px/1 var(--font-body); }
	.btn-q { height: 32px; padding: 0 12px; }
	.btn-q:disabled { opacity: 0.45; }
	.spd-say { margin: 6px 0 0; color: var(--wp-text-2); font: 400 12px/1.3 var(--font-body); }
	.spd-say.wrong { color: var(--red); }
</style>
