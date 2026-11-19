<!-- Settings › Library: the library on the listener's other devices (the core's `lan.rs` and `subsonic.rs`). One switch
     shares it on their network; each device they add gets a user name and password of its own, which a Subsonic app on
     the phone or another computer (Symfonium, Feishin, Substreamer, DSub…) signs in with at the address shown. A device
     removed is let in no more. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';

	type Device = { id: string; name: string; user: string; password: string; added_ms: number; seen_ms: number | null };
	type Lan = { on: boolean; port: number; listening: number | null; trouble: string | null; address: string | null; url: string | null; devices: Device[] };

	let lan = $state<Lan | null>(null);
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

	async function ask() {
		try {
			lan = await core.get<Lan>('/api/lan');
		} catch {
			/* the core is not answering */
		}
	}

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const loop = async () => {
			await ask();
			if (alive) timer = setTimeout(loop, 5000);
		};
		void loop();
		return () => {
			alive = false;
			clearTimeout(timer);
		};
	});

	async function act(what: () => Promise<Lan | unknown>) {
		busy = true;
		trouble = null;
		try {
			const got = await what();
			if (got && typeof got === 'object' && 'devices' in got) lan = got as Lan;
		} catch (e) {
			trouble = core.said(e);
		} finally {
			busy = false;
			removing = null;
		}
	}

	const setOn = (on: boolean) => act(() => core.post<Lan>('/api/lan', { on }));
	async function addDevice() {
		const n = name.trim();
		if (!n) return;
		await act(async () => {
			const got = await core.post<Lan>('/api/lan/devices', { name: n });
			const added = got.devices[got.devices.length - 1];
			if (added) shown = added.id;
			return got;
		});
		if (!trouble) {
			adding = false;
			name = '';
		}
	}
	const remove = (d: Device) => act(() => core.post<Lan>(`/api/lan/devices/${d.id}/remove`, {}));
	const newPassword = (d: Device) => act(() => core.post<Lan>(`/api/lan/devices/${d.id}/password`, {})).then(() => (shown = d.id));
	async function savePort() {
		const port = Number(portText);
		if (!Number.isInteger(port) || port < 1024 || port > 65535) {
			trouble = 'Pick a port from 1024 to 65535.';
			return;
		}
		await act(() => core.post<Lan>('/api/lan', { port }));
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
</script>

{#if lan}
	<div class="set-card">
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="lan-l">Your library on other devices</span>
				{#if !lan.on}
					<span class="set-sub">Play it on your phone or another computer on your Wi‑Fi, with a Subsonic app such as Symfonium, Feishin, Substreamer or DSub</span>
				{:else if lan.trouble}
					<span class="set-sub set-trouble" role="status">{lan.trouble}</span>
				{:else if lan.url}
					<span class="set-sub od-now" role="status">Sharing at <button type="button" class="od-copy" title="Copy the address" onclick={() => copy(lan!.url!, 'url')}>{lan.url}</button>{copied === 'url' ? ' · copied' : ''}</span>
					<span class="set-sub">Only on your network, and only to devices you add below. This computer has to be on, with Stash open.</span>
				{:else if lan.listening}
					<span class="set-sub od-now" role="status">Sharing on port {lan.listening}, but this computer isn’t on a network right now</span>
				{:else}
					<span class="set-sub" role="status">Starting…</span>
				{/if}
			</div>
			<button type="button" class="sw" role="switch" aria-checked={lan.on} aria-labelledby="lan-l" disabled={busy} onclick={() => setOn(!lan!.on)}><i></i></button>
		</div>
		{#if lan.on}
			{#each lan.devices as d (d.id)}
				<div class="set-row od-device">
					<div class="set-lab">
						<span class="set-name">{d.name}</span>
						<span class="set-sub">
							User name <button type="button" class="od-copy" title="Copy the user name" onclick={() => copy(d.user, `u-${d.id}`)}>{d.user}</button>
							· Password
							{#if shown === d.id}
								<button type="button" class="od-copy od-pw" title="Copy the password" onclick={() => copy(d.password, `p-${d.id}`)}>{d.password}</button>
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
			<div class="set-row od-device">
				{#if adding}
					<form class="od-add" onsubmit={(e) => { e.preventDefault(); void addDevice(); }}>
						<label class="set-sub" for="od-name">Name the device</label>
						<input id="od-name" class="od-input" bind:value={name} maxlength="40" placeholder="My phone" autocomplete="off" />
						<button type="submit" class="btn" disabled={busy || !name.trim()}>Add</button>
						<button type="button" class="btn ghost" onclick={() => { adding = false; name = ''; }}>Cancel</button>
					</form>
				{:else}
					<div class="set-lab">
						<span class="set-sub">{lan.devices.length ? 'Each device signs in with its own user name and password.' : 'Add a device to get the user name and password its app signs in with.'} In the app, add a server: the address above, then the user name and password.</span>
					</div>
					<div class="set-ctl"><button type="button" class="btn ghost" disabled={busy || lan.devices.length >= 32} onclick={() => (adding = true)}>Add device…</button></div>
				{/if}
			</div>
			<div class="set-row od-device">
				<div class="set-lab">
					<span class="set-name">Port</span>
					<span class="set-sub">Change it only if another program uses {lan.port}. Windows may ask once whether Stash can use your network: allow it on private networks.</span>
				</div>
				<div class="set-ctl">
					{#if editingPort}
						<form class="od-add" onsubmit={(e) => { e.preventDefault(); void savePort(); }}>
							<input class="od-input od-port" bind:value={portText} inputmode="numeric" aria-label="Port" />
							<button type="submit" class="btn" disabled={busy}>Save</button>
							<button type="button" class="btn ghost" onclick={() => (editingPort = false)}>Cancel</button>
						</form>
					{:else}
						<button type="button" class="btn ghost" disabled={busy} onclick={() => { portText = String(lan!.port); editingPort = true; }}>{lan.port}</button>
					{/if}
				</div>
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
	.od-device { padding-left: 36px; }
	.od-copy { font: 500 13px/1.3 var(--font-mono, ui-monospace, monospace); color: var(--wp-text); text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--wp-line-2); user-select: all; }
	.od-pw { letter-spacing: 0.02em; }
	.od-show { color: var(--wp-text); text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--wp-line-2); }
	.od-add { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
	.od-input { min-width: 0; width: 220px; padding: 7px 10px; border: 1px solid var(--wp-line-2); border-radius: 8px; background: transparent; color: var(--wp-text); font: 400 14px/1.3 var(--font-body); }
	.od-port { width: 90px; }
	.od-input:focus-visible { outline: 2px solid var(--acc); outline-offset: 1px; }
</style>
