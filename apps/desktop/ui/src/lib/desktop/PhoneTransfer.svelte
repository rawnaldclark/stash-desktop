<!-- Songs coming in from Stash for Android over USB (the core's `phone_transfer.rs`): a card in the corner while they
     come, and what came after, until it is closed. Asks the core every few seconds; says nothing when nothing happens. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from '../core';
	import Icon from '../ui/Icon.svelte';
	import { songs } from '../ui/format';

	type Now = { state: 'idle' | 'taking' | 'done'; phone: string | null; done: number; total: number; taken: number; had: number; not_kept: number; failed: number; cleared: boolean; at_ms: number };
	let now = $state<Now | null>(null);
	let closedAt = $state(0);

	onMount(() => {
		let alive = true;
		const ask = async () => {
			try {
				const n = await get<Now>('/api/phone');
				if (alive) now = n;
			} catch {
				/* the core is not answering: nothing to say */
			}
		};
		void ask();
		const timer = setInterval(ask, 3000);
		return () => {
			alive = false;
			clearInterval(timer);
		};
	});

	const shown = $derived(!!now && now.state !== 'idle' && now.at_ms > closedAt);
	const from = $derived(now?.phone ?? 'your phone');
	const line = $derived.by(() => {
		if (!now) return '';
		if (now.state === 'taking') return `${now.done} of ${now.total}`;
		const parts = [`${songs(now.taken)} added as downloads`];
		if (now.had) parts.push(`${now.had} here already`);
		if (now.not_kept) parts.push(`${songs(now.not_kept)} in your library to download here (their files can't be kept)`);
		if (now.failed) parts.push(`${now.failed} not taken: send again from the phone`);
		return parts.join(' · ');
	});
</script>

{#if shown && now}
	<div class="ph" role="status" aria-live="polite">
		<Icon name="smartphone" />
		<div class="ph-t">
			<b>{now.state === 'taking' ? `Taking songs from ${from}` : `Songs from ${from}`}</b>
			<span>{line}</span>
			{#if now.state === 'taking'}
				<div class="ph-bar" aria-hidden="true"><i style="width: {now.total ? (100 * now.done) / now.total : 0}%"></i></div>
			{:else if now.cleared}
				<span>The copy on the phone was cleared.</span>
			{/if}
		</div>
		{#if now.state === 'done'}
			<button type="button" class="ph-x" aria-label="Close" onclick={() => (closedAt = now?.at_ms ?? Date.now())}><Icon name="close" /></button>
		{/if}
	</div>
{/if}

<style>
	.ph { position: fixed; right: 20px; bottom: 112px; z-index: 55; display: flex; gap: 12px; align-items: flex-start; width: 340px; max-width: calc(100vw - 32px); padding: 14px 14px 14px 16px; border-radius: 14px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); color: var(--wp-text); }
	.ph > :global(svg) { flex: none; width: 22px; height: 22px; fill: currentColor; color: var(--acc-text); margin-top: 1px; }
	.ph-t { flex: 1; min-width: 0; display: grid; gap: 4px; }
	.ph-t b { font: 600 14px/1.25 var(--font-body); }
	.ph-t span { color: var(--wp-text-2); font: 400 13px/1.35 var(--font-body); }
	.ph-bar { height: 4px; border-radius: 2px; background: var(--wp-hover); overflow: hidden; margin-top: 4px; }
	.ph-bar i { display: block; height: 100%; background: var(--acc); transition: width 400ms var(--ease-standard); }
	.ph-x { flex: none; width: 28px; height: 28px; display: grid; place-items: center; border-radius: 8px; color: var(--wp-text-2); }
	.ph-x:hover { background: var(--wp-hover); color: var(--wp-text); }
	.ph-x :global(svg) { width: 18px; height: 18px; fill: currentColor; }
</style>
