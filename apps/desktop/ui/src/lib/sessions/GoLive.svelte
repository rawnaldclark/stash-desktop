<!-- Go live: this computer's station (made the first time, its name and vibe kept for next time), the DJ name the room shows,
     and what going live means: anyone in Community can tune in; they see the station and the DJ name, never a library or
     accounts; Stash must stay running. Esc or Not now closes it. (The web player's GoLive.svelte; keep them in step.) -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { community } from '../community/key';
	import { ui } from '../ui/state.svelte';
	import { client, MAX_ROOM_NAME, session } from './session.svelte';
	import { sessionsMessage } from './client';
	import { MAX_STATION_NAME, MAX_VIBE, type OwnStation, type StationSummary } from './rules';

	let { onclose }: { onclose: () => void } = $props();

	/**
	 * The dialog lives at the player's root (.wp-frame, which holds the colour tokens), not inside the page: a page's entry
	 * animation is a transform, and `position: fixed` under a transform is fixed to the page, not the window.
	 */
	function toRoot(node: HTMLElement) {
		(document.querySelector('.wp-frame') ?? document.body).appendChild(node);
		return { destroy: () => node.remove() };
	}

	let mine = $state<OwnStation | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let failed = $state<string | null>(null);
	let name = $state('');
	let vibe = $state('');
	let dj = $state(session.name ?? '');
	let first = $state<HTMLInputElement>();

	onMount(() => {
		void (async () => {
			const key = await community.key();
			if (!key) {
				failed = 'Join Community first: it’s how your station is yours.';
				loading = false;
				return;
			}
			try {
				mine = await client.mine(key);
				if (mine) {
					name = mine.name;
					vibe = mine.vibe ?? '';
					if (!dj) dj = mine.hostName ?? '';
				}
			} catch (e) {
				failed = sessionsMessage(e);
			} finally {
				loading = false;
				queueMicrotask(() => first?.focus());
			}
		})();
	});

	async function go(e: SubmitEvent) {
		e.preventDefault();
		if (busy) return;
		const n = name.trim();
		if (!n) return void (failed = 'Name your station.');
		busy = true;
		failed = null;
		try {
			const key = await community.key();
			if (!key) throw new Error('no key');
			const v = vibe.trim();
			const host = dj.trim();
			let station: StationSummary;
			if (!mine) {
				const id = await client.create(key, { name: n, vibe: v || null, hostName: host || null });
				station = { id, name: n, hostName: host || null, vibe: v || null, up: 0, down: 0, followers: 0 };
			} else {
				const changed = n !== mine.name || (v || null) !== mine.vibe || (host || null) !== mine.hostName;
				station = changed ? await client.edit(key, mine.id, { name: n, vibe: v || null, hostName: host || null }) : mine;
			}
			session.setName(host);
			if (await session.goLive(station)) {
				ui.setTab('room');
				onclose();
			}
		} catch (err) {
			failed = sessionsMessage(err);
		} finally {
			busy = false;
		}
	}

	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.stopPropagation();
			onclose();
		}
	}
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="ss-scrim" use:toRoot onkeydown={key} onclick={(e) => e.target === e.currentTarget && onclose()} role="presentation">
	<div class="ss-dlg" role="dialog" aria-modal="true" aria-labelledby="gl-h">
	<form class="ss-dlg-f" onsubmit={go}>
		<h2 id="gl-h">Go live</h2>
		<p class="ss-muted">Anyone in Community can tune in and hear what you play, as you play it. They see your station and your DJ name, never your library, playlists or accounts.</p>
		{#if loading}
			<p class="ss-fine" aria-busy="true">Finding your station…</p>
		{:else}
			<label class="ss-fld">Name your station<input bind:this={first} bind:value={name} maxlength={MAX_STATION_NAME} placeholder="Rawn’s Basement" required /></label>
			<label class="ss-fld">What’s playing tonight?<input bind:value={vibe} maxlength={MAX_VIBE} placeholder="dusty soul and loops, requests open" /><small>Optional · {vibe.length} / {MAX_VIBE} · Shown on your Live card and in the room. Kept for next time.</small></label>
			<label class="ss-fld">Your DJ name<input bind:value={dj} maxlength={MAX_ROOM_NAME} placeholder="Your name" autocomplete="nickname" /></label>
			<p class="ss-note">You’re live from Stash on this computer. Quit Stash, or stop playing for 15 minutes, and the session ends. Your station plays whatever Stash plays.</p>
		{/if}
		{#if failed}<p class="ss-err" role="alert">{failed}</p>{/if}
		<div class="ss-dlg-acts">
			<button type="button" class="btn ghost" onclick={onclose}>Not now</button>
			<button type="submit" class="btn" disabled={loading || busy} aria-busy={busy}>{mine ? 'Go live' : 'Make my station and go live'}</button>
		</div>
	</form>
	</div>
</div>
