<!-- Settings › Mornye extensions: the SpotiFLAC-format packages Stash runs itself, as Lokal and Mornye do. Installed
     from a repository (Mornye's own is always there; its SHA-256 checked) or from a file; each one installed and
     switched on is one of your sources. Some ask you to pass their verification page once, in a window of its own. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { ui } from '../ui/state.svelte';

	let m = $state<core.Mornye | null>(null);
	let wrong = $state('');
	let busy = $state<string | null>(null);
	let repoLink = $state('');
	let open = $state<string | null>(null);

	async function look() {
		try {
			m = await core.mornye();
		} catch (e) {
			wrong = core.said(e);
		}
	}
	onMount(() => void look());
	// While the repositories are read, or FFmpeg fetched, in the background, look again.
	const refreshing = $derived(!!m?.refreshing || !!m?.ffmpeg.fetching);
	$effect(() => {
		if (!refreshing) return;
		const again = setInterval(() => void look(), 1500);
		return () => clearInterval(again);
	});

	async function act(what: string, ask: () => Promise<unknown>, after = true) {
		wrong = '';
		busy = what;
		try {
			const got = await ask();
			if (got && typeof got === 'object' && 'installed' in got && Array.isArray((got as core.Mornye).installed)) m = got as core.Mornye;
			else if (after) await look();
		} catch (e) {
			wrong = core.said(e);
		} finally {
			busy = null;
		}
	}

	const installedOf = (id: string) => m?.installed.find((a) => a.name === id) ?? null;
	const available = $derived((m?.catalogue ?? []).filter((e) => !e.installed || e.update));
	const repoName = (id: string) => {
		const r = m?.repositories.find((r) => r.id === id);
		return r ? (r.official ? 'Mornye' : (r.name ?? new URL(r.url).host)) : '';
	};

	async function pickFile(e: Event) {
		const input = e.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (file) await act('file', () => core.mornyeInstallFile(file));
	}

	// ---- one extension's settings, access and actions

	type Panel = {
		key: string;
		values: Record<string, unknown>;
		schema: core.MornyeField[];
		qualities: core.MornyeQuality[];
		actions: { action: string; label?: string }[];
		status: core.MornyeStatus | null;
		said: string;
		form: { action: string; token: string; title: string; fields: core.MornyeField[]; values: Record<string, unknown> } | null;
		pasting: boolean;
		pasted: string;
		page: string;
	};
	let panel = $state<Panel | null>(null);

	async function openPanel(key: string) {
		if (open === key) {
			open = null;
			panel = null;
			return;
		}
		open = key;
		panel = null;
		try {
			const [s, status] = await Promise.all([core.mornyeSettings(key), core.mornyeStatus(key)]);
			panel = {
				key,
				values: { ...s.settings },
				schema: s.schema ?? [],
				qualities: s.quality_options ?? [],
				actions: s.global_actions ?? [],
				status,
				said: '',
				form: null,
				pasting: false,
				pasted: '',
				page: ''
			};
		} catch (e) {
			wrong = core.said(e);
		}
	}

	async function refreshStatus() {
		if (!panel) return;
		try {
			panel.status = await core.mornyeStatus(panel.key);
		} catch {
			/* asked again on the next look */
		}
	}
	// While its verification window is open, look every second and a half.
	const watching = $derived(!!panel?.status?.verifying);
	$effect(() => {
		if (!watching) return;
		const again = setInterval(() => void refreshStatus().then(look), 1500);
		return () => clearInterval(again);
	});

	async function verify() {
		if (!panel) return;
		const p = panel;
		p.said = '';
		try {
			const r = await core.mornyeVerify(p.key);
			if (r.ready) p.said = 'Ready.';
			else if (!r.window) {
				p.page = r.url ?? '';
				p.pasting = true;
			}
			await refreshStatus();
		} catch (e) {
			p.said = core.said(e);
		}
	}

	async function paste() {
		if (!panel) return;
		const p = panel;
		try {
			await core.mornyeCallback(p.key, p.pasted.trim());
			p.pasting = false;
			p.pasted = '';
			p.said = 'Verified.';
			await refreshStatus();
			await look();
		} catch (e) {
			p.said = core.said(e);
		}
	}

	async function save() {
		if (!panel) return;
		const p = panel;
		try {
			const r = await core.mornyeSaveSettings(p.key, p.values);
			p.values = { ...r.settings };
			p.said = 'Saved.';
			await refreshStatus();
			await look();
		} catch (e) {
			p.said = core.said(e);
		}
	}

	async function runAction(action: string, input?: Record<string, unknown>, token?: string) {
		if (!panel) return;
		const p = panel;
		p.said = '';
		try {
			const r = await core.mornyeAction(p.key, action, input, token);
			const form = r.action_form ?? r.byoa_form;
			p.form = form && r.formToken ? { action: form.submit_action, token: r.formToken, title: form.title ?? '', fields: form.fields, values: {} } : null;
			p.said = r.success === false ? (r.error ?? 'That didn’t work.') : (r.message ?? (r.open_auth_url && !r.window ? '' : 'Done.'));
			if (r.open_auth_url && !r.window) {
				p.page = r.open_auth_url;
				p.pasting = true;
			}
			await refreshStatus();
			await look();
		} catch (e) {
			p.said = core.said(e);
		}
	}

	async function logout() {
		if (!panel) return;
		const p = panel;
		try {
			await core.mornyeLogout(p.key);
			p.said = 'Signed out.';
			await refreshStatus();
			await look();
		} catch (e) {
			p.said = core.said(e);
		}
	}

	const labelOf = (f: core.MornyeField) => f.label ?? f.key;
	const optionsOf = (f: core.MornyeField) => (f.options ?? []).map((o) => (typeof o === 'string' ? { value: o, label: o } : { value: o.value, label: o.label ?? o.value }));
	const secretOf = (f: core.MornyeField) => f.secret || f.type === 'password' || f.type === 'otp';
	const stateOf = (a: core.MornyeInstalled) =>
		!a.enabled ? 'Off' : a.access.ready ? 'Ready' : a.access.missing.length ? `Needs ${a.access.missing.join(', ')}` : a.access.kind === 'verification' ? 'Needs verification' : a.access.kind === 'login' ? 'Needs sign-in' : 'Ready';
</script>

<div class="set-card">
	{#if m}
		{#if m.installed.length}
			<ol class="mx-list">
				{#each m.installed as a (a.key)}
					{@const entry = m.catalogue.find((e) => e.id === a.name && e.update)}
					<li class="mx" class:off={!a.enabled}>
						<div class="mx-top">
							<span class="tr-t">
								<b>{a.display_name} <i class="mx-v">{a.version}</i></b>
								<i title={a.description || undefined}>{stateOf(a)}{a.playable ? '' : ' · no search'}</i>
							</span>
							<span class="src-acts">
								{#if a.enabled && !a.access.ready && a.access.kind === 'verification'}
									<button type="button" class="btn" disabled={ui.offline} onclick={async () => { if (open !== a.key) await openPanel(a.key); await verify(); }}>Verify</button>
								{/if}
								{#if entry}<button type="button" class="btn" disabled={busy !== null || ui.offline} onclick={() => act(a.key, () => core.mornyeInstall(entry.repository, entry.id))}>{busy === a.key ? 'Updating…' : `Update to ${entry.version}`}</button>{/if}
								<button type="button" class="btn ghost" aria-expanded={open === a.key} onclick={() => openPanel(a.key)}>{open === a.key ? 'Close' : 'Settings'}</button>
								<button type="button" class="btn ghost" aria-label="Remove {a.display_name}" disabled={busy !== null} onclick={() => act(a.key, () => core.mornyeRemove(a.key))}>Remove</button>
								<button type="button" class="sw" role="switch" aria-checked={a.enabled} aria-label={a.display_name} onclick={() => act(a.key, () => core.mornyeSetEnabled(a.key, !a.enabled))}><i></i></button>
							</span>
						</div>
						{#if open === a.key && panel?.key === a.key}
							{@const p = panel}
							<div class="mx-panel">
								{#if p.status}
									<p class="set-sub">
										{#if a.access.kind === 'verification'}
											{p.status.authenticated ? 'Verified' : p.status.verifying ? 'Finish in the new window…' : 'Not verified'}
										{:else if a.access.kind === 'login'}
											{a.access.verified ? 'Signed in' : 'Not signed in'}
										{/if}
										{#if p.status.result && p.status.result !== 'ok'}<span class="set-trouble">{p.status.result}</span>{/if}
									</p>
								{/if}
								{#if a.access.kind === 'verification'}
									<div class="chips">
										<button type="button" class="btn" disabled={ui.offline} onclick={verify}>{p.status?.authenticated ? 'Verify again' : 'Verify'}</button>
										{#if p.status?.authenticated || a.access.verified}<button type="button" class="btn ghost" onclick={logout}>Forget</button>{/if}
									</div>
								{:else if a.access.kind === 'login' && a.access.verified}
									<div class="chips"><button type="button" class="btn ghost" onclick={logout}>Sign out</button></div>
								{/if}
								{#if p.pasting}
									<div class="mx-paste">
										<p class="set-sub">Open this page in a browser, then paste the address it ends on</p>
										{#if p.page}<input class="dev-input" readonly value={p.page} aria-label="Verification page" onfocus={(e) => e.currentTarget.select()} />{/if}
										<form class="dev-rename" onsubmit={(e) => { e.preventDefault(); void paste(); }}>
											<input class="dev-input" bind:value={p.pasted} placeholder="spotiflac://session-grant?…" aria-label="Address it ended on" autocomplete="off" spellcheck="false" />
											<button type="submit" class="btn" disabled={!p.pasted.trim()}>Done</button>
										</form>
									</div>
								{/if}
								{#if p.qualities.length > 1}
									<label class="mx-field">
										<span>Quality</span>
										<select class="dev-input" bind:value={p.values.downloadQuality}>
											<option value={undefined}>Best available</option>
											{#each p.qualities as q (q.id)}<option value={q.id}>{q.label ?? q.name ?? q.id}</option>{/each}
										</select>
									</label>
								{/if}
								{#each p.schema as f (f.key)}
									{#if f.type === 'button'}
										<div class="chips"><button type="button" class="btn ghost" disabled={ui.offline} onclick={() => runAction(f.action ?? f.key)}>{labelOf(f)}</button></div>
									{:else if f.type === 'boolean'}
										<label class="mx-field mx-check"><input type="checkbox" checked={!!p.values[f.key]} onchange={(e) => (p.values[f.key] = e.currentTarget.checked)} /> <span>{labelOf(f)}</span></label>
									{:else if f.type === 'select'}
										<label class="mx-field"><span>{labelOf(f)}</span>
											<select class="dev-input" bind:value={p.values[f.key]}>
												{#each optionsOf(f) as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
											</select>
										</label>
									{:else}
										<label class="mx-field"><span>{labelOf(f)}{f.required ? ' (required)' : ''}</span>
											<input class="dev-input" type={secretOf(f) ? 'password' : f.type === 'number' ? 'number' : 'text'} value={secretOf(f) ? '' : (p.values[f.key] ?? '')} placeholder={secretOf(f) && p.values[f.key] ? 'Saved · type to change' : (f.placeholder ?? '')} oninput={(e) => (p.values[f.key] = f.type === 'number' ? Number(e.currentTarget.value) : e.currentTarget.value)} autocomplete="off" spellcheck="false" />
										</label>
									{/if}
									{#if f.description ?? f.help}<p class="set-sub mx-help">{f.description ?? f.help}</p>{/if}
								{/each}
								{#if p.schema.some((f) => f.type !== 'button') || p.qualities.length > 1}
									<div class="chips"><button type="button" class="btn" onclick={save}>Save</button></div>
								{/if}
								{#each p.actions as g (g.action)}
									<div class="chips"><button type="button" class="btn ghost" disabled={ui.offline} onclick={() => runAction(g.action)}>{g.label ?? g.action}</button></div>
								{/each}
								{#if p.form}
									{@const form = p.form}
									<form class="mx-form" onsubmit={(e) => { e.preventDefault(); void runAction(form.action, form.values, form.token); }}>
										{#if form.title}<b>{form.title}</b>{/if}
										{#each form.fields as f (f.key)}
											<label class="mx-field"><span>{labelOf(f)}</span>
												{#if f.options?.length}
													<select class="dev-input" bind:value={form.values[f.key]}>{#each optionsOf(f) as o (o.value)}<option value={o.value}>{o.label}</option>{/each}</select>
												{:else}
													<input class="dev-input" type={secretOf(f) ? 'password' : 'text'} bind:value={form.values[f.key]} autocomplete="off" spellcheck="false" />
												{/if}
											</label>
										{/each}
										<div class="chips"><button type="submit" class="btn">Continue</button></div>
									</form>
								{/if}
								{#if p.said}<p class="set-sub" role="status">{p.said}</p>{/if}
							</div>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}

		<div class="set-row set-col">
			<div class="mx-head">
				<span class="set-name">Available</span>
				<div class="set-ctl">
					<label class="btn ghost bk-pick" aria-disabled={busy !== null}>{busy === 'file' ? 'Installing…' : 'From file…'}<input class="vh" type="file" accept=".sflx,.morneye-ext,.spotiflac-ext" aria-label="Install an extension from a file" disabled={busy !== null} onchange={pickFile} /></label>
					<button type="button" class="btn ghost" disabled={busy !== null || ui.offline || m.refreshing} onclick={() => act('refresh', () => core.mornyeRefresh())}>{busy === 'refresh' || m.refreshing ? 'Refreshing…' : 'Refresh'}</button>
				</div>
			</div>
			{#if available.length}
				<ul class="mx-list">
					{#each available as e (e.repository + e.id)}
						{#if !e.installed}
							<li class="mx">
								<div class="mx-top">
									<span class="tr-t"><b>{e.display_name} <i class="mx-v">{e.version}</i></b><i title={e.description}>{e.description}{m.repositories.length > 1 ? ` · from ${repoName(e.repository)}` : ''}</i>{#if !e.compatible}<i class="src-problem">Needs a newer Stash</i>{/if}</span>
									<span class="src-acts"><button type="button" class="btn" disabled={busy !== null || ui.offline || !e.compatible} onclick={() => act(e.repository + e.id, () => core.mornyeInstall(e.repository, e.id))}>{busy === e.repository + e.id ? 'Installing…' : 'Install'}</button></span>
								</div>
							</li>
						{/if}
					{/each}
				</ul>
			{:else if m.refreshing}
				<p class="set-sub" role="status">Loading…</p>
			{:else}
				<p class="set-sub">All installed</p>
			{/if}
		</div>

		<div class="set-row">
			<div class="set-lab">
				<span class="set-name">Extra formats</span>
				{#if m.ffmpeg.fetching}
					<span class="set-sub" role="status">Downloading… {(m.ffmpeg.fetching.got / 1048576).toFixed(0)}{m.ffmpeg.fetching.total ? ` of ${(m.ffmpeg.fetching.total / 1048576).toFixed(0)}` : ''} MB</span>
				{:else if m.ffmpeg.found}
					<span class="set-sub">Ready for Amazon Music and SoundCloud</span>
				{:else if m.ffmpeg.can_fetch}
					<span class="set-sub">Needed for Amazon Music and SoundCloud</span>
				{:else}
					<span class="set-sub">Install FFmpeg for Amazon Music and SoundCloud</span>
				{/if}
				{#if m.ffmpeg.trouble}<span class="set-sub set-trouble" role="alert">{m.ffmpeg.trouble}</span>{/if}
			</div>
			{#if !m.ffmpeg.fetching && !m.ffmpeg.found && m.ffmpeg.can_fetch}
				<button type="button" class="btn ghost" disabled={busy !== null || ui.offline} onclick={() => act('ffmpeg', () => core.mornyeFetchFfmpeg())}>Download (160 MB)</button>
			{/if}
		</div>

		<details class="set-more">
			<summary><span>Advanced</span><Icon name="expand_more" class="ic set-chev" /></summary>
			<div class="set-row set-col">
				<span class="set-name">Repositories</span>
				<ul class="mx-repos">
					{#each m.repositories as r (r.id)}
						<li>
							<span class="tr-t"><b>{r.official ? 'Mornye' : (r.name ?? r.url)}</b><i>{r.count} {r.count === 1 ? 'extension' : 'extensions'}{r.official ? ' · built in' : ''}</i>{#if r.error}<i class="src-problem">{r.error}</i>{/if}</span>
							{#if !r.official}<button type="button" class="btn ghost" aria-label="Remove {r.name ?? r.url}" onclick={() => act('repo', () => core.mornyeRemoveRepository(r.id))}>Remove</button>{/if}
						</li>
					{/each}
				</ul>
				<form class="dev-rename" onsubmit={(e) => { e.preventDefault(); const url = repoLink.trim(); void act('repo', () => core.mornyeAddRepository(url)).then(() => (repoLink = '')); }}>
					<input class="dev-input" type="url" placeholder="Repository link" bind:value={repoLink} aria-label="Repository link" autocomplete="off" spellcheck="false" />
					<button type="submit" class="btn" disabled={busy !== null || ui.offline || repoLink.trim() === ''}>Add</button>
				</form>
			</div>
		</details>
	{:else if !wrong}
		<div class="set-row"><p class="set-sub">Loading…</p></div>
	{/if}
	{#if wrong}<div class="set-row"><p class="set-sub set-trouble" role="alert">{wrong}</p></div>{/if}
</div>

<style>
	/* As wide as the card, also inside a column (Available): a long description is cut short, the buttons stay in view. */
	.mx-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; align-self: stretch; min-width: 0; }
	/* installed: rows of the card; available: rows inside its column */
	.mx { display: flex; flex-direction: column; justify-content: center; min-height: 56px; padding: 10px 16px 10px 20px; }
	.set-col .mx { min-height: 0; padding: 8px 0; }
	.mx + .mx { border-top: 1px solid var(--wp-line); }
	.mx.off .tr-t { opacity: 0.6; }
	.mx-head { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 8px 16px; }
	.mx-top { display: flex; align-items: center; gap: 12px; min-width: 0; }
	.mx-top .src-acts { flex-shrink: 0; }
	.mx-top .tr-t { flex: 1; min-width: 0; display: flex; flex-direction: column; }
	.mx-v { font-style: normal; font-weight: 400; opacity: 0.6; margin-left: 4px; }
	.mx-panel { display: flex; flex-direction: column; gap: 10px; margin: 12px 0 4px; padding: 12px; border-radius: 10px; border: 1px solid var(--wp-line-2); background: var(--wp-field); }
	.mx-field { display: flex; flex-direction: column; gap: 4px; }
	.mx-field > span { font-size: 13px; }
	.mx-check { flex-direction: row; align-items: center; gap: 8px; }
	.mx-help { margin-top: -6px; }
	.mx-form { display: flex; flex-direction: column; gap: 10px; }
	.mx-repos { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
	.mx-repos li { display: flex; align-items: center; gap: 12px; }
	.mx-repos .tr-t { flex: 1; min-width: 0; display: flex; flex-direction: column; }
	.mx-paste { display: flex; flex-direction: column; gap: 8px; }
</style>
