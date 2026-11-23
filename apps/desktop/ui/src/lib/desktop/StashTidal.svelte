<!-- Settings › Sources › Tidal: Stash's own Tidal once the listener added it (the core's `tidal.rs`): today's plays, the
     same daily allowance as on the phone, and the supporter's claim that makes it unlimited. Switched off, moved and
     removed in the Add-ons list above, like any add-on. Sources.svelte reads how it stands and shows this part. -->
<script lang="ts">
	import type { TidalStatus } from '../core';
	import * as core from '../core';
	import SupporterClaim from './SupporterClaim.svelte';

	let { t, onchange }: { t: TidalStatus; onchange: () => void } = $props();

	const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
	const allowance = $derived(t.allowance);
	const unlimited = $derived(!!allowance && allowance.limit === null);
	const spent = $derived(!!allowance && allowance.limit !== null && allowance.used >= allowance.limit);
	/** The claim is offered unless a supporter's token is kept and not refused (the allowance still has a limit). */
	const offerClaim = $derived(t.ready && t.can_claim && !unlimited && (!t.supporter || (!!allowance && allowance.limit !== null)));

	let opening = $state(false);
	let checkTrouble = $state<string | null>(null);
	/** Opens the add-on's quick check again, in its own window. */
	async function checkAgain() {
		if (opening) return;
		opening = true;
		checkTrouble = null;
		try {
			await core.checkStashTidal();
			onchange();
		} catch (x) {
			checkTrouble = x instanceof core.Refused ? x.message : 'The Stash core is not answering.';
		} finally {
			opening = false;
		}
	}
</script>

<section class="set-part" id="tidal" aria-labelledby="h-tidal">
	<h3 class="set-h" id="h-tidal">Tidal</h3>
	<div class="set-card">
		<div class="set-row set-col">
			<span class="set-name">Lossless and Hi-Res, by Stash</span>
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
		{#if t.needs_check || t.checking}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">One quick check</span>
					<span class="set-sub" role="status">{t.checking ? 'Finish it in the window that opened.' : 'Tidal asks this computer to pass its check again.'}</span>
					{#if checkTrouble || t.check_trouble}<span class="set-sub set-trouble" role="alert">{checkTrouble ?? t.check_trouble}</span>{/if}
				</div>
				<button type="button" class="btn" disabled={opening || t.checking} onclick={checkAgain}>{t.checking ? 'Checking…' : 'Check'}</button>
			</div>
		{/if}
		{#if offerClaim}
			<div class="set-row set-col">
				<span class="set-sub">Unlimited Tidal for supporters: $5 or more on Ko‑fi.</span>
				<SupporterClaim claiming={t.claiming} {onchange} />
			</div>
		{/if}
	</div>
</section>

<style>
	.tt-now { color: var(--wp-text); }
</style>
