<!-- Volume: a mute toggle and a slider (arrow keys, Home/End); the mouse wheel over either adjusts it. One `setVolume` state, so the
     bar's and Now playing's stay in step. `cls` styles the mute button like its neighbours. -->
<script lang="ts">
	import { player } from '../app';
	import Icon from './Icon.svelte';
	import { S } from './state.svelte';

	let { cls = 'bar-ib', class: wrap = '' }: { cls?: string; class?: string } = $props();
	const v = $derived(S.v.volume);
	// the level a mute goes back to
	let back = 0.7;
	const set = (x: number) => player.setVolume(Math.round(Math.max(0, Math.min(1, x)) * 100) / 100);
	// A wheel over the control turns it (not the page): a non-passive listener, so the scroll can be kept from the page.
	function wheel(node: HTMLElement) {
		const on = (e: WheelEvent) => {
			if (!e.deltaY) return;
			e.preventDefault();
			set(S.v.volume + (e.deltaY < 0 ? 0.05 : -0.05));
		};
		node.addEventListener('wheel', on, { passive: false });
		return { destroy: () => node.removeEventListener('wheel', on) };
	}
</script>

<span class="vol {wrap}" use:wheel>
	<button type="button" class={cls} aria-label={v === 0 ? 'Unmute' : 'Mute'} title={v === 0 ? 'Unmute' : 'Mute'} onclick={() => { if (v === 0) set(back || 0.7); else { back = v; set(0); } }}><Icon name={v === 0 ? 'volume_off' : 'volume_up'} /></button>
	<input class="vol-s" type="range" min="0" max="1" step="0.01" value={v} aria-label="Volume" aria-valuetext="{Math.round(v * 100)}%" oninput={(e) => set(Number(e.currentTarget.value))} style="--v:{v * 100}%" />
</span>
