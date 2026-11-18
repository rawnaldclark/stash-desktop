<!-- Settings › Library: songs downloaded on the phone, brought over USB (the core's `phone_transfer.rs`). Says how, which
     phone is plugged in and whether it has songs sent, and looks again on asking. The taking itself shows in the
     corner card (`PhoneTransfer.svelte`). -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { songs } from '../ui/format';

	type Phone = { name: string; has_songs: boolean | null };
	type Now = { state: 'idle' | 'taking' | 'done'; phone: string | null; done: number; total: number; phones: Phone[] };
	let now = $state<Now | null>(null);
	let looking = $state(false);

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

	async function look() {
		looking = true;
		try {
			await core.post('/api/phone/look', {});
			await new Promise((r) => setTimeout(r, 2500));
			await ask();
		} catch {
			/* shown by the status line staying as it was */
		} finally {
			looking = false;
		}
	}

	const status = $derived.by(() => {
		if (!now) return '';
		if (now.state === 'taking') return `Taking ${songs(now.total)} from ${now.phone ?? 'your phone'}: ${now.done} done`;
		const phone = now.phones[0];
		if (!phone) return 'No phone found. Plug it in and choose File transfer in its USB notification.';
		if (phone.has_songs === null) return `${phone.name} is plugged in. Looking for songs…`;
		if (!phone.has_songs) return `${phone.name} is plugged in, with no songs sent. Send them from Stash on the phone.`;
		return `${phone.name} has songs sent: they come in shortly.`;
	});
</script>

<div class="set-row">
	<div class="set-lab">
		<span class="set-name">Downloads from your phone</span>
		<span class="set-sub">In Stash on Android: Settings › Library &amp; storage › Send to computer. Then plug the phone in and choose File transfer. Songs come in by themselves while Stash is open.</span>
		{#if status}<span class="set-sub" role="status">{status}</span>{/if}
	</div>
	<div class="set-ctl">
		<button type="button" class="btn ghost" disabled={looking || now?.state === 'taking'} onclick={look}>{looking ? 'Looking…' : 'Look for my phone'}</button>
	</div>
</div>
