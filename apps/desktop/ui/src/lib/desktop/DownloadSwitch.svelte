<!-- Keeps a list on this computer (a playlist, or the liked songs): its songs are saved as files, and so are songs that
     come onto it later, so it plays offline. The switch, and how far the list is. Turning it off saves no more of it;
     songs saved stay (Library › Downloads takes them off). -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { keptFlipped, likedKept } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';

	let { list, name, count }: { /** "liked", or a playlist's id */ list: string; name: string; /** the list's songs now: when it moves, how far is read again */ count: number } = $props();

	let kept = $state<core.ListDownload | null>(null);
	let busy = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;
	let gone = false;
	/** Which read is the latest: an answer to an older one (another list's, if the page moved on) is let go. */
	let asked = 0;

	async function read() {
		clearTimeout(timer);
		const mine = ++asked;
		const of = list;
		let now: core.ListDownload;
		try {
			now = await core.listDownload(of);
		} catch {
			return;
		}
		if (gone || mine !== asked) return;
		kept = now;
		if (of === 'liked') likedKept.set(now.on);
		// While songs are coming in, how far is read again now and then (offline, nothing comes in).
		if (now.on && now.waiting > 0 && !ui.offline) timer = setTimeout(() => void read(), 2000);
	}
	$effect(() => {
		void list;
		void count;
		// Back online, songs come in again: how far is read, and read again while they do.
		void ui.offline;
		void read();
	});
	onDestroy(() => {
		gone = true;
		clearTimeout(timer);
	});

	async function flip() {
		if (!kept || busy) return;
		busy = true;
		try {
			const on = !kept.on;
			const of = list;
			const now = await core.setListDownload(of, on);
			keptFlipped(of, now.on, now.edits);
			if (gone || of !== list) return;
			kept = now;
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
		const notes = [kept.failed > 0 ? `${kept.failed} could not be saved` : '', kept.declined > 0 ? `${kept.declined} taken off by you` : ''].filter(Boolean);
		return [done, ...notes].join(' · ');
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
