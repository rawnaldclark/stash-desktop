<!-- Edit tags: what a song is called, who it is by, its album, genre, year and the rest, for one song or many at once.
     The change is written into the files themselves. With many songs, a field shows what they all say, or "(different)"
     where they differ; only the fields the listener changes are written, so the others stay as each file has them. -->
<script lang="ts">
	import { library } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';
	import { FIELDS, changedFields, editSaid, type FieldId } from './edit-tags';

	const asked = $derived(ui.editing);
	let now = $state.raw<core.TagsNow | null>(null);
	let failed = $state(false);
	let busy = $state(false);
	let wrong = $state<string | null>(null);
	/** What is typed, field by field; a field not in it has not been touched. */
	let typed = $state<Partial<Record<FieldId, string>>>({});
	let box = $state<HTMLElement | null>(null);

	$effect(() => {
		const a = asked;
		if (!a) return;
		now = null;
		failed = false;
		wrong = null;
		typed = {};
		core.tagsNow(a.ids).then(
			(t) => {
				if (ui.editing === a) now = t;
			},
			() => {
				if (ui.editing === a) failed = true;
			}
		);
	});
	$effect(() => {
		if (now) box?.querySelector<HTMLElement>('input')?.focus();
	});

	const many = $derived((now?.files ?? 0) > 1);
	const set = $derived(now ? changedFields(now, typed) : {});
	const changes = $derived(Object.keys(set).length);

	function close() {
		if (!busy) ui.editing = null;
	}
	async function save() {
		const a = asked;
		if (!a || !now || busy || !changes) return;
		busy = true;
		wrong = null;
		try {
			const report = await library.editTags(a.ids, set);
			ui.editing = null;
			ui.toast(editSaid(report));
		} catch (e) {
			wrong = core.said(e);
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
	<div class="et-veil" role="presentation" onclick={close}></div>
	<div class="et" role="dialog" aria-modal="true" aria-labelledby="et-h" tabindex="-1" bind:this={box} onkeydown={key}>
		<form
			onsubmit={(e) => {
				e.preventDefault();
				void save();
			}}
		>
			<header class="et-top">
				<h2 id="et-h">{now && now.files > 1 ? `Edit ${now.files.toLocaleString()} songs` : 'Edit song'}</h2>
				<button type="button" class="act-ib" aria-label="Close" title="Close" onclick={close}><Icon name="close" /></button>
			</header>
			{#if failed}
				<p class="et-say">Stash could not read those songs. Try again in a moment.</p>
			{:else if !now}
				<p class="et-say">Reading their tags…</p>
			{:else if now.files === 0}
				<p class="et-say">{now.others === 1 ? 'This song is not a file on this computer, so it has no tags to edit.' : 'None of these are files on this computer, so they have no tags to edit.'}</p>
			{:else}
				<p class="et-say">
					{many ? 'A field you change is written to every file. The others stay as each file has them.' : 'Changes are written into the file.'}
					{#if now.others}{' '}{now.others === 1 ? 'One song is' : `${now.others} songs are`} not on this computer and {now.others === 1 ? 'is' : 'are'} left alone.{/if}
				</p>
				<div class="et-g">
					{#each FIELDS as f (f.id)}
						{@const same = now[f.id].value}
						<label class="et-f" class:wide={f.wide} class:changed={f.id in set}>
							<span>{f.name}</span>
							<input
								type="text"
								inputmode={f.number ? 'numeric' : undefined}
								maxlength={f.number ? 4 : 300}
								value={typed[f.id] ?? same ?? ''}
								placeholder={same === null ? '(different)' : ''}
								disabled={busy}
								oninput={(e) => (typed = { ...typed, [f.id]: e.currentTarget.value })}
							/>
						</label>
					{/each}
				</div>
				{#if wrong}<p class="et-wrong" role="alert">{wrong}</p>{/if}
			{/if}
			<div class="et-b">
				<button type="button" class="et-no" onclick={close} disabled={busy}>Cancel</button>
				{#if now && now.files > 0}
					<button type="submit" class="et-go" disabled={busy || !changes}>{busy ? 'Writing…' : many ? `Write to ${now.files.toLocaleString()} files` : 'Save'}</button>
				{/if}
			</div>
		</form>
	</div>
{/if}

<style>
	.et-veil { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / .36); }
	.et { position: fixed; z-index: 91; left: 50%; top: 10vh; transform: translateX(-50%); width: min(620px, calc(100vw - 32px)); max-height: 84vh; overflow-y: auto; padding: 20px 22px 16px; border-radius: 16px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: 0 24px 64px rgb(0 0 0 / .28); }
	.et:focus { outline: none; }
	form { display: grid; gap: 14px; }
	.et-top { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
	.et-top h2 { margin: 0; font: 600 19px/1.3 var(--font-display, var(--font-body)); }
	.et-say { margin: -6px 0 0; color: var(--wp-text-2); font: 400 13.5px/1.5 var(--font-body); }
	.et-g { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
	.et-f { display: grid; gap: 6px; grid-column: span 1; font: 600 12px/1 var(--font-body); color: var(--wp-text-2); }
	.et-f.wide { grid-column: span 2; }
	.et-f input { height: 36px; min-width: 0; padding: 0 10px; border-radius: 10px; border: 1px solid var(--wp-line-2); background: var(--wp-field, transparent); color: var(--wp-text); font: 400 14px/1 var(--font-body); }
	.et-f input::placeholder { color: var(--wp-text-2); font-style: italic; }
	.et-f.changed span { color: var(--wp-accent); }
	.et-f.changed input { border-color: var(--wp-accent); }
	.et-f input:focus-visible { outline: 2px solid var(--wp-accent); outline-offset: 1px; }
	.et-wrong { margin: 0; color: #c62828; font: 500 13.5px/1.4 var(--font-body); }
	.et-b { display: flex; justify-content: flex-end; gap: 8px; }
	.et-b button { height: 38px; padding: 0 18px; border-radius: 999px; font: 600 14px/1 var(--font-body); }
	.et-no { color: var(--wp-text); }
	.et-go { background: var(--wp-accent); color: #fff; }
	.et-go:disabled { opacity: .5; }
	@media (hover: hover) and (pointer: fine) { .et-no:hover { background: var(--wp-hover); } }
	@media (max-width: 560px) { .et-g { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
