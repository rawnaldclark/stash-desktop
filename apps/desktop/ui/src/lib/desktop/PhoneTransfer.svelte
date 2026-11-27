<!-- Songs coming in from Stash for Android over USB (the core's `phone_transfer.rs`), told at every step in a card in
     the corner: a phone plugged in (and what to tap on it), the phone getting its songs ready, its songs waiting to be
     chosen from (the page From your phone, or every new one at once), the songs coming in (how many, how much, which),
     and what came of it, or that the phone was unplugged (or stopped the send, or its songs can't be saved here) and how
     to finish. Asks the core every second while something is happening, every few seconds otherwise. Not shown on the
     page that chooses the songs: it says the same. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { get, post, said } from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';
	import { songs } from '../ui/format';
	import type { Offer } from './phone-songs';
	import { broughtLine, type Brought } from './bring-in';

	type Now = {
		state: 'idle' | 'connected' | 'preparing' | 'ready' | 'taking' | 'stalled' | 'done' | 'stopped' | 'withdrawn' | 'cannot_save';
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
		here: number;
		left: number;
		cleared: boolean;
		/** Every song is in, but the send's list is still on the phone: the computer removes it at a look (by itself while `trying_again`, else once the phone is plugged in again). */
		list_left: boolean;
		/** The next look carries the send on by itself: the songs not taken come in, or the list that stayed goes. */
		trying_again: boolean;
		at_ms: number;
		/** Why songs can't be saved here, and where they go; or (`library`) why Stash's library can't keep them, and the drive it is on. */
		error: string | null;
		folder: string | null;
		library: boolean;
		drive: string | null;
		/** The send waiting carries the phone's library (likes, plays, playlists): chosen from on Bring in your music. */
		with_library?: boolean;
		/** What of a library comes in was chosen before for this phone: a take with no choice brings in the same. */
		library_choice_kept?: boolean;
		/** What the phone's library brought with this take, when it came in. */
		brought?: Brought | null;
	};
	let now = $state<Now | null>(null);
	let closedAt = $state(0);
	let starting = $state(false);
	let trouble = $state<string | null>(null);

	/** Takes every song of the send waiting that is not on this computer yet. */
	async function takeNew() {
		starting = true;
		trouble = null;
		try {
			const offer = await get<Offer>('/api/phone/songs');
			const files = offer.songs.filter((s) => !s.here).map((s) => s.file);
			if (files.length) await post('/api/phone/take', { sent_at_ms: offer.sent_at_ms, files });
		} catch (e) {
			trouble = said(e);
		} finally {
			starting = false;
		}
	}

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
			const busy = now && (now.state === 'preparing' || now.state === 'taking' || now.state === 'stalled');
			if (alive) timer = setTimeout(ask, busy ? 1000 : 3000);
		};
		void ask();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	const gb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);
	/** Every song of a send in, said so (one song, or none, as such). */
	const allIn = (n: number) => (n === 0 ? '' : n === 1 ? 'The song came in. ' : `All ${songs(n)} came in. `);
	const shown = $derived(!!now && now.state !== 'idle' && now.at_ms > closedAt && page.url.pathname !== '/phone' && page.url.pathname !== '/bring-in' && ui.mode !== null);
	const from = $derived(now?.phone ?? 'Your phone');
	/** The card's heading, its lines, and its bar (a share from 0 to 1, null for one that moves on its own, undefined for none). */
	const card = $derived.by((): { title: string; lines: string[]; bar: number | null | undefined } | null => {
		if (!now) return null;
		switch (now.state) {
			case 'connected':
				return { title: `${from} is connected`, lines: ['In Stash on your phone, open Settings › Library & Storage and tap Send to computer.'], bar: undefined };
			case 'preparing':
				return { title: `${from} is getting songs ready`, lines: [`${songs(now.ready)} ready so far. You choose which come in next.`], bar: null };
			case 'ready': {
				const fresh = now.total - now.here;
				if (now.with_library)
					return {
						title: now.total ? `Your music is waiting on ${from}` : `Your likes, playlists and plays are waiting on ${from}`,
						lines: [now.total ? `${songs(now.total)}${fresh ? ` · ${gb(now.bytes_total)}` : ', all on this computer already'}, with your likes, playlists and plays.` : 'Choose what comes in.', trouble ?? ''].filter(Boolean),
						bar: undefined
					};
				return {
					title: `${songs(now.total)} waiting on ${from}`,
					lines: [fresh ? `${now.here ? `${now.here.toLocaleString()} on this computer already · ` : ''}${gb(now.bytes_total)} in all` : 'All of them are on this computer already.', trouble ?? ''].filter(Boolean),
					bar: undefined
				};
			}
			case 'taking':
				return {
					title: `Taking songs from ${from}`,
					lines: [`${now.done} of ${now.total} · ${gb(now.bytes_done)} of ${gb(now.bytes_total)}`, now.song ?? '', 'Keep the phone plugged in.'].filter(Boolean),
					bar: now.total ? now.done / now.total : 0
				};
			case 'stalled':
				return {
					title: `Waiting on ${from}`,
					lines: [
						`${now.done} of ${now.total} · ${gb(now.bytes_done)} of ${gb(now.bytes_total)}`,
						'It stopped sending. Asking it again…',
						'If this stays, in the phone’s USB notification choose No data transfer, then File transfer.'
					],
					bar: now.total ? now.done / now.total : 0
				};
			case 'stopped':
				return {
					title: `Lost contact with ${from}`,
					lines: [
						// Every song in, and only the list left on the phone: nothing more comes in, the computer finishes with it.
						now.list_left && now.done === now.total
							? `${allIn(now.total)}Plug the phone back in so the computer can finish with it.`
							: `${now.total ? `${now.done - now.failed} of ${now.total} songs came in. ` : ''}Unplug the phone and plug it back in, or in its USB notification choose No data transfer, then File transfer${now.total ? ': the rest come in by themselves' : ''}.`
					],
					bar: undefined
				};
			case 'withdrawn':
				return {
					title: `${from} stopped the send`,
					lines: now.total ? [`${now.done - now.failed} of ${now.total} songs came in.`] : [],
					bar: undefined
				};
			case 'cannot_save': {
				const came = now.total ? `${now.done - now.failed} of ${now.total} songs came in. ` : '';
				const why = now.error ?? "it can't be written";
				return now.library
					? {
							title: `Songs from ${from} can't be kept`,
							lines: [`${came}Stash's library can't keep them: ${why}. Free some space on ${now.drive ?? 'its drive'}, or check that drive, then plug the phone back in.`],
							bar: undefined
						}
					: {
							title: `Songs from ${from} can't be saved`,
							lines: [`${came}They can't be saved in ${now.folder ?? 'your downloads folder'}: ${why}. Free some space or reconnect the drive, then plug the phone back in.`],
							bar: undefined
						};
			}
			case 'done': {
				// A sync that brought no new song but the phone's library is the library brought up to date, not nothing.
				const synced = !!now.brought && !now.taken && !now.had && !now.not_kept && !now.failed;
				const lines = synced ? [] : [`${songs(now.taken)} added to your downloads.`];
				if (now.brought) lines.push(broughtLine(now.brought));
				if (now.had) lines.push(`${songs(now.had)} you had here already.`);
				if (now.not_kept) lines.push(`${songs(now.not_kept)} in your library to download here: their files can't be kept.`);
				if (now.left && synced) lines.push(now.left === 1 ? 'The song you left out stays on the phone.' : `The ${now.left.toLocaleString()} songs you left out stay on the phone.`);
				else if (now.left) lines.push(now.left === 1 ? 'The other song stays in Stash on the phone.' : `The other ${now.left.toLocaleString()} stay in Stash on the phone.`);
				if (now.failed && now.trying_again) lines.push(`${songs(now.failed)} didn't come in yet: the computer tries ${now.failed === 1 ? 'it' : 'them'} again.`);
				else if (now.failed) lines.push(`${songs(now.failed)} didn't come in. Send again from the phone to try them.`);
				else if (now.list_left && now.trying_again) lines.push('Keep the phone plugged in: the computer finishes with it in a moment.');
				else if (now.list_left) lines.push('Unplug the phone and plug it back in so the computer can finish with it.');
				else if (now.cleared || now.left) lines.push('You can unplug the phone.');
				const title = now.failed ? `Songs from ${from}, with trouble` : synced ? `${from} is up to date` : `Songs from ${from} are here`;
				return { title, lines, bar: undefined };
			}
			default:
				return null;
		}
	});
	const closable = $derived(
		!!now && (now.state === 'connected' || now.state === 'ready' || now.state === 'done' || now.state === 'stopped' || now.state === 'withdrawn' || now.state === 'cannot_save')
	);
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
			{#if now.state === 'ready'}
				<div class="ph-do">
					{#if now.with_library}
						<button type="button" class="btn" onclick={() => void goto('/bring-in')}>Choose what comes in</button>
					{:else}
						<button type="button" class="btn" onclick={() => void goto('/phone')}>Choose songs</button>
					{/if}
					{#if now.total > now.here && (!now.with_library || now.library_choice_kept)}
						<button type="button" class="btn ghost" disabled={starting} onclick={takeNew}>{starting ? 'Starting…' : `Take all ${(now.total - now.here).toLocaleString()} new`}</button>
					{/if}
				</div>
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
	.ph-do { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 8px; }
	.ph-do .btn { height: 32px; padding: 0 14px; }
	.ph-x { flex: none; width: 28px; height: 28px; display: grid; place-items: center; border-radius: 8px; color: var(--wp-text-2); }
	.ph-x:hover { background: var(--wp-hover); color: var(--wp-text); }
	.ph-x :global(svg) { width: 18px; height: 18px; fill: currentColor; }
</style>
