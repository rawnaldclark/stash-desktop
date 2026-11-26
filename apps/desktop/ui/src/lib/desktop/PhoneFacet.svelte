<!-- One of the phone songs' filters with many choices (artists, albums, playlists): a chip that opens a list to tick
     them in, with a box to find one. None ticked is any. -->
<script lang="ts">
	import { tick } from 'svelte';
	import Icon from '../ui/Icon.svelte';
	import type { Facet } from './phone-songs';

	let { label, facets, chosen = $bindable() }: { label: string; facets: Facet[]; chosen: string[] } = $props();

	let open = $state(false);
	let find = $state('');
	let box = $state<HTMLElement>();
	let field = $state<HTMLInputElement>();
	/** The list opens to the left of its chip when there is no room for it to the right. */
	let leftward = $state(false);

	const fold = (t: string) => t.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();
	const shown = $derived(find.trim() ? facets.filter((f) => fold(f.label).includes(fold(find.trim()))) : facets);
	/** Long lists show their first few hundred: finding one is quicker than scrolling. */
	const MOST = 300;
	const WIDE = 300;

	async function toggleOpen() {
		open = !open;
		if (open) {
			find = '';
			const at = box?.getBoundingClientRect();
			const room = (box?.closest('.main') ?? document.documentElement).getBoundingClientRect();
			leftward = !!at && at.left + WIDE > room.right - 8;
			await tick();
			// The page stays where it is: the list is placed to be seen.
			field?.focus({ preventScroll: true });
		}
	}

	function flip(value: string) {
		chosen = chosen.includes(value) ? chosen.filter((v) => v !== value) : [...chosen, value];
	}

	function away(e: PointerEvent) {
		if (open && box && !box.contains(e.target as Node)) open = false;
	}

	function key(e: KeyboardEvent) {
		if (open && e.key === 'Escape') {
			e.stopPropagation();
			open = false;
			box?.querySelector<HTMLElement>('.chip')?.focus();
		}
	}
</script>

<svelte:window onpointerdown={away} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="phf" bind:this={box} onkeydown={key}>
	<button type="button" class="chip" class:on={chosen.length > 0} aria-expanded={open} onclick={toggleOpen}>
		{label}{chosen.length ? ` · ${chosen.length}` : ''}<Icon name="expand_more" />
	</button>
	{#if open}
		<div class="phf-pop" class:phf-left={leftward} style:width="{WIDE}px" role="group" aria-label={label}>
			<label class="phf-find"><Icon name="search" /><input type="search" placeholder="Find" aria-label="Find in {label.toLowerCase()}" bind:value={find} bind:this={field} maxlength="200" /></label>
			<ul class="phf-list">
				{#each shown.slice(0, MOST) as f (f.value)}
					<li>
						<label class="phf-r">
							<input type="checkbox" checked={chosen.includes(f.value)} onchange={() => flip(f.value)} />
							<span class="phf-l">{f.label}</span>
							<span class="phf-n">{f.count.toLocaleString()}</span>
						</label>
					</li>
				{:else}
					<li class="phf-none">None found</li>
				{/each}
				{#if shown.length > MOST}<li class="phf-none">{(shown.length - MOST).toLocaleString()} more: find one by name</li>{/if}
			</ul>
			{#if chosen.length}
				<button type="button" class="phf-clear" onclick={() => (chosen = [])}>Any {label.toLowerCase()}</button>
			{/if}
		</div>
	{/if}
</div>

<style>
	.phf { position: relative; }
	.phf .chip :global(svg) { width: 16px; height: 16px; margin-right: -6px; fill: currentColor; }
	.phf-pop { position: absolute; z-index: 30; top: calc(100% + 6px); left: 0; max-width: calc(100vw - 32px); display: grid; gap: 8px; padding: 10px; border-radius: 14px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); }
	.phf-left { left: auto; right: 0; }
	.phf-find { display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 10px; border-radius: 10px; background: var(--wp-hover); color: var(--wp-text-2); }
	.phf-find :global(svg) { width: 18px; height: 18px; fill: currentColor; flex: none; }
	.phf-find input { flex: 1; min-width: 0; background: none; border: 0; outline: none; color: var(--wp-text); font: 400 13.5px/1 var(--font-body); }
	.phf-list { margin: 0; padding: 0; list-style: none; max-height: 320px; overflow-y: auto; }
	.phf-r { display: grid; grid-template-columns: 18px minmax(0, 1fr) auto; align-items: center; gap: 10px; min-height: 34px; padding: 2px 6px; border-radius: 8px; cursor: pointer; font: 400 13.5px/1.3 var(--font-body); color: var(--wp-text); }
	.phf-r:hover { background: var(--wp-hover); }
	.phf-r input { width: 16px; height: 16px; accent-color: var(--acc); }
	.phf-l { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.phf-n { color: var(--wp-text-2); font-size: 12.5px; font-variant-numeric: tabular-nums; }
	.phf-none { padding: 8px 6px; color: var(--wp-text-2); font: 400 13px/1.3 var(--font-body); }
	.phf-clear { justify-self: start; height: 30px; padding: 0 10px; border-radius: 8px; color: var(--acc-text); font: 600 13px/1 var(--font-body); }
	.phf-clear:hover { background: var(--wp-hover); }
</style>
