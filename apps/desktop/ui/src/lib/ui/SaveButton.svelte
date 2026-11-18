<!-- Download: saves the song to this computer (a song of an add-on's is kept in the library first). What happened is said
     in a toast where the window has one, and on the button itself for a moment (a check), so the mini player says it too. -->
<script lang="ts">
	import type { LibraryItem } from '../api';
	import { saveSongs } from '../app';
	import Icon from './Icon.svelte';
	import { ui } from './state.svelte';

	let { item, class: cls = '' }: { item: LibraryItem | null | undefined; class?: string } = $props();
	let said = $state<string | null>(null);
	let busy = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function save(e: MouseEvent) {
		e.stopPropagation();
		if (!item || busy) return;
		busy = true;
		try {
			const words = await saveSongs([item]);
			ui.toast(words);
			said = words;
			clearTimeout(timer);
			timer = setTimeout(() => (said = null), 2500);
		} catch {
			/* said by the shell (the core's own words) */
		} finally {
			busy = false;
		}
	}
	$effect(() => () => clearTimeout(timer));
</script>

<button type="button" class="save-b {cls}" class:done={said !== null} disabled={!item || busy} aria-label={said ?? 'Download'} title={said ?? 'Download to this computer'} onclick={save}>
	<Icon name={said !== null ? 'check' : 'download'} />
</button>

<style>
	.save-b.done {
		color: var(--acc-text);
	}
</style>
