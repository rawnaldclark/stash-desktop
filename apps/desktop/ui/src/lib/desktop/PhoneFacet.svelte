<!-- One of the phone songs' filters, open: a wide panel under the filter row (not a pop-up), its choices with how many
     songs each has, a box to find one, and Done. Ticking one shows its songs; it never chooses them. None ticked is any. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import Icon from '../ui/Icon.svelte';
	import type { Facet } from './phone-songs';

	let {
		label,
		many,
		find: findHint,
		facets,
		picked = $bindable(),
		onclose
	}: { label: string; many: string; find: string; facets: Facet[]; picked: string[]; onclose: () => void } = $props();

	let find = $state('');
	let field = $state<HTMLInputElement>();

	const fold = (t: string) => t.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();
	const shown = $derived(find.trim() ? facets.filter((f) => fold(f.value).includes(fold(find.trim()))) : facets);
	/** Long lists show their first few hundred: finding one is quicker than scrolling. */
	const MOST = 300;

	// The page stays where it is: the panel opens right under the row it was asked from.
	onMount(() => field?.focus({ preventScroll: true }));

	function flip(value: string) {
		picked = picked.includes(value) ? picked.filter((v) => v !== value) : [...picked, value];
	}

	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.stopPropagation();
			onclose();
		}
	}
</script>

<!-- Escape closes the panel from anywhere inside it (its box to find, its ticks). -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="phf" role="group" aria-label="{label} filter" onkeydown={key}>
	<div class="phf-head">
		<h2 class="phf-title">Show songs from these {many}</h2>
		<label class="phf-find"><Icon name="search" /><input type="search" placeholder={findHint} aria-label={findHint} bind:value={find} bind:this={field} maxlength="200" /></label>
	</div>
	<ul class="phf-list">
		{#each shown.slice(0, MOST) as f (f.value)}
			<li>
				<label class="phf-r" class:on={picked.includes(f.value)}>
					<input type="checkbox" checked={picked.includes(f.value)} onchange={() => flip(f.value)} />
					<span class="phf-l" title={f.value}>{f.value}</span>
					<span class="phf-n">{f.count.toLocaleString()}</span>
				</label>
			</li>
		{:else}
			<li class="phf-none">None found</li>
		{/each}
	</ul>
	<div class="phf-foot">
		<span>{shown.length > MOST ? `${(shown.length - MOST).toLocaleString()} more: find one by name. ` : ''}Ticking shows those songs below; it doesn’t choose them.</span>
		<span class="phf-acts">
			{#if picked.length}<button type="button" class="phf-any" onclick={() => (picked = [])}>Any {many}</button>{/if}
			<button type="button" class="btn" onclick={onclose}>Done</button>
		</span>
	</div>
</div>

<style>
	.phf { margin: 0 0 14px; padding: 14px 14px 12px; border-radius: 16px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); }
	.phf-head { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 12px; margin-bottom: 10px; }
	.phf-title { margin: 0; font: 700 16px/1.2 var(--font-display); color: var(--wp-text); }
	.phf-find { display: flex; align-items: center; gap: 8px; width: 280px; max-width: 100%; height: 36px; padding: 0 12px; border-radius: 12px; background: var(--wp-hover); color: var(--wp-text-2); }
	.phf-find :global(svg) { width: 18px; height: 18px; fill: currentColor; flex: none; }
	.phf-find input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--wp-text); font: 400 13.5px/1 var(--font-body); }
	.phf-find:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
	.phf-list { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 2px 12px; max-height: 228px; margin: 0; padding: 0 4px 0 0; list-style: none; overflow-y: auto; }
	.phf-r { display: grid; grid-template-columns: 16px minmax(0, 1fr) auto; align-items: center; gap: 10px; min-height: 34px; padding: 0 8px; border-radius: 8px; cursor: pointer; font: 400 13.5px/1.3 var(--font-body); color: var(--wp-text); }
	.phf-r:hover { background: var(--wp-hover); }
	.phf-r.on { background: var(--acc-wash); }
	.phf-r input { width: 16px; height: 16px; margin: 0; accent-color: var(--acc); }
	.phf-l { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.phf-n { color: var(--wp-text-2); font-size: 12.5px; font-variant-numeric: tabular-nums; }
	.phf-none { padding: 8px 6px; color: var(--wp-text-2); font: 400 13px/1.3 var(--font-body); }
	.phf-foot { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 8px 12px; margin-top: 10px; color: var(--wp-text-2); font: 400 13px/1.35 var(--font-body); }
	.phf-acts { display: flex; align-items: center; gap: 8px; margin-left: auto; }
	.phf-any { height: 32px; padding: 0 10px; border-radius: 8px; color: var(--acc-text); font: 600 13px/1 var(--font-body); }
	.phf-any:hover { background: var(--wp-hover); }
</style>
