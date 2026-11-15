<!-- Settings, in the web player's layout: Sound (the sound card for Stash alone), Add-ons, Your library (music folders, bringing a
     library in and saving one), Appearance, About. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { library, player, sources } from '../../lib/app';
	import * as core from '../../lib/core';
	import { folders, lastPart, many, trouble, type Notice } from '../../lib/desktop/folders.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import { outputSentence } from '../../lib/ui/signal';
	import { L, S, ui } from '../../lib/ui/state.svelte';
	import { applyAppearance, readAppearance, saveAppearance, type Appearance, type ThemeChoice } from '../../lib/ui/theme';
	import type { SourceStatus } from '../../lib/api';
	import './settings.css';

	// ---- Sound

	const now = $derived(outputSentence(S.v.signal));

	// ---- Add-ons: where music beyond the listener's own files comes from. Each is added by its link; the order is the listener's.

	let link = $state('');
	let adding = $state(false);
	let addonTrouble = $state<string | null>(null);

	async function addonChange(run: () => Promise<void>) {
		addonTrouble = null;
		try {
			await run();
		} catch (e) {
			addonTrouble = trouble(e);
		}
	}
	async function add(e: SubmitEvent) {
		e.preventDefault();
		const wanted = link.trim();
		if (wanted === '' || adding) return;
		adding = true;
		addonTrouble = null;
		try {
			await sources.add(wanted);
			link = '';
		} catch (x) {
			addonTrouble = x instanceof core.Refused ? `That link was not added: ${trouble(x)}` : trouble(x);
		} finally {
			adding = false;
		}
	}
	function move(index: number, by: number) {
		const ids = L.sources.map((a) => a.id);
		const [id] = ids.splice(index, 1);
		ids.splice(index + by, 0, id);
		void addonChange(() => sources.order(ids));
	}
	/** What an add-on does, from what its manifest lists. */
	const does = (a: SourceStatus) => [a.resources.includes('search') ? 'search' : '', a.resources.includes('stream') ? 'playing' : ''].filter((p) => p !== '').join(' and ');
	const detail = (a: SourceStatus) => [a.version ? `version ${a.version}` : '', a.host ?? '', does(a)].filter((p) => p !== '').join(' · ');

	// ---- Your library: music folders, and one file that carries a library between this computer, the phone and the web player

	let importing = $state(false);
	/** What the last import or save said. */
	let fileNotice = $state<Notice | null>(null);

	/** What an import did, in a sentence or two. */
	function imported(result: core.Imported): string {
		const s = result.summary;
		const parts = [
			s.songs_added ? many(s.songs_added, 'new song', 'new songs') : '',
			s.likes_added ? many(s.likes_added, 'like', 'likes') : '',
			s.playlists_added ? many(s.playlists_added, 'playlist', 'playlists') : '',
			s.playlist_songs_added ? `${many(s.playlist_songs_added, 'song', 'songs')} added to playlists that were here` : '',
			s.plays_added ? many(s.plays_added, 'play', 'plays') : ''
		].filter((p) => p !== '');
		const from = result.generator ? ` from ${result.generator}` : '';
		if (parts.length === 0) return `Nothing new${from}: everything in that file was here already.`;
		const here = s.songs_already ? ` ${many(s.songs_already, 'song was', 'songs were')} here already.` : '';
		return `Imported${from}: ${parts.join(', ')}.${here}`;
	}

	async function importFile(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file || importing) return;
		importing = true;
		fileNotice = null;
		try {
			const result = await core.importLibrary(file);
			await library.reload();
			fileNotice = { text: imported(result), trouble: false };
		} catch (x) {
			fileNotice = { text: trouble(x), trouble: true };
		} finally {
			importing = false;
		}
	}

	/** Saves the library as the file the Android app and the web player import. */
	async function saveFile() {
		try {
			const { blob, likes, lists, plays } = await core.exportLibrary();
			const name = `stash-library-${new Date().toISOString().slice(0, 10)}.json`;
			const a = document.createElement('a');
			a.href = URL.createObjectURL(blob);
			a.download = name;
			document.body.append(a);
			a.click();
			a.remove();
			setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
			fileNotice = { text: `Saved ${name}: ${many(likes, 'like', 'likes')}, ${many(lists, 'playlist', 'playlists')}, ${many(plays, 'play', 'plays')}. Import it in Stash on your phone or the web.`, trouble: false };
		} catch (x) {
			fileNotice = { text: trouble(x), trouble: true };
		}
	}

	onMount(() => {
		void folders.init();
		void sources.reload().catch(() => {});
	});

	// ---- Appearance

	let look = $state<Appearance>(readAppearance());
	function setLook(a: Appearance) {
		look = a;
		saveAppearance(a);
		applyAppearance(a);
		ui.theme = a.theme;
	}
	const THEMES: [ThemeChoice, string][] = [
		['dark', 'Dark'],
		['light', 'Light'],
		['system', 'Like this device']
	];
