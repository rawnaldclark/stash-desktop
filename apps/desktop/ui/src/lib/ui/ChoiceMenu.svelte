<!-- A menu of choices (a playlist's, a folder's, the sidebar's "new"): the song menu's look and keys (arrow keys, Home,
     End, Escape; focus goes back where it was). An entry can ask for a name in place (a new folder, a folder's new name). -->
<script lang="ts">
	import { tick } from 'svelte';
	import Icon from './Icon.svelte';
	import { ui, type Choice } from './state.svelte';

	let el = $state<HTMLElement>();
	let pos = $state({ x: 0, y: 0 });
	/** The entry asking for a name, by its place; and the name so far. */
	let asking = $state<number | null>(null);
	let name = $state('');

	const c = $derived(ui.choices);
	const first = () => el?.querySelector<HTMLElement>('[role^="menuitem"]')?.focus();

	$effect(() => {
		if (!c) return;
		asking = null;
		name = '';
		pos = { x: c.x, y: c.y };
		const up = !!c.up;
		tick().then(() => {
			if (!el) return;
			const r = el.getBoundingClientRect();
			pos = {
				x: Math.max(8, Math.min(pos.x, window.innerWidth - r.width - 8)),
				y: Math.max(8, Math.min(up ? pos.y - r.height : pos.y, window.innerHeight - r.height - 8))
			};
			first();
		});
	});
	// Asking for a name makes the menu taller: it stays inside the window.
	$effect(() => {
		void asking;
		tick().then(() => {
			if (!el) return;
			const h = el.getBoundingClientRect().height;
			if (pos.y + h > window.innerHeight - 8) pos = { ...pos, y: Math.max(8, window.innerHeight - h - 8) };
		});
	});

	function close(refocus = true) {
		const a = c?.anchor;
		ui.choices = null;
		if (refocus && a?.isConnected) a.focus();
	}
	/** Runs a choice once the menu is gone (a choice may open another menu, or a page). */
	function choose(run: () => void) {
		close();
		run();
	}
	function ask(i: number, choice: Choice) {
		asking = i;
		name = choice.ask?.value ?? '';
	}
	function named(choice: Choice) {
		const make = choice.ask?.make;
		const text = name.trim();
		if (make && text) choose(() => make(text));
	}

	function onkey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if (asking !== null) {
				asking = null;
				tick().then(first);
			} else close();
			return;
		}
		if (asking !== null) return;
		const items = [...(el?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? [])];
		const i = items.indexOf(document.activeElement as HTMLElement);
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			items[(i + 1) % items.length]?.focus();
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			items[(i - 1 + items.length) % items.length]?.focus();
		} else if (e.key === 'Home') {
			e.preventDefault();
			items[0]?.focus();
		} else if (e.key === 'End') {
			e.preventDefault();
			items[items.length - 1]?.focus();
		} else if (e.key === 'Tab') close(false);
	}
</script>

<svelte:window onpointerdown={(e) => { if (c && el && !el.contains(e.target as Node)) close(false); }} onresize={() => c && close(false)} />

{#if c}
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div class="menu" role="menu" aria-label={c.label} tabindex="-1" bind:this={el} style="left:{pos.x}px;top:{pos.y}px" onkeydown={onkey}>
		{#if c.title}<p class="mh">{c.title}</p>{/if}
		{#each c.choices as choice, i (i)}
			{#if asking === i && choice.ask}
				<form class="mnew" onsubmit={(e) => { e.preventDefault(); named(choice); }}>
					<!-- svelte-ignore a11y_autofocus -->
					<input aria-label={choice.ask.placeholder} placeholder={choice.ask.placeholder} bind:value={name} autofocus maxlength="80" />
					<button type="submit" disabled={!name.trim()}>{choice.ask.submit}</button>
				</form>
			{:else if choice.checked !== undefined}
				<button type="button" role="menuitemradio" aria-checked={choice.checked} class="mi" class:mi-on={choice.checked} onclick={() => choose(choice.run ?? (() => {}))}><Icon name={choice.checked ? 'check' : choice.icon} /><span>{choice.label}</span></button>
			{:else}
				<button type="button" role="menuitem" class="mi" class:mi-danger={choice.danger} onclick={() => (choice.ask ? ask(i, choice) : choose(choice.run ?? (() => {})))}><Icon name={choice.icon} /><span>{choice.label}</span></button>
			{/if}
		{/each}
	</div>
{/if}
