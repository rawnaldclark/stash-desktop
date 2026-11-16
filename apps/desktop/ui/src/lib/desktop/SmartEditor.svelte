<!-- A smart playlist's rules, made here or changed. Each rule reads as a sentence ("Last played · not in the last · 90 days");
     the songs they pick are counted by the core as the rules are written, and what makes no sense is said in its words.
     A new one starts from one of a few common lists. -->
<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { library } from '../app';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { songs } from '../ui/format';
	import { L, ui } from '../ui/state.svelte';
	import { FIELDS, ORDERS, PRESETS, QUALITIES, fieldOf, ruleFor } from './smart';

	const editing = ui.smart?.id ? L.playlists.find((p) => p.id === ui.smart?.id) : undefined;
	const start: core.SmartRules = editing?.rules ?? PRESETS[0].rules;
	let name = $state(editing?.name ?? PRESETS[0].name);
	let any = $state(start.any ?? false);
	let rules = $state<core.SmartRule[]>(start.rules.map((r) => ({ ...r })));
	let order = $state<core.SmartOrder>(start.order ?? 'added');
	let limited = $state(start.limit != null);
	let limit = $state<number>(start.limit ?? 50);
	let busy = $state(false);
	let found = $state<{ count: number; titles: string[] } | null>(null);
	let wrong = $state('');
	let nameField = $state<HTMLInputElement>();
	const before = document.activeElement as HTMLElement | null;

	/** The rules as the core takes them. */
	const shaped = (): core.SmartRules => ({
		any,
		rules: $state.snapshot(rules) as core.SmartRule[],
		order,
		...(limited && limit > 0 ? { limit: Math.floor(limit) } : {})
	});
	const same = (a: core.SmartRules, b: core.SmartRules) => JSON.stringify({ ...a, limit: a.limit ?? null }) === JSON.stringify({ ...b, limit: b.limit ?? null });
	/** The common list the rules are now, if they are one. */
	const matching = $derived(PRESETS.find((p) => same(p.rules, shaped()))?.name);

	function pick(p: (typeof PRESETS)[number]) {
		// The name follows the list picked, unless the listener named it.
		if (!name.trim() || PRESETS.some((x) => x.name === name.trim())) name = p.name;
		any = p.rules.any;
		rules = p.rules.rules.map((r) => ({ ...r }));
		order = p.rules.order ?? 'added';
		limited = p.rules.limit != null;
		limit = p.rules.limit ?? 50;
	}

	/** What can be told without asking the core: a rule still being written. */
	function unfinished(list: core.SmartRule[]): string {
		for (const r of list) {
			const kind = fieldOf(r.field).kind;
			const blank = r.value === null || r.value === undefined || String(r.value).trim() === '';
			if (kind === 'text' && blank) return 'a rule is missing its words';
			if (['number', 'minutes', 'days', 'often'].includes(kind) && (blank || Number.isNaN(Number(r.value)))) return 'a rule is missing its number';
			if (kind === 'often' && !r.days) return 'a rule is missing its days';
		}
		return '';
	}

	// The core counts what the rules pick a moment after they change; an answer to older rules is dropped.
	let timer: ReturnType<typeof setTimeout> | undefined;
	let asked = 0;
	$effect(() => {
		const now = shaped();
		clearTimeout(timer);
		const early = unfinished(now.rules);
		if (early) {
			asked++;
			found = null;
			wrong = early;
			return;
		}
		timer = setTimeout(() => {
			const mine = ++asked;
			core.previewRules(now).then(
				(p) => {
					if (mine !== asked) return;
					found = { count: p.count, titles: p.rows.map((t) => t.title) };
					wrong = '';
				},
				(e) => {
					if (mine !== asked) return;
					found = null;
					wrong = e instanceof Error ? e.message : String(e);
				}
			);
		}, 250);
	});

	function close() {
		ui.smart = null;
		void tick().then(() => before?.focus?.());
	}
	async function save() {
		if (busy || !name.trim() || wrong) return;
		busy = true;
		try {
			if (editing) {
				await library.setPlaylistRules(editing.id, shaped());
				if (name.trim() !== editing.name) await library.renamePlaylist(editing.id, name);
				close();
			} else {
				const made = await library.createSmartPlaylist(name, shaped());
				close();
				await goto(`/playlist/${made.id}`);
			}
		} catch {
			/* said by the shell (the core's own words) */
		} finally {
			busy = false;
		}
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			close();
		}
		// The window's own keys (space plays, arrows seek) wait while the rules are written.
		e.stopPropagation();
	}

	onMount(() => {
		nameField?.focus();
		nameField?.select();
		return () => clearTimeout(timer);
	});
