<!-- Library › Downloads: songs saved to this computer, and those on their way. What is saved plays with no connection; the
     rest plays through the add-ons until it is saved. A song is saved from its menu, or a whole list from Liked or a playlist. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { itemOfRow, library, player } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import TrackRow from '../ui/TrackRow.svelte';
	import { songs } from '../ui/format';
	import { ui } from '../ui/state.svelte';
	import { trouble } from './folders.svelte';
	import { NF } from './unfound.svelte';

	const PAGE = 300;
	let d = $state<core.Downloads | null>(null);
	let shown = $state(PAGE);
	let picking = $state(false);
	let savedBefore = -1;
	let failedBefore = 0;

	async function read() {
		try {
			const next = await core.downloads(0, shown);
			// Songs that became files: the library's lists show them as files from now on.
			if (savedBefore >= 0 && next.counts.saved > savedBefore) void library.reload();
			savedBefore = next.counts.saved;
			// A song that could not be saved may now be known not to play.
			if (next.counts.failed > failedBefore) void NF.refresh();
			failedBefore = next.counts.failed;
			d = next;
		} catch {
			/* the core said nothing new: the list stays as it was */
		}
	}

	onMount(() => {
		let gone = false;
		let timer: ReturnType<typeof setTimeout>;
		// Every second while something is on its way, else every few seconds (a song may be put on the list from a menu).
		const tick = async () => {
			await read();
			if (gone) return;
			const busy = !!d && d.counts.queued + d.counts.saving > 0;
			timer = setTimeout(tick, busy ? 1000 : 4000);
		};
		void tick();
		return () => {
			gone = true;
			clearTimeout(timer);
		};
	});

	const rows = $derived(d?.rows ?? []);
	const items = $derived(rows.map((r) => itemOfRow(r)));
	const total = $derived(d ? d.counts.queued + d.counts.saving + d.counts.saved + d.counts.failed : 0);
	const failed = $derived(rows.filter((r) => r.state === 'failed'));

	const mb = (bytes: number | null) => (bytes ? `${(bytes / 1_048_576).toFixed(bytes < 10_485_760 ? 1 : 0)} MB` : '');
	function status(r: core.DownloadRow): string {
		if (r.state === 'saving') {
			const now = d?.now?.id === r.id ? d.now : null;
			return now?.of ? `Saving ${Math.floor((now.bytes / now.of) * 100)} %` : now ? `Saving ${mb(now.bytes)}` : 'Saving';
		}
		if (r.state === 'queued') return 'Waiting';
		if (r.state === 'failed') return 'Not saved';
		return [r.codec?.toUpperCase().replace('PCM', 'WAV'), mb(r.bytes)].filter(Boolean).join(' · ') || 'Saved';
	}
	const summary = $derived.by(() => {
		if (!d) return '';
		const c = d.counts;
		const parts = [];
		if (c.saving + c.queued) parts.push(`${songs(c.saving + c.queued)} on the way`);
		if (c.failed) parts.push(`${c.failed} not saved`);
		parts.push(`${songs(c.saved)} saved`);
		return parts.join(' · ');
	});

	const play = (i: number) => void player.playItems(items, i, { from: 'Downloads' });
	async function retry() {
		try {
			d = await core.retrySaving();
		} catch (e) {
			ui.toast(trouble(e));
		}
	}
	async function change() {
		picking = true;
		try {
			d = await core.setDownloadsFolder({ pick: true });
		} catch (e) {
			ui.toast(trouble(e));
		} finally {
			picking = false;
		}
	}
	const extra = (r: core.DownloadRow) => ({ download: r.state === 'saved' ? ('saved' as const) : ('waiting' as const) });
</script>

{#if d}
	{#if total}
		<div class="lib-actions">
			<button type="button" class="act-play" aria-label="Play your downloads" disabled={!items.length} onclick={() => play(0)}><Icon name="play_arrow" /></button>
			<span class="lib-count">{summary}</span>
			{#if d.counts.failed || d.held}<button type="button" class="btn ghost" onclick={retry}>Try again</button>{/if}
		</div>
		{#if d.held}
			<ul class="dl-why" role="status"><li>Saving waits: {d.held.why}. It tries again at {new Date(d.held.until_ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}, or now with Try again.</li></ul>
		{/if}
		{#if failed.length}
			<ul class="dl-why">
				{#each failed.slice(0, 3) as r (r.id)}<li><b>{r.title}</b> was not saved: {r.error ?? 'no reason was given'}.</li>{/each}
				{#if failed.length > 3}<li>And {failed.length - 3} more.</li>{/if}
			</ul>
		{/if}
		<div class="trk-list">
			{#each rows as r, i (r.id)}
				<TrackRow item={items[i]} num={i + 1} album tag={status(r)} onplay={() => play(i)} extra={extra(r)} />
			{/each}
		</div>
		{#if total > rows.length && rows.length >= shown}
			<button type="button" class="btn ghost hist-more" onclick={() => { shown += PAGE; void read(); }}>Show more</button>
		{/if}
	{:else}
		<p class="s-hint">Songs you save to this computer play with no connection. Save one from its menu, or all your likes or a playlist at once with the download button.</p>
	{/if}
	<p class="dl-where">
		<span>Saved in <b>{d.folder}</b></span>
		<button type="button" class="btn ghost" disabled={picking} onclick={change}>Change…</button>
	</p>
{/if}

<style>
	.dl-why { margin: 0 0 12px; padding: 10px 14px; border-radius: 10px; background: var(--wp-field); color: var(--wp-text-2); font: 400 13px/1.45 var(--font-body); list-style: none; }
	.dl-why b { color: var(--wp-text); font-weight: 600; }
	.dl-where { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; margin: 18px 0 0; color: var(--wp-text-2); font: 400 13px/1.4 var(--font-body); }
	.dl-where b { color: var(--wp-text); font-weight: 600; word-break: break-all; }
	.hist-more { margin: 16px 8px 0 0; }
</style>
