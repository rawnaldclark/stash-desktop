<!-- The question before songs are deleted for good (picked songs, one song, or everything by an artist). It says what the
     delete touches before anything happens: the playlists the songs are in, the files Stash saved, and the files of the
     listener's own music folders. Files go to the Recycle Bin. Enter deletes, Escape lets go. -->
<script lang="ts">
	import { library } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { P } from '../ui/pick.svelte';
	import { ui } from '../ui/state.svelte';
	import { deleteLines, deleteSaid, sizeText } from './delete-songs';

	const asked = $derived(ui.deleting);
	let preview = $state.raw<core.DeletePreview | null>(null);
	let failed = $state(false);
	let busy = $state(false);
	/** The listener's own files are deleted too, unless they say otherwise (a song whose file stays cannot leave). */
	let files = $state(true);
	let box = $state<HTMLElement | null>(null);

	$effect(() => {
		const a = asked;
		if (!a) return;
		preview = null;
		failed = false;
		files = true;
		core.removePreview(a.what).then(
			(p) => {
				if (ui.deleting === a) preview = p;
			},
			() => {
				if (ui.deleting === a) failed = true;
			}
		);
	});
	$effect(() => {
		if (preview) box?.querySelector<HTMLElement>('.del-go')?.focus();
	});

	const lines = $derived(preview ? deleteLines(preview) : []);
	/** How many songs really go: a folder file kept keeps its song. */
	const going = $derived(preview ? preview.songs - (files ? 0 : preview.folder_files) : 0);

	function close() {
		if (!busy) ui.deleting = null;
	}
	async function go() {
		const a = asked;
		if (!a || !preview || busy || going === 0) return;
		busy = true;
		try {
			const done = await library.deleteSongs(a.what, { playlists: true, files });
			P.clear();
			ui.deleting = null;
			ui.toast(deleteSaid(done));
			a.then?.(done.removed);
		} catch {
			// The change's own report says why.
		} finally {
			busy = false;
		}
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			close();
		}
	}
</script>

{#if asked}
	<div class="del-veil" role="presentation" onclick={close}></div>
	<div class="del" role="alertdialog" aria-modal="true" aria-labelledby="del-h" aria-describedby="del-d" tabindex="-1" bind:this={box} onkeydown={key}>
		<h2 id="del-h">
			{#if !preview}
				{asked.title}
			{:else if 'artist' in asked.what}
				Delete everything by {asked.title}?
			{:else}
				Delete {preview.songs === 1 ? (asked.title || 'this song') : `${preview.songs.toLocaleString()} songs`}?
			{/if}
		</h2>
		<div id="del-d" class="del-d">
			{#if failed}
				<p>Stash could not look at those songs. Try again in a moment.</p>
			{:else if !preview}
				<p class="del-wait">Looking at what this would delete…</p>
			{:else if preview.songs === 0}
				<p>None of these are in your library.</p>
			{:else}
				<p>
					{'artist' in asked.what ? `${preview.songs.toLocaleString()} ${preview.songs === 1 ? 'song leaves' : 'songs leave'} your library for good` : preview.songs === 1 ? 'It leaves your library for good' : 'They leave your library for good'}{lines.length ? ':' : '.'}
				</p>
				{#if lines.length}
					<ul class="del-l">
						{#each lines as line (line.icon + line.text)}
							<li><Icon name={line.icon} /><span>{line.text}</span></li>
						{/each}
					</ul>
				{/if}
				{#if preview.folder_files}
					<label class="del-c">
						<input type="checkbox" bind:checked={files} disabled={busy} />
						<span>
							Also delete {preview.folder_files === 1 ? 'the file' : `the ${preview.folder_files.toLocaleString()} files`} in your music folders ({sizeText(preview.folder_bytes)}).
							{#if !files}<em>Left there, {preview.folder_files === 1 ? 'that song stays' : 'those songs stay'} in your library.</em>{/if}
						</span>
					</label>
				{/if}
				<p class="del-n">What you played of them stays in your history. Files go to the Recycle Bin.</p>
			{/if}
		</div>
		<div class="del-b">
			<button type="button" class="del-no" onclick={close} disabled={busy}>Cancel</button>
			{#if preview && preview.songs > 0}
				<button type="button" class="del-go" onclick={go} disabled={busy || going === 0}>
					<Icon name="delete" /><span>{busy ? 'Deleting…' : going === 1 ? 'Delete 1 song' : `Delete ${going.toLocaleString()} songs`}</span>
				</button>
			{/if}
		</div>
	</div>
{/if}

<style>
	.del-veil { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / .36); animation: del-fade .12s ease-out; }
	.del { position: fixed; z-index: 91; left: 50%; top: 22vh; transform: translateX(-50%); width: min(460px, calc(100vw - 32px)); padding: 22px 22px 16px; border-radius: 16px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 24px 64px rgb(0 0 0 / .28); display: grid; gap: 12px; animation: del-in .14s ease-out; }
	.del:focus { outline: none; }
	@keyframes del-fade { from { opacity: 0; } }
	@keyframes del-in { from { opacity: 0; translate: 0 8px; } }
	@media (prefers-reduced-motion: reduce) { .del, .del-veil { animation: none; } }
	h2 { margin: 0; font: 600 19px/1.3 var(--font-display, var(--font-body)); overflow-wrap: anywhere; }
	.del-d { display: grid; gap: 10px; color: var(--wp-text-2); font: 400 14px/1.5 var(--font-body); }
	.del-d p { margin: 0; }
	.del-wait { min-height: 3em; }
	.del-l { margin: 0; padding: 0; list-style: none; display: grid; gap: 6px; color: var(--wp-text); }
	.del-l li { display: flex; align-items: flex-start; gap: 10px; }
	.del-l :global(.ic) { width: 18px; height: 18px; flex: none; margin-top: 2px; color: var(--wp-text-2); }
	.del-c { display: flex; align-items: flex-start; gap: 10px; padding: 10px 12px; border-radius: 10px; background: var(--wp-hover); color: var(--wp-text); cursor: pointer; }
	.del-c input { margin: 3px 0 0; accent-color: var(--wp-accent); }
	.del-c em { display: block; font-style: normal; color: var(--wp-text-2); }
	.del-n { font-size: 13px; }
	.del-b { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
	.del-b button { display: inline-flex; align-items: center; gap: 8px; height: 38px; padding: 0 16px; border-radius: 999px; font: 600 14px/1 var(--font-body); }
	.del-b :global(.ic) { width: 18px; height: 18px; }
	.del-no { color: var(--wp-text); }
	.del-go { background: #c62828; color: #fff; }
	.del-go:disabled { opacity: .55; }
	@media (hover: hover) and (pointer: fine) { .del-no:hover { background: var(--wp-hover); } .del-go:not(:disabled):hover { background: #b71c1c; } }
	.del-b button:focus-visible { outline: 2px solid var(--wp-accent); outline-offset: 2px; }
</style>