</script>

<div class="sm-veil" role="presentation" onclick={close}></div>
<div class="sm" role="dialog" aria-modal="true" aria-labelledby="sm-h" tabindex="-1" onkeydown={key}>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			void save();
		}}
	>
		<header class="sm-top">
			<h2 id="sm-h">{editing ? 'Smart playlist' : 'New smart playlist'}</h2>
			<button type="button" class="act-ib" aria-label="Close" title="Close" onclick={close}><Icon name="close" /></button>
		</header>
		<p class="sm-say">It fills itself with the songs that match its rules, and changes as your library does.</p>
		<label class="sm-name"><span>Name</span><input class="sm-in" bind:this={nameField} bind:value={name} maxlength="80" required /></label>
		{#if !editing}
			<div class="sm-presets" role="group" aria-label="Start from">
				{#each PRESETS as p (p.name)}
					<button type="button" class="chip" class:on={matching === p.name} aria-pressed={matching === p.name} onclick={() => pick(p)}>{p.name}</button>
				{/each}
			</div>
		{/if}
		<p class="sm-lead">
			{#if rules.length > 1}
				Songs that match
				<select class="sm-in sm-any" bind:value={any} aria-label="Every rule, or any one">
					<option value={false}>every</option>
					<option value={true}>any</option>
				</select>
				rule:
			{:else if rules.length}
				Songs where:
			{:else}
				Every song in your library.
			{/if}
		</p>
		<ul class="sm-rules">
			{#each rules as rule, i (i)}
				{@const f = fieldOf(rule.field)}
				<li class="sm-rule">
					<select class="sm-in" aria-label="Rule {i + 1}: what" value={rule.field} onchange={(e) => (rules[i] = ruleFor(e.currentTarget.value as core.SmartField))}>
						{#each FIELDS as x (x.id)}<option value={x.id}>{x.label}</option>{/each}
					</select>
					<select class="sm-in" aria-label="Rule {i + 1}: how" bind:value={rule.op}>
						{#each f.ops as [op, words] (op)}<option value={op}>{words}</option>{/each}
					</select>
					{#if f.kind === 'text'}
						<input class="sm-in sm-grow" aria-label="Rule {i + 1}: words" bind:value={rule.value} maxlength="200" spellcheck="false" />
					{:else if f.kind === 'number'}
						<input class="sm-in sm-num" aria-label="Rule {i + 1}: number" type="number" min="0" step="1" bind:value={rule.value} />
						{#if f.unit}<span class="sm-unit">{f.unit}</span>{/if}
					{:else if f.kind === 'minutes'}
						<input
							class="sm-in sm-num"
							aria-label="Rule {i + 1}: minutes"
							type="number"
							min="0"
							step="1"
							value={Math.round(Number(rule.value) / 60)}
							oninput={(e) => (rule.value = Math.round(Number(e.currentTarget.value) * 60))}
						/>
						<span class="sm-unit">min</span>
					{:else if f.kind === 'days'}
						<input class="sm-in sm-num" aria-label="Rule {i + 1}: days" type="number" min="1" step="1" bind:value={rule.value} />
						<span class="sm-unit">days</span>
					{:else if f.kind === 'often'}
						<input class="sm-in sm-num" aria-label="Rule {i + 1}: times" type="number" min="1" step="1" bind:value={rule.value} />
						<span class="sm-unit">times in the last</span>
						<input class="sm-in sm-num" aria-label="Rule {i + 1}: days" type="number" min="1" step="1" bind:value={rule.days} />
						<span class="sm-unit">days</span>
					{:else if f.kind === 'quality'}
						<select class="sm-in" aria-label="Rule {i + 1}: quality" bind:value={rule.value}>
							{#each QUALITIES as [q, words] (q)}<option value={q}>{words}</option>{/each}
						</select>
					{/if}
					<button type="button" class="act-ib sm-x" aria-label="Remove rule {i + 1}" title="Remove" onclick={() => rules.splice(i, 1)}><Icon name="close" /></button>
				</li>
			{/each}
		</ul>
		<button type="button" class="btn ghost sm-add" disabled={rules.length >= 20} onclick={() => rules.push(ruleFor('artist'))}><Icon name="add" />Add a rule</button>
		<div class="sm-row">
			<label class="sm-order"><span>Order</span>
				<select class="sm-in" bind:value={order}>
					{#each ORDERS as [o, words] (o)}<option value={o}>{words}</option>{/each}
				</select>
			</label>
			<label class="sm-limit">
				<input type="checkbox" bind:checked={limited} />
				<span>At most</span>
				<input class="sm-in sm-num" type="number" min="1" max="100000" step="1" bind:value={limit} disabled={!limited} aria-label="How many songs at most" />
				<span>songs</span>
			</label>
		</div>
		<p class="sm-now" role="status">
			{#if wrong}
				<span class="sm-wrong">Not yet: {wrong}.</span>
			{:else if found}
				<b>{songs(found.count)}</b>{#if found.titles.length}{' '}now: {found.titles.join(', ')}{found.count > found.titles.length ? ', …' : ''}{/if}
			{:else}
				&nbsp;
			{/if}
		</p>
		<footer class="sm-end">
			<button type="button" class="btn ghost" onclick={close}>Cancel</button>
			<button type="submit" class="btn" disabled={busy || !name.trim() || !!wrong}>{editing ? 'Save' : 'Create'}</button>
		</footer>
	</form>
</div>

<style>
	.sm-veil { position: fixed; inset: 0; z-index: 80; background: rgb(0 0 0 / .32); }
	.sm { position: fixed; z-index: 81; left: 50%; top: 8vh; transform: translateX(-50%); width: min(680px, calc(100vw - 32px)); max-height: 84vh; overflow-y: auto; border-radius: 14px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); color: var(--wp-text); outline: none; }
	form { display: flex; flex-direction: column; gap: 14px; padding: 18px 20px 20px; }
	.sm-top { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
	.sm-top h2 { margin: 0; font: 600 18px/1.3 var(--font-display, var(--font-body)); }
	.sm-say { margin: -6px 0 0; color: var(--wp-text-2); font: 400 13px/1.45 var(--font-body); }
	.sm-name { display: grid; gap: 6px; font: 600 12px/1 var(--font-body); color: var(--wp-text-2); }
	.sm-in { height: 34px; min-width: 0; padding: 0 10px; border-radius: 10px; border: 1px solid var(--wp-line-2); background: var(--wp-field, transparent); color: var(--wp-text); font: 400 14px/1 var(--font-body); }
	.sm-name .sm-in { font-size: 15px; }
	.sm-presets { display: flex; flex-wrap: wrap; gap: 8px; }
	.sm-lead { margin: 4px 0 -4px; display: flex; align-items: center; flex-wrap: wrap; gap: 8px; font: 500 14px/1.4 var(--font-body); }
	.sm-any { height: 30px; }
	.sm-rules { display: grid; gap: 8px; margin: 0; padding: 0; list-style: none; }
	.sm-rule { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; padding: 8px; border-radius: 12px; background: var(--wp-hover); }
	.sm-grow { flex: 1 1 160px; }
	.sm-num { width: 84px; }
	.sm-unit { color: var(--wp-text-2); font: 400 13px/1 var(--font-body); }
	.sm-x { margin-left: auto; }
	.sm-add { align-self: flex-start; gap: 6px; }
	.sm-add :global(.ic) { width: 18px; height: 18px; }
	.sm-row { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 12px 20px; }
	.sm-order, .sm-limit { display: flex; align-items: center; gap: 8px; font: 400 14px/1 var(--font-body); }
	.sm-order span { color: var(--wp-text-2); }
	.sm-now { min-height: 20px; margin: 0; color: var(--wp-text-2); font: 400 13px/1.45 var(--font-body); overflow-wrap: anywhere; }
	.sm-now b { color: var(--wp-text); font-weight: 600; }
	.sm-wrong { color: var(--red); }
	.sm-end { display: flex; justify-content: flex-end; gap: 10px; }
	@media (prefers-reduced-motion: no-preference) { .sm { animation: sm-in 140ms var(--ease-out-expo, ease-out) both; } }
	@keyframes sm-in { from { opacity: 0; transform: translate(-50%, -6px); } }
</style>
