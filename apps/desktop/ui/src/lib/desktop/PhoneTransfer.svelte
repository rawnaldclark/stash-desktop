<!-- Songs coming in from Stash for Android over USB (the core's `phone_transfer.rs`), told at every step in a card in
     the corner: a phone plugged in (and what to tap on it), the phone getting its songs ready, the songs coming in (how
     many, how much, which), and what came of it, or that the phone was unplugged and how to finish. Asks the core every
     second while something is happening, every few seconds otherwise. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from '../core';
	import Icon from '../ui/Icon.svelte';
	import { songs } from '../ui/format';

	type Now = {
		state: 'idle' | 'connected' | 'preparing' | 'taking' | 'done' | 'stopped';
		phone: string | null;
		ready: number;
		done: number;
		total: number;
		bytes_done: number;
		bytes_total: number;
		song: string | null;
		taken: number;
		had: number;
		not_kept: number;
		failed: number;
		cleared: boolean;
		at_ms: number;
	};
	let now = $state<Now | null>(null);
	let closedAt = $state(0);

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const ask = async () => {
			try {
				const n = await get<Now>('/api/phone');
				if (alive) now = n;
			} catch {
				/* the core is not answering: nothing to say */
			}
			const busy = now && (now.state === 'preparing' || now.state === 'taking');
			if (alive) timer = setTimeout(ask, busy ? 1000 : 3000);
		};
		void ask();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	const gb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);
	const shown = $derived(!!now && now.state !== 'idle' && now.at_ms > closedAt);
	const from = $derived(now?.phone ?? 'Your phone');
	/** The card's heading, its lines, and its bar (a share from 0 to 1, null for one that moves on its own, undefined for none). */
	const card = $derived.by((): { title: string; lines: string[]; bar: number | null | undefined } | null => {
		if (!now) return null;
		switch (now.state) {
			case 'connected':
				return { title: `${from} is connected`, lines: ['In Stash on your phone, open Settings › Library & storage and tap Send to computer.'], bar: undefined };
			case 'preparing':
				return { title: `${from} is getting songs ready`, lines: [`${songs(now.ready)} ready so far. They come in here next.`], bar: null };
			case 'taking':
				return {
					title: `Taking songs from ${from}`,
					lines: [`${now.done} of ${now.total} · ${gb(now.bytes_done)} of ${gb(now.bytes_total)}`, now.song ?? '', 'Keep the phone plugged in.'].filter(Boolean),
					bar: now.total ? now.done / now.total : 0
				};
			case 'stopped':
				return {
					title: `Lost contact with ${from}`,
					lines: [
						`${now.total ? `${now.done} of ${now.total} songs came in. ` : ''}Unplug the phone, plug it back in and choose File transfer: the rest come in by themselves.`
					],
					bar: undefined
				};
			case 'done': {
				const lines = [`${songs(now.taken)} added to your downloads.`];
				if (now.had) lines.push(`${songs(now.had)} you had here already.`);
				if (now.not_kept) lines.push(`${songs(now.not_kept)} in your library to download here: their files can't be kept.`);
				if (now.failed) lines.push(`${songs(now.failed)} didn't come in. Send again from the phone to try them.`);
				else if (now.cleared) lines.push('You can unplug the phone.');
				return { title: now.failed ? `Songs from ${from}, with trouble` : `Songs from ${from} are here`, lines, bar: undefined };
			}
			default:
				return null;
		}
	});
	const closable = $derived(!!now && (now.state === 'connected' || now.state === 'done' || now.state === 'stopped'));
</script>

{#if shown && now && card}
	<div class="ph" role="status" aria-live="polite">
		<Icon name="smartphone" />
		<div class="ph-t">
			<b>{card.title}</b>
			{#each card.lines as line, i (i)}<span class:ph-song={now.state === 'taking' && i === 1}>{line}</span>{/each}
			{#if card.bar !== undefined}
				<div class="ph-bar" class:ph-wait={card.bar === null} aria-hidden="true"><i style:width={card.bar === null ? undefined : `${100 * card.bar}%`}></i></div>
			{/if}
		</div>
		{#if closable}
			<button type="button" class="ph-x" aria-label="Close" onclick={() => (closedAt = now?.at_ms ?? Date.now())}><Icon name="close" /></button>
		{/if}
	</div>
{/if}

<style>
	.ph { position: fixed; right: 20px; bottom: 112px; z-index: 55; display: flex; gap: 12px; align-items: flex-start; width: 360px; max-width: calc(100vw - 32px); padding: 14px 14px 14px 16px; border-radius: 14px; background: var(--wp-field); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); color: var(--wp-text); }
	.ph > :global(svg) { flex: none; width: 22px; height: 22px; fill: currentColor; color: var(--acc-text); margin-top: 1px; }
	.ph-t { flex: 1; min-width: 0; display: grid; gap: 4px; }
	.ph-t b { font: 600 14px/1.25 var(--font-body); }
	.ph-t span { color: var(--wp-text-2); font: 400 13px/1.35 var(--font-body); }
	.ph-t .ph-song { color: var(--wp-text); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.ph-bar { position: relative; height: 4px; border-radius: 2px; background: var(--wp-hover); overflow: hidden; margin-top: 4px; }
	.ph-bar i { display: block; height: 100%; background: var(--acc); transition: width 400ms var(--ease-standard); }
	.ph-wait i { position: absolute; width: 35%; animation: ph-move 1.2s var(--ease-standard) infinite; }
	@keyframes ph-move { from { left: -35%; } to { left: 100%; } }
	@media (prefers-reduced-motion: reduce) { .ph-wait i { animation: none; left: 0; width: 100%; opacity: 0.5; } }
	.ph-x { flex: none; width: 28px; height: 28px; display: grid; place-items: center; border-radius: 8px; color: var(--wp-text-2); }
	.ph-x:hover { background: var(--wp-hover); color: var(--wp-text); }
	.ph-x :global(svg) { width: 18px; height: 18px; fill: currentColor; }
</style>
