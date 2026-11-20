<!-- Settings › Sources › Tidal: Stash's own Tidal (the core's `tidal.rs`), lossless for everyone with the same daily
     allowance as on the phone, unlimited for supporters. Shown only in a build that has it; switched on and off in the
     Add-ons list above, where it sits before YouTube Music. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import SupporterClaim from './SupporterClaim.svelte';

	type Tidal = {
		available: boolean;
		ready: boolean;
		on: boolean;
		trouble: string | null;
		allowance: { used: number; limit: number | null; resets_at_ms: number } | null;
		supporter: boolean;
		can_claim: boolean;
		claiming: string | null;
	};

	let t = $state<Tidal | null>(null);
	/** Until this time, the core is read often: a claim just went through, and the allowance is being asked again. */
	let settleUntil = 0;

	async function ask() {
		try {
			t = await core.get<Tidal>('/api/tidal');
		} catch {
			/* the core is not answering */
		}
	}

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const loop = async () => {
			await ask();
			if (alive) timer = setTimeout(loop, Date.now() < settleUntil ? 1500 : 30_000);
		};
		void loop();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	async function changed() {
		settleUntil = Date.now() + 15_000;
		await ask();
	}

	const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
	const allowance = $derived(t?.allowance ?? null);
	const unlimited = $derived(!!allowance && allowance.limit === null);
	const spent = $derived(!!allowance && allowance.limit !== null && allowance.used >= allowance.limit);
	/** The claim is offered unless a supporter's token is kept and not refused (the allowance still has a limit). */
	const offerClaim = $derived(!!t && t.ready && t.can_claim && !unlimited && (!t.supporter || (!!allowance && allowance.limit !== null)));
</script>

{#if t?.available}
	<section class="set-part" id="tidal" aria-labelledby="h-tidal">
		<h3 class="set-h" id="h-tidal">Tidal</h3>
		<div class="set-card">
			<div class="set-row set-col">
				<span class="set-name">Lossless and Hi-Res, built in</span>
				{#if !t.ready}
					<span class="set-sub" class:set-trouble={!!t.trouble} role="status">{t.trouble ? `Can’t be used right now: ${t.trouble}` : 'Getting ready…'}</span>
				{:else if !t.on}
					<span class="set-sub">Switched off. Switch it on in Add-ons, above.</span>
				{:else if unlimited}
					<span class="set-sub tt-now" role="status">Supporter · unlimited Tidal</span>
				{:else if allowance && spent}
					<span class="set-sub tt-now" role="status">Today’s {allowance.limit} plays are used up. Songs come from your other sources until {time(allowance.resets_at_ms)}.</span>
				{:else if allowance}
					<span class="set-sub tt-now" role="status">Today: {allowance.used} of {allowance.limit} plays · resets {time(allowance.resets_at_ms)}</span>
				{:else}
					<span class="set-sub">Free, with a daily allowance</span>
				{/if}
			</div>
			{#if offerClaim}
				<div class="set-row set-col">
					<span class="set-sub">Unlimited Tidal for supporters: $5 or more on Ko‑fi.</span>
					<SupporterClaim claiming={t.claiming} onchange={changed} />
				</div>
			{/if}
		</div>
	</section>
{/if}

<style>
	.tt-now { color: var(--wp-text); }
</style>
