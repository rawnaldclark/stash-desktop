<!-- Keeps a list on this computer (a playlist, or the liked songs): its songs are saved as files, and so are songs that
     come onto it later, so it plays offline. The switch, and how far the list is. Turning it off saves no more of it;
     songs saved stay (Library › Downloads takes them off). -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';

	let { list, name, count }: { /** "liked", or a playlist's id */ list: string; name: string; /** the list's songs now: when it moves, how far is read again */ count: number } = $props();

	let kept = $state<core.ListDownload | null>(null);
	let busy = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function read() {
		clearTimeout(timer);
		try {
			kept = await core.listDownload(list);
		} catch {
			return;
		}
		// While songs are coming in, how far is read again now and then.
		if (kept.on && kept.waiting > 0) timer = setTimeout(() => void read(), 2000);
	}
	$effect(() => {
		void list;
		void count;
		void read();
	});
	onDestroy(() => clearTimeout(timer));

	async function flip() {
		if (!kept || busy) return;
		busy = true;
		try {
			const on = !kept.on;
			kept = await core.setListDownload(list, on);
			ui.toast(on ? `${name} is kept on this computer: its songs are being saved, and songs added later will be too` : `${name} is no longer kept on this computer. Songs saved stay (Library › Downloads)`);
			void read();
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			busy = false;
		}
	}

	const words = $derived.by(() => {
		if (!kept?.on) return '';
		const of = `${kept.here} of ${kept.songs}`;
		if (kept.waiting > 0) return `Saving: ${of} on this computer`;
		const done = kept.here >= kept.songs ? 'On this computer' : `${of} on this computer`;
		return kept.failed > 0 ? `${done} · ${kept.failed} could not be saved` : done;
	});
</script>

<button
	type="button"
	class="act-ib dl-sw"
	class:on={kept?.on}
	aria-pressed={kept?.on ?? false}
	aria-label="Keep {name} on this computer"
	title={kept?.on ? 'Kept on this computer. Click to stop' : 'Keep on this computer'}
	disabled={!kept || busy}
	onclick={flip}><Icon name="download" /></button
>
{#if words}<span class="dl-said" role="status">{words}</span>{/if}

<style>
	.dl-sw.on { color: var(--acc); background: var(--acc-wash); }
	.dl-said { font: 500 13px/1.3 var(--font-body); color: var(--wp-text-2); }
</style>
