<!-- Settings › Library: songs downloaded on the phone, brought over USB (the core's `phone_transfer.rs`). Says how, which
     phone is plugged in and whether it has songs sent, and looks again on asking. The taking itself shows in the
     corner card (`PhoneTransfer.svelte`). -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { songs } from '../ui/format';

	type Phone = { name: string; has_songs: boolean | null };
	type Now = { state: 'idle' | 'connected' | 'preparing' | 'taking' | 'stalled' | 'done' | 'stopped'; phone: string | null; done: number; total: number; answered: number; phones: Phone[] };
	let now = $state<Now | null>(null);
	let looking = $state(false);
	/** Set when a look asked for here is done: the status line then says it looked. */
	let lookedAt = $state<number | null>(null);

	async function ask() {
		try {
			now = await core.get<Now>('/api/phone');
		} catch {
			/* the core is not answering */
		}
	}

	onMount(() => {
		void ask();
		const timer = setInterval(ask, 2000);
		return () => clearInterval(timer);
	});

	/** Asks the core to look, and waits (up to 20 s) until it has. */
	async function look() {
		looking = true;
		lookedAt = null;
		try {
			const { asked } = await core.post<{ asked: number }>('/api/phone/look', {});
			const until = Date.now() + 20_000;
			while (Date.now() < until) {
				await new Promise((r) => setTimeout(r, 400));
				await ask();
				if ((now?.answered ?? 0) >= asked) break;
			}
			lookedAt = Date.now();
		} catch {
			/* the status line stays as it was */
		} finally {
			looking = false;
		}
	}

	const status = $derived.by(() => {
		if (!now) return '';
		if (now.state === 'taking') return `Taking ${songs(now.total)} from ${now.phone ?? 'your phone'}: ${now.done} done`;
		if (now.state === 'preparing') return `${now.phone ?? 'Your phone'} is getting songs ready…`;
		if (now.state === 'stalled') return `${now.phone ?? 'Your phone'} stopped sending. Asking it again…`;
		if (now.state === 'stopped') return `Lost contact with ${now.phone ?? 'your phone'}. Unplug it and plug it back in, or in its USB notification choose No data transfer, then File transfer.`;
		const phone = now.phones[0];
		if (!phone) return 'No phone found. Plug it in, then choose File transfer in its USB notification.';
		if (phone.has_songs === null) return `${phone.name} is plugged in. Looking for songs…`;
		if (!phone.has_songs) return `${phone.name} is plugged in, but has no songs waiting. On the phone, tap Send to computer.`;
		return `${phone.name} has songs waiting: they’re coming in.`;
	});
</script>

<div class="set-row">
	<div class="set-lab">
		<span class="set-name">Downloads from your phone</span>
		<span class="set-sub">Plug your phone in and choose File transfer. Then in Stash on the phone: Settings › Library &amp; Storage › Send to computer. Songs come in here by themselves, with progress on both screens.</span>
		{#if status}<span class="set-sub" class:fp-new={lookedAt !== null} role="status">{lookedAt !== null && now?.state !== 'taking' ? 'Looked just now. ' : ''}{status}</span>{/if}
	</div>
	<div class="set-ctl">
		<button type="button" class="btn ghost" aria-busy={looking} disabled={looking || now?.state === 'taking'} onclick={look}>{looking ? 'Looking…' : 'Look for my phone'}</button>
	</div>
</div>

<style>
	.fp-new { color: var(--wp-text); }
</style>
