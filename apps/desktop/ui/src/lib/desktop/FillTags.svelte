<!-- Fill in missing tags: the files that say little of themselves (no album, genre, year or track number) are looked up
     in a public music catalogue. What was found is listed for the listener to look over, each song with a tick; only
     the ticked ones are written, and only into fields the file has blank. -->
<script lang="ts">
	import { library } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';
	import { editSaid } from './edit-tags';
	import { fillLine, offerLine } from './fill-tags';

	let now = $state.raw<core.FillNow | null>(null);
	let failed = $state<string | null>(null);
	let busy = $state(false);
	/** The songs whose tick was taken off. */
	let off = $state<Set<number>>(new Set());

	async function read() {
		try {
			now = await core.fillNow();
			failed = null;
		} catch (e) {
			failed = core.said(e);
		}
	}
	// While a lookup runs, its progress is read again every second and a half.
	$effect(() => {
		void read();
		const timer = setInterval(() => {
			if (now?.running && !busy) void read();
		}, 1500);
		return () => clearInterval(timer);
	});

	const found = $derived(now?.found ?? []);
	const ticked = $derived(found.filter((f) => !off.has(f.id)));

	async function act(run: () => Promise<unknown>) {
		if (busy) return;
		busy = true;
		try {
			await run();
			await read();
		} catch (e) {
			failed = core.said(e);
		} finally {
			busy = false;
		}
	}
	const start = () => act(() => core.post('/api/fill/start', {}));
	const stop = () => act(() => core.post('/api/fill/stop', {}));
	const write = () =>
		act(async () => {
			const report = await library.fillTags(ticked.map((f) => f.id));
			ui.toast(editSaid(report));
		});
	function toggle(id: number) {
		const next = new Set(off);
		if (!next.delete(id)) next.add(id);
		off = next;
	}
	function close() {
		ui.filling = false;
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			close();
		}
	}
</script>

<div class="ft-veil" role="presentation" onclick={close}></div>
<div class="ft" role="dialog" aria-modal="true" aria-labelledby="ft-h" tabindex="-1" onkeydown={key}>
	<header class="ft-top">
		<h2 id="ft-h">Fill in missing tags</h2>
		<button type="button" class="act-ib" aria-label="Close" title="Close" onclick={close}><Icon name="close" /></button>
	</header>
	{#if failed && !now}
		<p class="ft-say" role="alert">{failed}</p>
	{:else if !now}
		<p class="ft-say">Reading your library…</p>
	{:else}
		<p class="ft-say">
			{fillLine(now)}
		</p>
		{#if !now.running && !found.length}
			<p class="ft-say ft-note">
				Stash looks each one up in Apple's music catalogue by its title and artist, so this uses your connection and sends those two to Apple. It only fills
				what a file has blank (album, year, track number, genre) and writes nothing until you have looked the results over.
			</p>
		{/if}
		{#if now.running}
			<div class="ft-bar" role="progressbar" aria-valuemin="0" aria-valuemax={now.total} aria-valuenow={now.done}><i style:width="{now.total ? (now.done / now.total) * 100 : 0}%"></i></div>
		{/if}
		{#if now.problem}<p class="ft-wrong" role="alert">{now.problem}</p>{/if}
		{#if failed}<p class="ft-wrong" role="alert">{failed}</p>{/if}
		{#if found.length}
			<ul class="ft-list">
				{#each found as f (f.id)}
					<li>
						<label class="ft-r">
							<input type="checkbox" checked={!off.has(f.id)} onchange={() => toggle(f.id)} disabled={busy} />
							<span class="ft-song"><b>{f.title}</b><span>{f.artist}</span></span>
							<span class="ft-set">{offerLine(f.set)}</span>
						</label>
					</li>
				{/each}
			</ul>
		{/if}
		<div class="ft-b">
			{#if found.length && !now.running}
				<button type="button" class="ft-no" onclick={() => (off = off.size ? new Set() : new Set(found.map((f) => f.id)))} disabled={busy}>{off.size ? 'Tick all' : 'Untick all'}</button>
			{/if}
			<span class="ft-gap"></span>
			{#if now.running}
				<button type="button" class="ft-no" onclick={stop} disabled={busy}>Stop looking</button>
			{:else if found.length}
				<button type="button" class="ft-go" onclick={write} disabled={busy || !ticked.length}>{busy ? 'Writing…' : `Write to ${ticked.length.toLocaleString()} ${ticked.length === 1 ? 'file' : 'files'}`}</button>
			{:else if now.blanks}
				<button type="button" class="ft-go" onclick={start} disabled={busy}>{now.total ? 'Look again' : `Look up ${now.blanks.toLocaleString()} ${now.blanks === 1 ? 'file' : 'files'}`}</button>
			{:else}
				<button type="button" class="ft-go" onclick={close}>Done</button>
			{/if}
		</div>
	{/if}
</div>

<style>
	.ft-veil { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / .36); }
	.ft { position: fixed; z-index: 91; left: 50%; top: 8vh; transform: translateX(-50%); width: min(820px, calc(100vw - 32px)); max-height: 86vh; display: flex; flex-direction: column; gap: 12px; padding: 20px 22px 16px; border-radius: 16px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 24px 64px rgb(0 0 0 / .28); }
	.ft:focus { outline: none; }
	.ft-top { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
	.ft-top h2 { margin: 0; font: 600 19px/1.3 var(--font-display, var(--font-body)); }
	.ft-say { margin: 0; color: var(--wp-text); font: 400 14px/1.5 var(--font-body); }
	.ft-note { color: var(--wp-text-2); font-size: 13.5px; }
	.ft-wrong { margin: 0; color: #c62828; font: 500 13.5px/1.4 var(--font-body); }
	.ft-bar { height: 6px; border-radius: 3px; background: var(--wp-hover); overflow: hidden; }
	.ft-bar i { display: block; height: 100%; background: var(--acc); transition: width .4s; }
	.ft-list { margin: 0 -8px; padding: 0; list-style: none; overflow-y: auto; min-height: 0; flex: 1 1 auto; border-top: 1px solid var(--wp-line-2); border-bottom: 1px solid var(--wp-line-2); }
	.ft-r { display: grid; grid-template-columns: 20px minmax(0, 5fr) minmax(0, 7fr); align-items: center; gap: 12px; min-height: 44px; padding: 4px 8px; border-radius: 8px; font: 400 13.5px/1.35 var(--font-body); cursor: pointer; }
	.ft-r input { width: 16px; height: 16px; accent-color: var(--acc); }
	.ft-song { display: grid; min-width: 0; }
	.ft-song b { font-weight: 500; color: var(--wp-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ft-song span { color: var(--wp-text-2); font-size: 12.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ft-set { color: var(--wp-text-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ft-b { display: flex; align-items: center; gap: 8px; }
	.ft-gap { flex: 1; }
	.ft-b button { height: 38px; padding: 0 18px; border-radius: 999px; font: 600 14px/1 var(--font-body); }
	.ft-no { color: var(--wp-text); }
	.ft-go { background: var(--acc); color: var(--acc-ink); }
	.ft-go:disabled { opacity: .5; }
	@media (hover: hover) and (pointer: fine) { .ft-no:hover, .ft-r:hover { background: var(--wp-hover); } }
</style>
