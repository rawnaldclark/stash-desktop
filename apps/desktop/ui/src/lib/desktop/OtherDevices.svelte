<!-- Settings › Library: the library on the listener's other devices. Away from home, through Stash's relay (the core's
     `away.rs`; for supporters, claimed here by email as on the phone); on their Wi‑Fi (`lan.rs`), or anywhere with
     Tailscale; and the devices let in, each with its own user name and password, which a Subsonic app (Symfonium,
     Feishin, Substreamer, DSub…) signs in with. A device removed is let in no more. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { ui } from '../ui/state.svelte';
	import SupporterClaim from './SupporterClaim.svelte';

	type Device = { id: string; name: string; user: string; password: string; added_ms: number; seen_ms: number | null };
	type Lan = {
		on: boolean;
		port: number;
		listening: number | null;
		trouble: string | null;
		address: string | null;
		url: string | null;
		tailscale: { name: string; url: string | null } | null;
		devices: Device[];
		/** For smaller songs on mobile data (the core's `transcode.rs`). */
		ffmpeg: { found: boolean; can_fetch: boolean; fetching: { got: number; total: number | null } | null; trouble: string | null };
	};
	type Away = { available: boolean; can_claim: boolean; on: boolean; supporter: boolean; claiming: string | null; connected: boolean; trouble: string | null; url: string | null };

	let lan = $state<Lan | null>(null);
	let away = $state<Away | null>(null);
	let trouble = $state<string | null>(null);
	let busy = $state(false);
	/** The device whose password shows, the one being removed (waiting for a yes), what was just copied. */
	let shown = $state<string | null>(null);
	let removing = $state<string | null>(null);
	let copied = $state<string | null>(null);
	let adding = $state(false);
	let name = $state('');
	let portText = $state('');
	let editingPort = $state(false);
	let renewing = $state(false);

	async function ask() {
		try {
			[lan, away] = await Promise.all([core.get<Lan>('/api/lan'), core.get<Away>('/api/away')]);
		} catch {
			/* the core is not answering */
		}
	}

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const loop = async () => {
			await ask();
			const settling = (!!away && away.on && away.supporter && !away.connected && !away.trouble) || !!lan?.ffmpeg.fetching;
			if (alive) timer = setTimeout(loop, settling ? 1500 : 5000);
		};
		void loop();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	async function act(what: () => Promise<unknown>) {
		busy = true;
		trouble = null;
		try {
			await what();
		} catch (e) {
			trouble = core.said(e);
		} finally {
			busy = false;
			removing = null;
			await ask();
		}
	}

	const setLan = (on: boolean) => act(() => core.post('/api/lan', { on }));
	const setAway = (on: boolean) => act(() => core.post('/api/away', { on }));
	const newAddress = () => act(() => core.post('/api/away/new-address', {})).then(() => (renewing = false));
	const fetchFfmpeg = () => act(() => core.mornyeFetchFfmpeg());

	async function addDevice() {
		const n = name.trim();
		if (!n) return;
		await act(async () => {
			const got = await core.post<Lan>('/api/lan/devices', { name: n });
			const added = got.devices[got.devices.length - 1];
			if (added) shown = added.id;
		});
		if (!trouble) {
			adding = false;
			name = '';
		}
	}
	const remove = (d: Device) => act(() => core.post(`/api/lan/devices/${d.id}/remove`, {}));
	const newPassword = (d: Device) => act(() => core.post(`/api/lan/devices/${d.id}/password`, {})).then(() => (shown = d.id));
	async function savePort() {
		const port = Number(portText);
		if (!Number.isInteger(port) || port < 1024 || port > 65535) {
			trouble = 'Pick a port from 1024 to 65535.';
			return;
		}
		await act(() => core.post('/api/lan', { port }));
		if (!trouble) editingPort = false;
	}

	async function copy(text: string, what: string) {
		try {
			await navigator.clipboard.writeText(text);
			copied = what;
			setTimeout(() => {
				if (copied === what) copied = null;
			}, 1500);
		} catch {
			trouble = 'Could not copy: select the text and copy it yourself.';
		}
	}

	const ago = (ms: number) => {
		const minutes = Math.floor((Date.now() - ms) / 60_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes} min ago`;
		const hours = Math.floor(minutes / 60);
		return hours < 24 ? `${hours} h ago` : `${Math.floor(hours / 24)} d ago`;
	};
	const sharing = $derived(!!lan && (lan.on || (!!away && away.on && away.supporter)));
</script>

{#snippet address(url: string, what: string)}
	<button type="button" class="od-copy" title="Copy the address" onclick={() => copy(url, what)}>{url}</button>{copied === what ? ' · copied' : ''}
{/snippet}

{#if lan && away}
	<div class="set-card">
		<div class="set-row set-col">
			<span class="set-name">Your library on other devices</span>
			<span class="set-sub">Play it on your phone or another computer with a Subsonic app, such as Symfonium, Feishin, Substreamer or DSub. This computer has to be on, with Stash open.</span>
		</div>

		<!-- Away from home -->
		{#if away.available}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name" id="away-l">Away from home</span>
					{#if !away.supporter}
						<span class="set-sub">Anywhere with a connection, through Stash’s relay. For supporters: anyone who gave $5 or more on Ko‑fi.</span>
					{:else if !away.on}
						<span class="set-sub">Anywhere with a connection, through Stash’s relay</span>
					{:else if away.connected && away.url}
						<span class="set-sub od-now" role="status">Ready at {@render address(away.url, 'away')}</span>
						<span class="set-sub">Your music passes through Stash’s relay on its way to your phone. Only devices you add below get in.</span>
					{:else if away.trouble}
						<span class="set-sub set-trouble" role="status">Not connected: {away.trouble}</span>
					{:else}
						<span class="set-sub" role="status">Connecting…</span>
					{/if}
				</div>
				{#if away.supporter}
					<button type="button" class="sw" role="switch" aria-checked={away.on} aria-labelledby="away-l" disabled={busy} onclick={() => setAway(!away!.on)}><i></i></button>
				{/if}
			</div>
			{#if !away.supporter && away.can_claim}
				<div class="set-row set-col od-sub">
					<SupporterClaim claiming={away.claiming} onchange={ask} onunlocked={() => core.post('/api/away', { on: true })} />
				</div>
			{/if}
			{#if away.on && away.supporter && away.url}
				<div class="set-row od-sub">
					{#if renewing}
						<span class="set-sub od-ask" role="alert">Make a new address? The old one stops working, so every device needs the new one.</span>
						<div class="set-ctl">
							<button type="button" class="btn" disabled={busy} onclick={newAddress}>New address</button>
							<button type="button" class="btn ghost" onclick={() => (renewing = false)}>Cancel</button>
						</div>
					{:else}
						<span class="set-sub">Shared the address by mistake? Make a new one.</span>
						<div class="set-ctl"><button type="button" class="btn ghost" disabled={busy} onclick={() => (renewing = true)}>New address…</button></div>
					{/if}
				</div>
			{/if}
		{/if}

		<!-- On the Wi‑Fi (and with Tailscale, anywhere) -->
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="lan-l">On your Wi‑Fi</span>
				{#if !lan.on}
					<span class="set-sub">Straight from this computer to devices on the same network{lan.tailscale ? ', or on your Tailscale network' : ''}</span>
				{:else if lan.trouble}
					<span class="set-sub set-trouble" role="status">{lan.trouble}</span>
				{:else if lan.url}
					<span class="set-sub od-now" role="status">Ready at {@render address(lan.url, 'lan')}</span>
				{:else if lan.listening}
					<span class="set-sub od-now" role="status">Sharing on port {lan.listening}, but this computer isn’t on a network right now</span>
				{:else}
					<span class="set-sub" role="status">Starting…</span>
				{/if}
				{#if lan.on && lan.tailscale?.url}
					<span class="set-sub">With Tailscale on your phone too: {@render address(lan.tailscale.url, 'ts')}</span>
				{/if}
			</div>
			<button type="button" class="sw" role="switch" aria-checked={lan.on} aria-labelledby="lan-l" disabled={busy} onclick={() => setLan(!lan!.on)}><i></i></button>
		</div>
		{#if lan.on}
			<div class="set-row od-sub">
				<div class="set-lab">
					<span class="set-sub">Port {lan.port}. Change it only if another program uses it. Windows may ask once whether Stash can use your network: allow it on private networks.</span>
				</div>
				<div class="set-ctl">
					{#if editingPort}
						<form class="od-add" onsubmit={(e) => { e.preventDefault(); void savePort(); }}>
							<input class="od-input od-port" bind:value={portText} inputmode="numeric" aria-label="Port" />
							<button type="submit" class="btn" disabled={busy}>Save</button>
							<button type="button" class="btn ghost" onclick={() => (editingPort = false)}>Cancel</button>
						</form>
					{:else}
						<button type="button" class="btn ghost" disabled={busy} onclick={() => { portText = String(lan!.port); editingPort = true; }}>Change port</button>
					{/if}
				</div>
			</div>
		{/if}

		<!-- The devices let in -->
		{#if sharing}
			{#each lan.devices as d (d.id)}
				<div class="set-row od-sub">
					<div class="set-lab">
						<span class="set-name">{d.name}</span>
						<span class="set-sub">
							User name <button type="button" class="od-copy" title="Copy the user name" onclick={() => copy(d.user, `u-${d.id}`)}>{d.user}</button>
							· Password
							{#if shown === d.id}
								<button type="button" class="od-copy" title="Copy the password" onclick={() => copy(d.password, `p-${d.id}`)}>{d.password}</button>
							{:else}
								<button type="button" class="od-show" onclick={() => (shown = d.id)}>Show</button>
							{/if}
							{copied === `u-${d.id}` || copied === `p-${d.id}` ? ' · copied' : ''}
						</span>
						<span class="set-sub">{d.seen_ms ? `Last connected ${ago(d.seen_ms)}` : 'Not connected yet'}</span>
						{#if removing === d.id}
							<span class="set-sub od-ask" role="alert">Remove {d.name}? It can’t play your library any more, until you add it again.</span>
						{/if}
					</div>
					<div class="set-ctl">
						{#if removing === d.id}
							<button type="button" class="btn" disabled={busy} onclick={() => remove(d)}>Remove</button>
							<button type="button" class="btn ghost" onclick={() => (removing = null)}>Cancel</button>
						{:else}
							{#if shown === d.id}<button type="button" class="btn ghost" onclick={() => (shown = null)}>Hide</button>{/if}
							<button type="button" class="btn ghost" disabled={busy} onclick={() => newPassword(d)}>New password</button>
							<button type="button" class="btn ghost" disabled={busy} onclick={() => (removing = d.id)}>Remove</button>
						{/if}
					</div>
				</div>
			{/each}
			<div class="set-row od-sub">
				{#if adding}
					<form class="od-add" onsubmit={(e) => { e.preventDefault(); void addDevice(); }}>
						<label class="set-sub" for="od-name">Name the device</label>
						<input id="od-name" class="od-input" bind:value={name} maxlength="40" placeholder="My phone" autocomplete="off" />
						<button type="submit" class="btn" disabled={busy || !name.trim()}>Add</button>
						<button type="button" class="btn ghost" onclick={() => { adding = false; name = ''; }}>Cancel</button>
					</form>
				{:else}
					<div class="set-lab">
						<span class="set-sub">{lan.devices.length ? 'Each device signs in with its own user name and password.' : 'Add a device to get the user name and password its app signs in with.'} In the app, add a server: an address above, then the user name and password.</span>
					</div>
					<div class="set-ctl"><button type="button" class="btn ghost" disabled={busy || lan.devices.length >= 32} onclick={() => (adding = true)}>Add device…</button></div>
				{/if}
			</div>

			<!-- Smaller songs on mobile data: the app asks, FFmpeg makes them -->
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">Smaller songs on mobile data</span>
					{#if lan.ffmpeg.found}
						<span class="set-sub">Ready. In your app, pick a smaller quality for mobile data (in Tempo and Symfonium, under transcoding).</span>
					{:else if lan.ffmpeg.fetching}
						<span class="set-sub" role="status">Downloading FFmpeg… {(lan.ffmpeg.fetching.got / 1048576).toFixed(0)}{lan.ffmpeg.fetching.total ? ` of ${(lan.ffmpeg.fetching.total / 1048576).toFixed(0)}` : ''} MB</span>
					{:else}
						<span class="set-sub">Apps can ask for smaller songs on mobile data. Stash needs FFmpeg to make them; until then songs go at full size.{lan.ffmpeg.can_fetch ? '' : ' Install FFmpeg on this computer.'}</span>
					{/if}
					{#if lan.ffmpeg.trouble}<span class="set-sub set-trouble" role="alert">{lan.ffmpeg.trouble}</span>{/if}
				</div>
				{#if !lan.ffmpeg.found && !lan.ffmpeg.fetching && lan.ffmpeg.can_fetch}
					<div class="set-ctl"><button type="button" class="btn ghost" disabled={busy || ui.offline} onclick={fetchFfmpeg}>Download (160 MB)</button></div>
				{/if}
			</div>
		{/if}
		{#if trouble}
			<div class="set-row"><span class="set-sub set-trouble" role="alert">{trouble}</span></div>
		{/if}
	</div>
{/if}

<style>
	.od-now { color: var(--wp-text); }
	.od-ask { margin: 0; color: var(--wp-text); }
	.od-sub { padding-left: 36px; }
	.od-copy { font: 500 13px/1.3 var(--font-mono, ui-monospace, monospace); color: var(--wp-text); text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--wp-line-2); user-select: all; word-break: break-all; }
	.od-show { color: var(--wp-text); text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--wp-line-2); }
	.od-add { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
	.od-input { min-width: 0; width: 220px; padding: 7px 10px; border: 1px solid var(--wp-line-2); border-radius: 8px; background: transparent; color: var(--wp-text); font: 400 14px/1.3 var(--font-body); }
	.od-port { width: 110px; }
	.od-input:focus-visible { outline: 2px solid var(--acc); outline-offset: 1px; }
</style>
