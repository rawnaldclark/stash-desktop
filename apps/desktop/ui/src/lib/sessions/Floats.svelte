<!-- The room's reactions, rising over the cover: each a pill as wide as its face, ringed in the sender's colour. A big one is
     larger, sways, and steps between its two frames as it climbs ((°□°) ┬─┬ to (╯°□°)╯︵ ┻━┻). Decoration only: hidden from
     screen readers (the counts under the song say how the room feels). -->
<script lang="ts">
	import { faceFrames } from './faces';
	import type { Float } from './session.svelte';

	let { floats }: { floats: Float[] } = $props();
	/** Big ones show their first frame, then their last, half-way up. */
	let flipped = $state(new Set<number>());
	const timed = new Set<number>();

	$effect(() => {
		for (const f of floats) {
			if (!f.big || timed.has(f.id)) continue;
			const id = f.id;
			timed.add(id);
			setTimeout(() => (flipped = new Set([...flipped, id])), 1_100);
		}
	});
	const frame = (f: Float) => {
		const fr = faceFrames(f.face) ?? [];
		return (f.big && !flipped.has(f.id) ? fr[0] : fr.at(-1)) ?? '';
	};
</script>

<div class="ss-floats" aria-hidden="true">
	{#each floats as f (f.id)}
		<span class="ss-float" class:big={f.big} style="left: {f.x}%; --ring: {f.color}">{frame(f)}</span>
	{/each}
</div>
