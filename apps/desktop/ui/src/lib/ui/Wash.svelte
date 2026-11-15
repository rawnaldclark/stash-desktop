<!-- The album-colour wash behind the views. Layers cross-fade by opacity only (the literal colours live in each layer, not in --np-*). -->
<script lang="ts">
	import { untrack } from 'svelte';
	import type { Palette } from './palette';

	let { palette }: { palette: Palette } = $props();
	let layers = $state<{ id: number; p: Palette }[]>(untrack(() => [{ id: 0, p: palette }]));
	let seq = 0;

	$effect(() => {
		const p = palette;
		untrack(() => {
			if (layers[layers.length - 1]?.p.key === p.key) return;
			const id = ++seq;
			layers = [...layers, { id, p }];
			setTimeout(() => (layers = layers.filter((l) => l.id >= id)), 900);
		});
	});
</script>

<div class="wash" aria-hidden="true">
	{#each layers as l (l.id)}
		<i class="wsh" style="--wd:{l.p.dom};--wv:{l.p.vib};--wm:{l.p.mut}"></i>
	{/each}
</div>
