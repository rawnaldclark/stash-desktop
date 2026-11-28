<!-- Join Community: one sentence on what it is for, and Join. Nothing is sent before; Join has the core make this computer's
     key, which is its own (never a phone's). The web player's community/JoinCommunity.svelte, for this computer. -->
<script lang="ts">
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';
	import { community } from './key';

	let { lead }: { /** The sentence over Join: what joining is for */ lead: string } = $props();
	let busy = $state(false);

	async function join() {
		if (busy) return;
		busy = true;
		try {
			await community.join();
		} catch {
			ui.toast('Couldn’t join Community. Try again.');
		} finally {
			busy = false;
		}
	}
</script>

<section class="cj" aria-label="Join Community">
	<span class="cj-ic" aria-hidden="true"><Icon name="groups" /></span>
	<p class="cj-h">{lead}</p>
	<button type="button" class="btn cj-go" disabled={busy} aria-busy={busy} onclick={join}>Join Community</button>
	<p class="cj-fine">Joining makes a private key on this computer so your votes count once. Nothing is sent until you join.</p>
</section>

<style>
	.cj { display: grid; justify-items: center; gap: 16px; max-width: 520px; margin: 24px auto 0; text-align: center; }
	.cj-ic { display: grid; place-items: center; width: 64px; height: 64px; border-radius: 50%; background: var(--acc-wash, rgb(124 58 237 / .16)); color: var(--acc-text, var(--wp-text)); }
	.cj-ic :global(.ic) { width: 34px; height: 34px; }
	.cj-h { margin: 0; font: 500 clamp(20px, 2.4vw, 26px)/1.3 var(--font-display); letter-spacing: -.01em; color: var(--wp-text); text-wrap: balance; }
	.cj-go { height: 44px; padding: 0 24px; border-radius: 22px; font-size: 14px; }
	.cj-go:disabled { opacity: .7; cursor: progress; }
	.cj-fine { margin: 0; max-width: 42ch; font: 400 13px/1.5 var(--font-body); color: var(--wp-text-2); }
</style>
