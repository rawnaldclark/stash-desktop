<!-- Settings › Library: the music in the listener's own Dropbox and OneDrive (the core's `cloud.rs`). Connect an account
     (its own sign-in, in a window of Stash's), pick folders of music, and every step says how it goes: the sign-in, each
     folder's scan (files met, songs added, the file being read), and what went wrong, in words. Songs from the files join
     the library and play from the files. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { songs } from '../ui/format';

	type Progress = { files: number; read: number; added: number; left: number; file: string | null };
	type Folder = { id: number; path: string; songs: number; scanned_at_ms: number | null; trouble: string | null; scanning: Progress | null; waiting: boolean; last: Progress | null };
	type Account = { id: number; provider: string; name: string; email: string | null; signed_in: boolean; folders: Folder[] };
	type Told = { provider: string; ok: boolean; message: string; at_ms: number };
	type Cloud = { providers: { id: string; name: string }[]; window: boolean; connecting: string | null; told: Told | null; offline: boolean; accounts: Account[] };
	type Entry = { id: string; name: string; path: string };
	type Place = { id: string; name: string; path: string };

	let cloud = $state<Cloud | null>(null);
	let trouble = $state<string | null>(null);
	/** The account being disconnected, or the folder being removed, waiting for a yes. */
	let confirming = $state<string | null>(null);
	let busy = $state(false);
	/** The folder picker: the account, where it is (top first), what is in there. */
	let picker = $state<{ account: Account; trail: Place[]; folders: Entry[]; songsHere: number; loading: boolean; error: string | null } | null>(null);

	async function ask() {
		try {
			cloud = await core.get<Cloud>('/api/cloud');
		} catch {
			/* no cloud storage in this build, or the core is not answering */
		}
	}

	onMount(() => {
		let alive = true;
		let timer: ReturnType<typeof setTimeout>;
		const loop = async () => {
			await ask();
			const busyNow = !!cloud && (!!cloud.connecting || cloud.accounts.some((a) => a.folders.some((f) => f.scanning || f.waiting)));
			if (alive) timer = setTimeout(loop, busyNow ? 1000 : 4000);
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
			confirming = null;
			await ask();
		}
	}

	const connect = (provider: string) => act(() => core.post('/api/cloud/connect', { provider }));
	const disconnect = (a: Account) => act(() => core.post(`/api/cloud/accounts/${a.id}/disconnect`, {}));
	const removeFolder = (f: Folder) => act(() => core.post(`/api/cloud/folders/${f.id}/remove`, {}));
	const scanNow = (f?: Folder) => act(() => core.post('/api/cloud/scan', f ? { folder: f.id } : {}));

	async function browse(account: Account, trail: Place[]) {
		picker = { account, trail, folders: [], songsHere: 0, loading: true, error: null };
		const here = trail[trail.length - 1];
		try {
			const got = await core.get<{ folders: Entry[]; songs_here: number }>(`/api/cloud/accounts/${account.id}/browse?folder=${encodeURIComponent(here.id)}`);
			if (picker?.account.id === account.id && picker.trail === trail) picker = { ...picker, folders: got.folders, songsHere: got.songs_here, loading: false };
		} catch (e) {
			if (picker) picker = { ...picker, loading: false, error: core.said(e) };
		}
	}
	const providerName = (id: string) => cloud?.providers.find((p) => p.id === id)?.name ?? id;
	const openPicker = (a: Account) => void browse(a, [{ id: '', name: providerName(a.provider), path: '' }]);
	const into = (e: Entry) => picker && void browse(picker.account, [...picker.trail, { id: e.id, name: e.name, path: e.path }]);
	const backTo = (i: number) => picker && void browse(picker.account, picker.trail.slice(0, i + 1));
	async function pick() {
		if (!picker) return;
		const here = picker.trail[picker.trail.length - 1];
		const account = picker.account;
		await act(() => core.post(`/api/cloud/accounts/${account.id}/folders`, { id: here.id, path: here.path }));
		if (!trouble) picker = null;
	}

	const ago = (ms: number) => {
		const minutes = Math.floor((Date.now() - ms) / 60_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes} min ago`;
		const hours = Math.floor(minutes / 60);
		return hours < 24 ? `${hours} h ago` : `${Math.floor(hours / 24)} d ago`;
	};
	/** How a folder's scan stands, in a line. */
	function folderLine(f: Folder): string {
		if (f.scanning) {
			const p = f.scanning;
			const parts = [`${p.files.toLocaleString()} ${p.files === 1 ? 'file' : 'files'} looked at`, p.added ? `${songs(p.added)} added` : ''].filter(Boolean).join(' · ');
			return `Scanning: ${parts}${p.file ? ` · reading ${p.file}` : ''}`;
		}
		if (f.waiting) return cloud?.offline ? 'Waits until Stash is back online' : 'Waiting to scan…';
		const kept = `${songs(f.songs)}${f.scanned_at_ms ? ` · scanned ${ago(f.scanned_at_ms)}` : ''}`;
		if (f.last && (f.last.added || f.last.left)) {
			const changed = [f.last.added ? `${f.last.added} new` : '', f.last.left ? `${f.last.left} gone` : ''].filter(Boolean).join(', ');
			return `${kept} (${changed})`;
		}
		return kept;
	}
	const accountsOf = (provider: string) => cloud?.accounts.filter((a) => a.provider === provider) ?? [];
</script>

{#if cloud && cloud.providers.length}
	<div class="set-card">
		{#each cloud.providers as p (p.id)}
			{@const told = cloud.told?.provider === p.id ? cloud.told : null}
			{@const accounts = accountsOf(p.id)}
			{@const waiting = cloud.connecting === p.id}
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name">{p.name}</span>
					{#if accounts.length === 0}
						<span class="set-sub">Play the music in your {p.name}. Pick its folders; the songs join your library and play from there.</span>
					{/if}
					{#each accounts as a (a.id)}
						<span class="set-sub">Connected as {a.name}{a.email ? ` · ${a.email}` : ''}</span>
						{#if !a.signed_in}<span class="set-sub set-trouble" role="status">{p.name} stopped taking Stash’s sign-in. Sign in again to keep playing these songs.</span>{/if}
					{/each}
					{#if waiting}
						<span class="set-sub cs-now" role="status">Finish signing in in the {p.name} window…</span>
					{:else if told}
						<span class="set-sub" class:set-trouble={!told.ok} class:cs-now={told.ok} role="status">{told.message}{told.ok && accounts.every((a) => a.folders.length === 0) ? '. Now pick a folder of music.' : ''}</span>
					{/if}
				</div>
				<div class="set-ctl">
					{#if accounts.length === 0 || accounts.some((a) => !a.signed_in)}
						<button type="button" class="btn" disabled={busy || waiting || !cloud.window || cloud.offline} onclick={() => connect(p.id)}>{waiting ? 'Signing in…' : accounts.length ? 'Sign in again' : 'Connect'}</button>
					{/if}
					{#each accounts as a (a.id)}
						{#if confirming === `account-${a.id}`}
							<button type="button" class="btn" disabled={busy} onclick={() => disconnect(a)}>Disconnect</button>
							<button type="button" class="btn ghost" onclick={() => (confirming = null)}>Cancel</button>
						{:else}
							{#if a.signed_in}<button type="button" class="btn ghost" disabled={busy || !!picker} onclick={() => openPicker(a)}>Add folder…</button>{/if}
							<button type="button" class="btn ghost" disabled={busy} onclick={() => (confirming = `account-${a.id}`)}>Disconnect</button>
						{/if}
					{/each}
				</div>
				{#each accounts as a (a.id)}
					{#if confirming === `account-${a.id}`}
						<p class="set-sub cs-ask" role="alert">Disconnect {p.name}? Its songs leave your library, except ones you liked, saved to this computer or put in a playlist. Nothing in your {p.name} changes.</p>
					{/if}
				{/each}
			</div>
			{#each accounts as a (a.id)}
				{#each a.folders as f (f.id)}
					<div class="set-row cs-folder">
						<div class="set-lab">
							<span class="set-name set-path" title={f.path || `All of ${p.name}`}>{f.path || `All of ${p.name}`}</span>
							{#if f.trouble && !f.scanning}
								<span class="set-sub set-trouble" role="status">Problem: {f.trouble}</span>
							{:else}
								<span class="set-sub set-path" class:cs-now={!!f.scanning} role="status" title={folderLine(f)}>{folderLine(f)}</span>
							{/if}
							{#if f.scanning}<div class="cs-bar" aria-hidden="true"><i></i></div>{/if}
							{#if confirming === `folder-${f.id}`}
								<span class="set-sub cs-ask" role="alert">Remove this folder? Its songs leave your library, except ones you liked, saved or put in a playlist.</span>
							{/if}
						</div>
						<div class="set-ctl">
							{#if confirming === `folder-${f.id}`}
								<button type="button" class="btn" disabled={busy} onclick={() => removeFolder(f)}>Remove</button>
								<button type="button" class="btn ghost" onclick={() => (confirming = null)}>Cancel</button>
							{:else}
								{#if !f.scanning && !f.waiting && a.signed_in}<button type="button" class="btn ghost" disabled={busy || cloud.offline} onclick={() => scanNow(f)}>{f.trouble ? 'Try again' : 'Scan now'}</button>{/if}
								<button type="button" class="btn ghost" disabled={busy} onclick={() => (confirming = `folder-${f.id}`)}>Remove</button>
							{/if}
						</div>
					</div>
				{/each}
				{#if picker && picker.account.id === a.id}
					<div class="set-row set-col cs-pick" role="group" aria-label="Pick a folder of music">
						<nav class="cs-trail" aria-label="Where you are">
							{#each picker.trail as place, i (place.id + i)}
								{#if i > 0}<span aria-hidden="true">›</span>{/if}
								<button type="button" class="cs-crumb" disabled={i === picker.trail.length - 1} onclick={() => backTo(i)}>{place.name}</button>
							{/each}
						</nav>
						{#if picker.loading}
							<span class="set-sub" role="status">Opening…</span>
						{:else if picker.error}
							<span class="set-sub set-trouble" role="alert">{picker.error}</span>
						{:else}
							<span class="set-sub">{picker.songsHere ? `${songs(picker.songsHere)} right here. ` : ''}{picker.folders.length ? 'Open a folder, or use this one: everything inside it is scanned.' : 'No folders inside. Use this one?'}</span>
							{#if picker.folders.length}
								<ul class="cs-list">
									{#each picker.folders as e (e.id)}
										<li><button type="button" class="cs-item" onclick={() => into(e)}>{e.name}</button></li>
									{/each}
								</ul>
							{/if}
						{/if}
						<div class="set-ctl">
							<button type="button" class="btn" disabled={busy || picker.loading} onclick={pick}>Use {picker.trail.length > 1 ? `“${picker.trail[picker.trail.length - 1].name}”` : `all of ${picker.trail[0].name}`}</button>
							<button type="button" class="btn ghost" onclick={() => (picker = null)}>Cancel</button>
						</div>
					</div>
				{/if}
			{/each}
		{/each}
		{#if trouble}
			<div class="set-row"><span class="set-sub set-trouble" role="alert">{trouble}</span></div>
		{/if}
		{#if cloud.offline && cloud.accounts.length}
			<div class="set-row"><span class="set-sub">Stash is offline: cloud songs don’t play, and folders aren’t scanned, until it’s back online.</span></div>
		{/if}
	</div>
{/if}

<style>
	.cs-now { color: var(--wp-text); }
	.cs-ask { flex-basis: 100%; margin: 0; color: var(--wp-text); }
	.cs-folder { padding-left: 36px; }
	.cs-bar { position: relative; height: 3px; margin-top: 6px; border-radius: 2px; background: var(--wp-hover); overflow: hidden; max-width: 320px; }
	.cs-bar i { position: absolute; left: -35%; width: 35%; height: 100%; background: var(--acc); animation: cs-move 1.2s var(--ease-standard) infinite; }
	@keyframes cs-move { from { left: -35%; } to { left: 100%; } }
	@media (prefers-reduced-motion: reduce) { .cs-bar i { animation: none; left: 0; width: 100%; opacity: 0.5; } }
	.cs-pick { padding-left: 36px; }
	.cs-trail { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 6px; font: 500 14px/1.3 var(--font-body); color: var(--wp-text-2); }
	.cs-crumb { color: var(--wp-text); text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--wp-line-2); }
	.cs-crumb:disabled { text-decoration: none; cursor: default; font-weight: 600; }
	.cs-list { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; max-height: 280px; overflow: auto; border: 1px solid var(--wp-line); border-radius: 10px; }
	.cs-item { width: 100%; text-align: left; padding: 9px 12px; font: 400 14px/1.3 var(--font-body); color: var(--wp-text); border-radius: 8px; }
	@media (hover: hover) and (pointer: fine) { .cs-item:hover { background: var(--wp-hover); } }
</style>
