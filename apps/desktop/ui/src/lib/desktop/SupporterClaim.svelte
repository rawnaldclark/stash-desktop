<!-- The supporter claim, as on the phone: the email used on Ko‑fi, then the code the tip jar sends there. The core keeps
     the token it gives (`away.rs`): Stash's relay lets a supporter in by it, and Stash's Tidal is unlimited with it. -->
<script lang="ts">
	import * as core from '../core';

	let {
		claiming,
		ask = 'Already a supporter? Enter the email you used on Ko‑fi',
		onchange,
		onunlocked,
	}: {
		/** The email a code was sent to, while the claim waits for the code. */
		claiming: string | null;
		ask?: string;
		/** After each step: the owner reads the core again. */
		onchange: () => unknown;
		onunlocked?: () => Promise<unknown>;
	} = $props();

	const uid = $props.id();
	let email = $state('');
	let code = $state('');
	let busy = $state(false);
	let trouble = $state<string | null>(null);

	async function act(what: () => Promise<unknown>) {
		busy = true;
		trouble = null;
		try {
			await what();
		} catch (e) {
			trouble = core.said(e);
		} finally {
			busy = false;
			await onchange();
		}
	}
	const sendCode = () => act(() => core.post('/api/away/claim/start', { email: email.trim() }));
	const unlock = () =>
		act(async () => {
			await core.post('/api/away/claim/verify', { code: code.trim() });
			code = '';
			await onunlocked?.();
		});
	const cancel = () => act(() => core.post('/api/away/claim/cancel', {}));
</script>

{#if claiming}
	<form class="sc-form" onsubmit={(e) => { e.preventDefault(); void unlock(); }}>
		<label class="set-sub" for="{uid}-code">Enter the code sent to {claiming}</label>
		<input id="{uid}-code" class="sc-input sc-code" bind:value={code} maxlength="12" autocomplete="one-time-code" inputmode="numeric" />
		<button type="submit" class="btn" disabled={busy || !code.trim()}>Unlock</button>
		<button type="button" class="btn ghost" disabled={busy} onclick={cancel}>Use another email</button>
	</form>
{:else}
	<form class="sc-form" onsubmit={(e) => { e.preventDefault(); void sendCode(); }}>
		<label class="set-sub" for="{uid}-email">{ask}</label>
		<input id="{uid}-email" class="sc-input" type="email" bind:value={email} maxlength="254" autocomplete="email" placeholder="you@example.com" />
		<button type="submit" class="btn" disabled={busy || !email.includes('@')}>Send code</button>
	</form>
{/if}
{#if trouble}<span class="set-sub set-trouble" role="alert">{trouble}</span>{/if}

<style>
	.sc-form { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
	.sc-input { min-width: 0; width: 220px; padding: 7px 10px; border: 1px solid var(--wp-line-2); border-radius: 8px; background: transparent; color: var(--wp-text); font: 400 14px/1.3 var(--font-body); }
	.sc-code { width: 110px; }
	.sc-input:focus-visible { outline: 2px solid var(--acc); outline-offset: 1px; }
</style>
