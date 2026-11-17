<!-- Settings, in the web player's layout: Sound (the output, its equalizer, the sound card for Stash alone), Add-ons, Your library
     (music folders, bringing a library in and saving one), Appearance, About. -->
<script lang="ts">
	import { importPlaylistFile } from '../../lib/desktop/playlist-files';
	import { onMount } from 'svelte';
	import { goOffline, library, player, sources } from '../../lib/app';
	import * as core from '../../lib/core';
	import { folders, lastPart, many, trouble, type Notice } from '../../lib/desktop/folders.svelte';
	import Scrobbling from '../../lib/desktop/Scrobbling.svelte';
	import Shortcuts from '../../lib/desktop/Shortcuts.svelte';
	import Blocked from '../../lib/desktop/Blocked.svelte';
	import { R } from '../../lib/desktop/radio.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import { correctionLine, eqStatus, eqSub, eqTrouble, outName } from '../../lib/ui/equalizer';
	import { outputSentence } from '../../lib/ui/signal';
	import { L, S, ui } from '../../lib/ui/state.svelte';
	import { applyAppearance, readAppearance, saveAppearance, type Appearance, type ThemeChoice } from '../../lib/ui/theme';
	import type { SourceStatus } from '../../lib/api';
	import './settings.css';

	// ---- Sound

	const now = $derived(outputSentence(S.v.signal));
	const RG = [
		['off', 'Off'],
		['track', 'By song'],
		['album', 'By album']
	] as const;

	/** Where the sound goes: the system's outputs, read again whenever the window comes back to the front (one may have been plugged in). */
	let outs = $state<core.Devices | null>(null);
	let outTrouble = $state<string | null>(null);
	const readOuts = () =>
		core.devices().then(
			(d) => (outs = d),
			() => {}
		);
	async function chooseOut(name: string) {
		outTrouble = null;
		try {
			outs = await core.setDevice(name);
			// The equalizer shown is the new output's at once, not at the next look.
			player.refresh();
		} catch (e) {
			outTrouble = trouble(e);
			void readOuts();
		}
	}

	// ---- The equalizer of the output in use: a correction from a file, and two tones. Each output keeps its own.

	const eq = $derived(S.v.eq);
	const TONES = [
		['bass', 'Bass'],
		['treble', 'Treble']
	] as const;
	const TILTS = [
		['less', 'Less'],
		['as-is', 'As is'],
		['more', 'More']
	] as const;
	/** A file that was not brought in, in the reader's words. */
	let eqRefused = $state<string | null>(null);
	let eqPicking = $state(false);
	/** This page was left (a file's refusal is then said in a toast). */
	let left = false;
	/** One request for each tap. */
	function setEq(patch: core.EqPatch) {
		eqRefused = null;
		player.setEq(patch);
	}
	/** A tone. The core turns the switch on for Less or More; As is leaves the switch as it is. */
	function tone(which: 'bass' | 'treble', t: core.Tilt) {
		setEq(which === 'bass' ? { bass: t } : { treble: t });
	}
	/** "Choose a file…": the system's own dialog, then the file is read and its correction put on. */
	async function chooseEqFile() {
		if (eqPicking) return;
		eqPicking = true;
		eqRefused = null;
		try {
			await player.eqFromFile();
		} catch (e) {
			// Said on this page; when the listener left it while the dialog was open, said where they are.
			if (left) ui.toast(trouble(e));
			else eqRefused = trouble(e);
		} finally {
			eqPicking = false;
		}
	}
	const eqSays = $derived(eqStatus(eq));
	const eqAlert = $derived(eqTrouble(eq.trouble));

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
	/** A new version of an add-on, shown until it is taken or put off. */
	let offer = $state<{ id: string; version: string; changes: core.AddonChange[]; token: string | null } | null>(null);
	/** The add-on being asked for a new version, or being updated. */
	let checking = $state<string | null>(null);
	/** What the last check said, when there was nothing to take. */
	let addonNote = $state<string | null>(null);
	async function check(a: SourceStatus) {
		if (checking) return;
		checking = a.id;
		addonTrouble = null;
		addonNote = null;
		offer = null;
		try {
			const found = await sources.check(a.id);
			if (found.changes.length) offer = { id: a.id, ...found };
			else addonNote = `${a.name} is up to date.`;
		} catch (x) {
			addonTrouble = `${a.name} could not be checked: ${trouble(x)}`;
		} finally {
			checking = null;
		}
	}
	async function takeOffer() {
		if (!offer || !offer.token || checking) return;
		const { id, token } = offer;
		checking = id;
		addonTrouble = null;
		try {
			await sources.update(id, token);
			offer = null;
		} catch (x) {
			offer = null;
			addonTrouble = trouble(x);
		} finally {
			checking = null;
		}
	}
	function move(index: number, by: number) {
		// The listener's own add-ons: what is built in is asked after them, whatever their order.
		const ids = L.sources.filter((a) => !a.builtIn).map((a) => a.id);
		const [id] = ids.splice(index, 1);
		ids.splice(index + by, 0, id);
		void addonChange(() => sources.order(ids));
	}
	/** What an add-on does, from what its manifest lists. */
	const does = (a: SourceStatus) => [a.resources.includes('search') ? 'search' : '', a.resources.includes('stream') ? 'playing' : ''].filter((p) => p !== '').join(' and ');
	// What each add-on was asked for today (its allowance is the listener's), when it last gave a song, its last trouble.
	let used = $state<Record<string, core.AddonUse>>({});
	async function readUse() {
		const midnight = new Date();
		midnight.setHours(0, 0, 0, 0);
		try {
			used = await core.addonUse(midnight.getTime());
		} catch {
			/* the core is not answering: the lines stay as they were */
		}
	}
	const whole = (n: number) => n.toLocaleString();
	const ago = (ms: number) => {
		const minutes = Math.floor((Date.now() - ms) / 60_000);
		return minutes < 1 ? 'just now' : minutes < 60 ? `${minutes} min ago` : `${Math.floor(minutes / 60)} h ago`;
	};
	function useLine(id: string): string {
		const u = used[id];
		if (!u) return '';
		const parts = [u.streams ? `${many(u.streams, 'song', 'songs')} played` : '', u.downloads ? (u.streams ? `${whole(u.downloads)} saved` : `${many(u.downloads, 'song', 'songs')} saved`) : ''].filter(Boolean);
		const asked = parts.length ? `Today: ${parts.join(', ')}` : 'Nothing asked of it today';
		return u.found_at_ms ? `${asked} · last gave a song ${ago(u.found_at_ms)}` : asked;
	}
	/** Its last trouble, when that came after the last song it gave. */
	const troubleOf = (id: string) => {
		const u = used[id];
		return u?.trouble && u.trouble.at_ms > (u.found_at_ms ?? 0) ? u.trouble : null;
	};
	/** Where it is, when that is nearer than the internet: it may send Stash only as near as that. */
	const where = (a: SourceStatus) => (a.reach === 'home' ? 'on your network' : a.reach === 'here' ? 'on this computer' : '');
	const detail = (a: SourceStatus) => (a.builtIn ? ['built in', does(a)] : [a.version ? `version ${a.version}` : '', a.host ?? '', where(a), does(a)]).filter((p) => p !== '').join(' · ');
	/** How many of the sources are the listener's own add-ons (they come first in the list). */
	const own = $derived(L.sources.filter((a) => !a.builtIn).length);

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
			s.plays_added ? many(s.plays_added, 'play', 'plays') : '',
			s.blocks_added ? `${many(s.blocks_added, 'song', 'songs')} you chose not to play` : ''
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

	// ---- Lyrics: LRCLIB when no add-on has a song's lyrics (on unless switched off).

	// ---- Similar music from Last.fm (radio, and autoplay that knows what is like what), when this build has it.
	let similarBusy = $state(false);
	async function setSimilar(on: boolean) {
		similarBusy = true;
		try {
			await R.set(on);
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			similarBusy = false;
		}
	}

	let autoplay = $state<boolean | null>(null);
	async function setAutoplay(on: boolean) {
		autoplay = on;
		try {
			autoplay = (await core.setAutoplay(on)).on;
		} catch {
			autoplay = !on;
		}
	}

	let tray = $state<boolean | null>(null);
	async function setTray(on: boolean) {
		tray = on;
		try {
			tray = (await core.setWindowSettings(on)).tray;
		} catch {
			tray = !on;
		}
	}

	// ---- Songs kept as they were played: how much room they may take.
	const ROOMS: [number, string][] = [
		[0, 'Off'],
		[1024, '1 GB'],
		[2048, '2 GB'],
		[5120, '5 GB'],
		[10240, '10 GB']
	];
	let room = $state<core.PlayedRoom | null>(null);
	async function setRoom(cap_mb: number) {
		try {
			room = await core.setPlayedRoom(cap_mb);
		} catch (e) {
			ui.toast(core.said(e));
		}
	}
	async function clearRoom() {
		try {
			room = await core.clearPlayed();
		} catch (e) {
			ui.toast(core.said(e));
		}
	}
	const roomSize = (bytes: number) => (bytes >= 1_073_741_824 ? `${(bytes / 1_073_741_824).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1_048_576))} MB`);

	let lrclib = $state<boolean | null>(null);
	async function setLrclib(on: boolean) {
		lrclib = on;
		try {
			lrclib = (await core.setLyricsSettings(on)).lrclib;
		} catch {
			lrclib = !on;
		}
	}

	onMount(() => {
		void folders.init();
		void sources.reload().catch(() => {});
		void core.lyricsSettings().then(
			(s) => (lrclib = s.lrclib),
			() => {}
		);
		void core.windowSettings().then(
			(s) => (tray = s.tray),
			() => {}
		);
		void core.autoplaySettings().then(
			(s) => (autoplay = s.on),
			() => {}
		);
		void R.refresh();
		void readUse();
		const useTimer = setInterval(() => void readUse(), 15_000);
		void core.playedRoom().then(
			(r) => (room = r),
			() => {}
		);
		void readOuts();
		const back = () => {
			void readOuts();
			void readUse();
		};
		window.addEventListener('focus', back);
		return () => {
			left = true;
			window.removeEventListener('focus', back);
			clearInterval(useTimer);
		};
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
				{#if outs && outs.devices.length > 1}
					<div class="set-row set-col">
						<div class="set-lab">
							<span class="set-name" id="out-l">Play through</span>
							<span class="set-sub">The output changes at once, also in the middle of a song, and stays chosen.</span>
						</div>
						<div class="chips" role="group" aria-labelledby="out-l">
							{#each outs.devices as d (d.name)}
								<button type="button" class="chip" class:on={outs.device === d.name} aria-pressed={outs.device === d.name} onclick={() => chooseOut(d.name)}>{outName(d)}</button>
							{/each}
						</div>
						{#if outs.missing}<p class="set-sub" role="status">{outs.missing.description || 'The output you chose'} is not connected. Stash plays through the system’s choice until it is back.</p>{/if}
						{#if outTrouble}<p class="set-sub set-trouble" role="alert">{outTrouble}</p>{/if}
					</div>
				{/if}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name" id="eq-l">Equalizer</span>
						<span class="set-sub">{eqSub(eq, !(outs && outs.devices.length > 1))}</span>
					</div>
					<button type="button" class="sw" role="switch" aria-checked={eq.on} aria-labelledby="eq-l" onclick={() => setEq({ on: !eq.on })}><i></i></button>
				</div>
				<div class="set-row set-col">
					<div class="set-lab">
						<span class="set-name" id="corr-l">Correction</span>
						{#if eq.correction}
							<span class="set-sub eq-name">{correctionLine(eq.correction)}</span>
							<span class="set-sub">{eq.correction.summary}</span>
						{:else}
							<span class="set-sub">A file made for your headphones or speakers, in Equalizer APO’s format. For headphones: find yours on autoeq.app and download it for Equalizer APO.</span>
						{/if}
					</div>
					<div class="chips" role="group" aria-labelledby="corr-l">
						{#if eq.correction}
							<button type="button" class="btn ghost" disabled={eqPicking} onclick={chooseEqFile}>Choose another…</button>
							<button type="button" class="btn ghost" onclick={() => setEq({ correction: null })}>Remove</button>
						{:else}
							<button type="button" class="btn" disabled={eqPicking} onclick={chooseEqFile}>Choose a file…</button>
						{/if}
					</div>
					{#if eqRefused}<p class="set-sub set-trouble" role="alert">{eqRefused}</p>{/if}
				</div>
				{#each TONES as [which, name] (which)}
					<div class="set-row set-col">
						<div class="set-lab"><span class="set-name" id="{which}-l">{name}</span></div>
						<div class="chips" class:eq-off={!eq.on} role="group" aria-labelledby="{which}-l">
							{#each TILTS as [t, label] (t)}
								<button type="button" class="chip" class:on={eq[which] === t} aria-pressed={eq[which] === t} onclick={() => tone(which, t)}>{label}</button>
							{/each}
						</div>
					</div>
				{/each}
				{#if eqSays || eqAlert}
					<div class="set-row set-col">
						{#if eqSays}<p class="set-sub" role="status">{eqSays}</p>{/if}
						{#if eqAlert}<p class="set-sub set-trouble" role="alert">{eqAlert}</p>{/if}
					</div>
				{/if}
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name" id="ex-l">Take the sound card for Stash alone</span>
						<span class="set-sub">Each track then goes to the sound card at its own sample rate, with the system’s mixer out of the way. While Stash plays, other programs make no sound. If the sound card cannot take a track’s format, the nearest one it can take is used, and the bar at the bottom says so. Stash’s own equalizer still works here; equalizers outside Stash, such as Equalizer APO, do not reach the sound card this way.</span>
					</div>
					<button type="button" class="sw" role="switch" aria-checked={S.v.exclusive} aria-labelledby="ex-l" onclick={() => player.setExclusive(!S.v.exclusive)}><i></i></button>
				</div>
				<div class="set-row set-col">
					<p class="set-p">Off, the system mixes Stash with everything else at one fixed rate, and tracks at another rate are resampled.</p>
					<p class="set-p">The volume and the equalizer here change the samples themselves. To send a track out untouched, leave the volume at 100 and the equalizer off.</p>
					{#if now}<p class="set-p" role="status"><b>{now}</b></p>{/if}
				</div>
				<div class="set-row">
					<div class="set-lab">
						<span class="set-name" id="ap-l">Autoplay</span>
						{#if R.ready}
							<span class="set-sub">When the queue ends, the music goes on with songs like the last one: Last.fm suggests similar artists, and their songs come from your library first, then your add-ons. Offline, or when Last.fm has nothing, it picks from your library: the same artists, your likes and what you play most.</span>
						{:else}
							<span class="set-sub">When the queue ends, the music goes on with songs like the last ones from your library: by the same artists, then your likes and what you play most.</span>
						{/if}
					</div>
					<button type="button" class="sw" role="switch" aria-checked={autoplay === true} aria-labelledby="ap-l" disabled={autoplay === null} onclick={() => autoplay !== null && setAutoplay(!autoplay)}><i></i></button>
				</div>
				{#if R.available}
					<div class="set-row" id="similar">
						<div class="set-lab">
							<span class="set-name" id="lfm-l">Similar music from Last.fm</span>
							<span class="set-sub">For radio and autoplay, Stash asks Last.fm which artists and songs are like the one you start from. Only that song’s title and artist, or the artist’s name, are sent, through Stash’s own server (which sees this computer’s internet address, as any server does) and on to Last.fm; nothing about your library, what you play or your accounts. An answer is used for two weeks, so the same question is not asked again in that time, and is kept on this computer for up to two months (an older one is used while Last.fm cannot be reached). Off, nothing is sent, there is no radio, autoplay keeps to your library, and what was kept is deleted.</span>
						</div>
						<button type="button" class="sw" role="switch" aria-checked={R.on} aria-labelledby="lfm-l" disabled={similarBusy} onclick={() => void setSimilar(!R.on)}><i></i></button>
					</div>
				{/if}
				<div class="set-row set-col">
					<div class="set-lab">
						<span class="set-name" id="rg-l">Even out loudness</span>
						<span class="set-sub">Uses the loudness measured in a song’s tags (ReplayGain). By song makes every song equally loud; by album keeps the quiet and loud songs of an album as they were made. A song it evens out has its samples changed, so it is not bit-perfect; songs without the tags play as they are.</span>
					</div>
					<div class="chips" role="group" aria-labelledby="rg-l">
						{#each RG as [mode, label] (mode)}
							<button type="button" class="chip" class:on={S.v.replaygain === mode} aria-pressed={S.v.replaygain === mode} onclick={() => player.setReplaygain(mode)}>{label}</button>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	</section>

	<section class="set-sec" id="addons" aria-labelledby="h-addons">
		<h2 class="set-h" id="h-addons">Add-ons</h2>
		<div class="set-card">
			<div class="set-row set-col">
				{#if L.sources.length}
					{#if L.sources.length > 1}<div class="set-lab"><span class="set-sub">{own > 1 ? 'The first add-on is asked first.' : 'Your add-on is asked first.'}{own < L.sources.length ? ' What your add-ons don’t have is found on YouTube Music.' : ''}</span></div>{/if}
					<ol class="src-list">
						{#each L.sources as a, i (a.id)}
							<li class="src" class:off={!a.enabled}>
								<span class="src-n" aria-hidden="true">{i + 1}</span>
								<span class="tr-t">
									<b>{a.name}</b>
									<i>{detail(a)}</i>
									{#if useLine(a.id)}<i class="src-use">{useLine(a.id)}</i>{/if}
									{#if troubleOf(a.id)}<i class="src-problem">Trouble {ago(troubleOf(a.id)!.at_ms)}: {troubleOf(a.id)!.message}</i>{/if}
									{#if a.problem}<i class="src-problem">Cannot be used: {a.problem}.</i>{:else if a.description}<i>{a.description}</i>{/if}
								</span>
								<span class="src-acts">
									<button type="button" class="sw" role="switch" aria-checked={a.enabled} aria-label="{a.name}: on" title={a.enabled ? 'On' : 'Off'} onclick={() => addonChange(() => sources.setEnabled(a.id, !a.enabled))}><i></i></button>
									{#if !a.builtIn}
										<button type="button" class="act-ib" disabled={i === 0} aria-label="Ask {a.name} earlier" title="Ask earlier" onclick={() => move(i, -1)}><Icon name="arrow_upward" /></button>
										<button type="button" class="act-ib" disabled={i >= own - 1} aria-label="Ask {a.name} later" title="Ask later" onclick={() => move(i, 1)}><Icon name="arrow_downward" /></button>
										<button type="button" class="btn ghost" aria-label="Check {a.name} for a new version" title="Check for a new version" disabled={checking !== null || ui.offline} onclick={() => check(a)}>{checking === a.id && offer?.id !== a.id ? 'Checking…' : 'Check'}</button>
										<button type="button" class="btn ghost" aria-label="Remove {a.name}" onclick={() => { if (offer?.id === a.id) offer = null; void addonChange(() => sources.remove(a.id)); }}>Remove</button>
									{/if}
								</span>
								{#if offer?.id === a.id}
									<div class="src-offer" role="group" aria-labelledby="offer-h">
										<b id="offer-h">{offer.version !== a.version ? `Version ${offer.version} of ${a.name}` : `${a.name} has changed`}</b>
										<ul>
											{#each offer.changes as c (c.words)}<li class:gives={c.more}>{c.words}</li>{/each}
										</ul>
										{#if offer.changes.some((c) => c.more)}<p class="src-problem">Updating gives it more than when you added it.</p>{/if}
										<div class="src-offer-acts">
											<button type="button" class="btn" disabled={checking !== null} onclick={takeOffer}>{checking === a.id ? 'Updating…' : 'Update'}</button>
											<button type="button" class="btn ghost" onclick={() => (offer = null)}>Not now</button>
										</div>
									</div>
								{/if}
							</li>
						{/each}
					</ol>
				{:else}
					<p class="set-p">Stash plays your own files and comes with no add-ons. An add-on is a server that finds music and plays it; you add one by the link its maker gives you.</p>
				{/if}
				{#if L.sources.length && own === 0}
					<p class="set-p">An add-on is a server that finds music and plays it, in lossless when it has it; you add one by the link its maker gives you. Your add-ons are asked before YouTube Music.</p>
				{/if}
				<form class="dev-rename" onsubmit={add}>
					<input class="dev-input" type="url" placeholder="An add-on’s link (https://…)" bind:value={link} aria-label="An add-on’s link" autocomplete="off" spellcheck="false" />
					<button type="submit" class="btn" disabled={adding || link.trim() === ''}>{adding ? 'Reading…' : 'Add'}</button>
				</form>
				{#if addonTrouble}<p class="set-sub set-trouble" role="alert">{addonTrouble}</p>{/if}
				{#if addonNote}<p class="set-sub" role="status">{addonNote}</p>{/if}
			</div>
		</div>
	</section>

	<section class="set-sec" id="lyrics" aria-labelledby="h-lyrics">
		<h2 class="set-h" id="h-lyrics">Lyrics</h2>
		<div class="set-card">
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name" id="lrc-l">Look up lyrics on LRCLIB</span>
					<span class="set-sub">A song’s own lyrics come first: a .lrc file beside it, or the lyrics in its tags. Then your add-ons. When none of them has the words, LRCLIB, an open database of timed lyrics, is asked; it is sent the song’s title, artist, album and length. What it finds is kept, so each song is asked for once.</span>
				</div>
				<button type="button" class="sw" role="switch" aria-checked={lrclib === true} aria-labelledby="lrc-l" disabled={lrclib === null} onclick={() => lrclib !== null && setLrclib(!lrclib)}><i></i></button>
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
			<div class="set-row set-col">
				<div class="set-lab">
					<span class="set-name">From another player</span>
					<span class="set-sub">A playlist file (.m3u or .m3u8, as foobar2000, MusicBee, VLC and most players save them) becomes a playlist here: files you have play from where they are, and other songs are found by name. A playlist’s page saves it as one too.</span>
				</div>
				<div class="chips">
					<button type="button" class="btn ghost" onclick={() => void importPlaylistFile()}>Import a playlist file…</button>
				</div>
			</div>
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name" id="off-l">Offline</span>
					<span class="set-sub">Only what is on this computer plays: your files, songs saved, and songs kept as they were played. {R.available ? 'No add-on or Last.fm is asked' : 'No add-on is asked'} and nothing streams; songs that are not here are dimmed, and plays wait to be scrobbled. It stays on until you turn it off.</span>
				</div>
				<button type="button" class="sw" role="switch" aria-checked={ui.offline} aria-labelledby="off-l" onclick={() => void goOffline(!ui.offline)}><i></i></button>
			</div>
			{#if room}
				<div class="set-row set-col">
					<div class="set-lab">
						<span class="set-name" id="kp-l">Keep songs you play</span>
						<span class="set-sub">A song played from an add-on is kept on this computer as it plays, so next time it plays with no connection and without asking the add-on. When the room is full, the songs played longest ago make way.</span>
					</div>
					<div class="chips" role="group" aria-labelledby="kp-l">
						{#each ROOMS as [mb, label] (mb)}
							<button type="button" class="chip" class:on={room.cap_mb === mb} aria-pressed={room.cap_mb === mb} onclick={() => setRoom(mb)}>{label}</button>
						{/each}
					</div>
					{#if room.songs}
						<p class="set-sub" role="status">
							{many(room.songs, 'song', 'songs')} kept, {roomSize(room.bytes)}.
							<button type="button" class="btn ghost kp-clear" onclick={clearRoom}>Clear</button>
						</p>
					{/if}
				</div>
			{/if}
		</div>
	</section>

	<section class="set-sec" id="scrobbling" aria-labelledby="h-scrobbling">
		<h2 class="set-h" id="h-scrobbling">Scrobbling</h2>
		<Scrobbling />
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
			{#if !core.measuring}
				<div class="set-row">
					<div class="set-lab"><span class="set-name" id="tray-l">Keep playing when the window is closed</span><span class="set-sub">Stash stays in the tray by the clock. Click it to bring the window back; its menu plays, pauses and skips, and quits Stash.</span></div>
					<button type="button" class="sw" role="switch" aria-checked={tray === true} aria-labelledby="tray-l" disabled={tray === null} onclick={() => tray !== null && setTray(!tray)}><i></i></button>
				</div>
			{/if}
		</div>
	</section>

	<Blocked />
	<Shortcuts />

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
	.src-acts { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 4px; }
	.src-acts .sw { margin-right: 8px; }
	.src-acts .btn { margin-left: 8px; }
	.src.off .tr-t { opacity: .55; }
	.src .tr-t i { white-space: normal; }
	.src-problem, .set-trouble { color: var(--red) !important; }
	/* a new version of an add-on: what changed, what gives it more first in red, then take it or put it off */
	.src-offer { grid-column: 2 / -1; display: grid; gap: 6px; margin: 0 0 6px; padding: 10px 12px; border: 1px solid var(--wp-line-2); border-radius: 8px; background: var(--wp-field); font-size: 13px; }
	.src-offer b { font-weight: 600; color: var(--wp-text); }
	.src-offer ul { margin: 0; padding-left: 18px; display: grid; gap: 2px; list-style: disc; color: var(--wp-text-2); }
	.src-offer li.gives { color: var(--red); }
	.src-offer p { margin: 0; }
	.src-offer-acts { display: flex; gap: 8px; margin-top: 4px; }
	/* the equalizer: the correction's name reads first; while the switch is off, the tones it keeps are dimmed */
	.eq-name { color: var(--wp-text); }
	.eq-off .chip { opacity: .55; }
	/* a folder's path: one line (the whole path is its title) */
	.set-path { overflow: clip; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
	@container main (max-width: 560px) { .src { grid-template-columns: 24px minmax(0, 1fr); } .src-acts { grid-column: 2; } }
</style>
