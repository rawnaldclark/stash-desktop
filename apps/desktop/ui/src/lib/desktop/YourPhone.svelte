<!-- Settings › Your phone: link this computer with the Stash app on the listener's phone, so the phone can be the remote for
     what plays here. A QR code to scan in the app, the six digits both screens show, then the linked devices. -->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import * as core from '../core';
	import { qrPath } from './qr';

	let view = $state<core.LinkView | null>(null);
	let failure = $state<string | null>(null);
	let now = $state(Date.now());

	async function act(fn: () => Promise<core.LinkView>) {
		try {
			view = await fn();
			failure = null;
		} catch (e) {
			failure = core.said(e);
		}
	}

	// While a code is up or the link is being made, the window asks the core every second for the next step.
	const live = $derived(!!view && ['opening', 'code', 'confirm', 'linking'].includes(view.pair.step));
	$effect(() => {
		if (!live) return;
		const t = setInterval(() => {
			now = Date.now();
			void act(core.linkNow);
		}, 1000);
		return () => clearInterval(t);
	});
	onMount(() => {
		void act(core.linkNow).then(() => {
			if (view?.linked) void act(core.linkRefresh);
		});
	});
	onDestroy(() => {
		// A code left on screen stops when the listener goes elsewhere (nothing of a pending answer is kept).
		if (view && ['opening', 'code', 'confirm'].includes(view.pair.step)) void core.linkStop();
	});

	const qr = $derived(view?.pair.step === 'code' ? qrPath(view.pair.link) : null);
	const left = $derived(view?.pair.step === 'code' ? Math.max(0, Math.round((view.pair.expiresAt - now) / 1000)) : 0);
	const mmss = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
	const others = $derived(view?.devices.filter((d) => !d.me) ?? []);
	const kindName = (t: string) => (t === 'phone' ? 'Phone' : t === 'desktop' ? 'Computer' : 'Browser');
</script>

<div class="set-card yp">
	{#if !view}
		<div class="set-row"><span class="set-sub">{failure ?? 'Reading…'}</span></div>
	{:else if view.pair.step === 'code' && qr}
		<div class="set-row yp-code">
			<svg class="yp-qr" viewBox="0 0 {qr.size} {qr.size}" role="img" aria-label="Code to scan with the Stash app on your phone">
				<rect width={qr.size} height={qr.size} fill="#fff" />
				<path d={qr.path} fill="#000" />
			</svg>
			<div class="set-lab">
				<span class="set-name">Scan this with the Stash app on your phone</span>
				<span class="set-sub">In the app: Settings › Stash on the web › Link a computer, then point the camera at this code.</span>
				<span class="set-sub" aria-live="polite">The code changes in {mmss(left)}.</span>
				<div class="set-ctl"><button type="button" class="btn ghost" onclick={() => act(core.linkStop)}>Cancel</button></div>
			</div>
		</div>
	{:else if view.pair.step === 'confirm'}
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name">Link {view.pair.phoneName}?</span>
				<span class="set-sub">Check that your phone shows the same six digits.</span>
				<p class="yp-digits" aria-label="Code {view.pair.code}">{view.pair.code}</p>
			</div>
			<div class="set-ctl">
				<button type="button" class="btn ghost" onclick={() => act(core.linkCancel)}>They don't match</button>
				<button type="button" class="btn" onclick={() => act(core.linkConfirm)}>Link</button>
			</div>
		</div>
	{:else if view.pair.step === 'opening' || view.pair.step === 'linking'}
		<div class="set-row"><span class="set-sub" role="status">{view.pair.step === 'opening' ? 'Getting a code…' : `Linking ${view.pair.phoneName}…`}</span></div>
	{:else}
		{#if view.pair.step === 'error' || view.pair.step === 'stale'}
			<div class="set-row">
				<span class="set-sub" role="alert">{view.pair.step === 'error' ? view.pair.message : 'The code ran out.'}</span>
				<button type="button" class="btn" onclick={() => act(core.linkPair)}>New code</button>
			</div>
		{/if}
		{#if view.linked}
			{#each others as d (d.id)}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name">{d.name}</span>
						<span class="set-sub">{kindName(d.type)}{d.status === 'keyChanged' ? ' · Its key changed: remove it on your phone and link it again' : d.status === 'unverified' ? ' · Not verified yet' : ''}</span>
					</div>
				</div>
			{/each}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">This computer</span>
					<span class="set-sub">{view.name}. {view.hasPhone ? 'Your phone can play here, add to the queue and control what plays.' : ''}</span>
				</div>
				<button type="button" class="btn ghost" onclick={() => act(core.linkUnlink)}>Unlink</button>
			</div>
		{:else if view.pair.step !== 'error' && view.pair.step !== 'stale'}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">Use your phone as the remote</span>
					<span class="set-sub">Browse on your phone, play here, and add to this computer's queue from anywhere in the app. Your phone and this computer find each other through Stash's link service, encrypted end to end.</span>
				</div>
				<button type="button" class="btn" onclick={() => act(core.linkPair)}>Link your phone</button>
			</div>
		{/if}
		{#if view.problem || failure}<div class="set-row"><span class="set-sub" role="alert">{view.problem ?? failure}</span></div>{/if}
	{/if}
</div>

<style>
	.yp-code { align-items: flex-start; justify-content: flex-start; gap: 20px; padding-top: 18px; padding-bottom: 18px; }
	.yp-qr { flex: none; width: 200px; height: 200px; border-radius: 8px; }
	.yp-digits { margin: 8px 0 0; font: 700 32px/1 var(--font-display, var(--font-body)); letter-spacing: .08em; font-variant-numeric: tabular-nums; color: var(--wp-text); }
</style>