</script>

<div class="view view-in view-settings">
	<h1 class="vh2">Settings</h1>

	<section class="set-sec" id="sound" aria-labelledby="h-sound">
		<h2 class="set-h" id="h-sound">Sound</h2>
		<div class="set-card">
			{#if !S.v.available}
				<div class="set-na" role="status">
					<b>No sound</b>
					<span>{S.v.reason ?? 'The player is not available'}.</span>
				</div>
			{:else}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name" id="ex-l">Take the sound card for Stash alone</span>
						<span class="set-sub">Each track then goes to the sound card at its own sample rate, with the system’s mixer out of the way. While Stash plays, other programs make no sound. If the sound card cannot take a track’s format, the nearest one it can take is used, and the bar at the bottom says so.</span>
					</div>
					<button type="button" class="sw" role="switch" aria-checked={S.v.exclusive} aria-labelledby="ex-l" onclick={() => player.setExclusive(!S.v.exclusive)}><i></i></button>
				</div>
				<div class="set-row set-col">
					<p class="set-p">Off, the system mixes Stash with everything else at one fixed rate, and tracks at another rate are resampled.</p>
					<p class="set-p">The volume here changes the samples themselves. To send a track out untouched, leave it at 100.</p>
					{#if now}<p class="set-p" role="status"><b>{now}</b></p>{/if}
				</div>
			{/if}
		</div>
	</section>

	<section class="set-sec" id="addons" aria-labelledby="h-addons">
		<h2 class="set-h" id="h-addons">Add-ons</h2>
		<div class="set-card">
			<div class="set-row set-col">
				{#if L.sources.length}
					{#if L.sources.length > 1}<div class="set-lab"><span class="set-sub">The first add-on is asked first.</span></div>{/if}
					<ol class="src-list">
						{#each L.sources as a, i (a.id)}
							<li class="src" class:off={!a.enabled}>
								<span class="src-n" aria-hidden="true">{i + 1}</span>
								<span class="tr-t">
									<b>{a.name}</b>
									<i>{detail(a)}</i>
									{#if a.problem}<i class="src-problem">Cannot be used: {a.problem}.</i>{:else if a.description}<i>{a.description}</i>{/if}
								</span>
								<span class="src-acts">
									<button type="button" class="sw" role="switch" aria-checked={a.enabled} aria-label="{a.name}: on" title={a.enabled ? 'On' : 'Off'} onclick={() => addonChange(() => sources.setEnabled(a.id, !a.enabled))}><i></i></button>
									<button type="button" class="act-ib" disabled={i === 0} aria-label="Ask {a.name} earlier" title="Ask earlier" onclick={() => move(i, -1)}><Icon name="arrow_upward" /></button>
									<button type="button" class="act-ib" disabled={i === L.sources.length - 1} aria-label="Ask {a.name} later" title="Ask later" onclick={() => move(i, 1)}><Icon name="arrow_downward" /></button>
									<button type="button" class="btn ghost" aria-label="Remove {a.name}" onclick={() => addonChange(() => sources.remove(a.id))}>Remove</button>
								</span>
							</li>
						{/each}
					</ol>
				{:else}
					<p class="set-p">Stash plays your own files and comes with no add-ons. An add-on is a server that finds music and plays it; you add one by the link its maker gives you.</p>
				{/if}
				<form class="dev-rename" onsubmit={add}>
					<input class="dev-input" type="url" placeholder="https://…/manifest.json" bind:value={link} aria-label="An add-on’s link" autocomplete="off" spellcheck="false" />
					<button type="submit" class="btn" disabled={adding || link.trim() === ''}>{adding ? 'Reading…' : 'Add'}</button>
				</form>
				{#if addonTrouble}<p class="set-sub set-trouble" role="alert">{addonTrouble}</p>{/if}
			</div>
		</div>
	</section>

	<section class="set-sec" id="library" aria-labelledby="h-library">
		<h2 class="set-h" id="h-library">Your library</h2>
		<div class="set-card">
			<div class="set-row set-col">
				<div class="set-lab">
					<span class="set-name">Music folders</span>
					{#if folders.list.length}
						{#each folders.list as f (f)}<span class="set-sub set-path" title={f}>{f}</span>{/each}
					{:else}
						<span class="set-sub">Stash reads the music in the folders you add. Nothing is moved or changed there.</span>
					{/if}
				</div>
				{#if folders.scan}
					<p class="set-sub" role="status">Scanning {folders.scan.folder ? lastPart(folders.scan.folder) : ''}… {many(folders.scan.report?.files ?? 0, 'file', 'files')}</p>
				{:else}
					<div class="chips">
						<button type="button" class="btn" onclick={() => folders.add()}>Add a music folder…</button>
						{#if folders.list.length}<button type="button" class="btn ghost" onclick={() => folders.again()}>Scan again</button>{/if}
					</div>
				{/if}
				{#if folders.notice}<p class="set-sub" class:set-trouble={folders.notice.trouble} role="status">{folders.notice.text}</p>{/if}
			</div>
			<div class="set-row set-col">
				<div class="set-lab">
					<span class="set-name">From and to another Stash</span>
					<span class="set-sub">One file carries likes, playlists and plays between this computer, Stash on your phone and the web player. The phone’s Backup (.zip) can be imported too.</span>
				</div>
				<div class="chips">
					<label class="btn ghost bk-pick" aria-disabled={importing}>{importing ? 'Importing…' : 'Import a library…'}<input class="vh" type="file" accept=".json,.zip,.db,application/json,application/zip" disabled={importing} onchange={importFile} /></label>
					<button type="button" class="btn ghost" onclick={saveFile}>Save a library file</button>
				</div>
				{#if fileNotice}<p class="set-sub" class:set-trouble={fileNotice.trouble} role="status">{fileNotice.text}</p>{/if}
			</div>
		</div>
	</section>

	<section class="set-sec" id="appearance" aria-labelledby="h-appearance">
		<h2 class="set-h" id="h-appearance">Appearance</h2>
		<div class="set-card">
			<div class="set-row set-col">
				<div class="set-lab"><span class="set-name" id="th-l">Theme</span></div>
				<div class="chips" role="group" aria-labelledby="th-l">
					{#each THEMES as [id, label] (id)}
						<button type="button" class="chip" class:on={look.theme === id} aria-pressed={look.theme === id} onclick={() => setLook({ ...look, theme: id })}>{label}</button>
					{/each}
				</div>
			</div>
			<div class="set-row">
				<div class="set-lab"><span class="set-name" id="bk-l">Pure black</span><span class="set-sub">A black background whenever the dark theme shows. Easy on OLED screens at night.</span></div>
				<button type="button" class="sw" role="switch" aria-checked={look.black} aria-labelledby="bk-l" onclick={() => setLook({ ...look, black: !look.black })}><i></i></button>
			</div>
		</div>
	</section>

	<section class="set-sec" id="about" aria-labelledby="h-about">
		<h2 class="set-h" id="h-about">About</h2>
		<div class="set-card">
			<div class="set-row">
				<div class="set-lab"><span class="set-name">Stash</span><span class="set-sub">Version {__STASH_VERSION__}</span></div>
			</div>
		</div>
	</section>
</div>

<style>
	/* an add-on's row: the web player's numbered list, with its controls where the status pill was */
	.src-acts { display: flex; align-items: center; gap: 4px; }
	.src-acts .sw { margin-right: 8px; }
	.src-acts .btn { margin-left: 8px; }
	.src.off .tr-t { opacity: .55; }
	.src .tr-t i { white-space: normal; }
	.src-problem, .set-trouble { color: var(--red) !important; }
	/* a folder's path: one line (the whole path is its title) */
	.set-path { overflow: clip; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
	@container main (max-width: 560px) { .src { grid-template-columns: 24px minmax(0, 1fr); } .src-acts { grid-column: 2; } }
</style>
