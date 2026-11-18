<!-- Settings: a short list of groups beside the page (above it in a narrow window), each group a few cards of rows. Groups:
     Sound, Playback (with Not played), Sources (Add-ons, YouTube Music, Mornye extensions), Library, Scrobbling, Shortcuts,
     Appearance, About. Each row is a name, a hint only where it is needed, and its control. -->
<script lang="ts">
	import { importPlaylistFile } from '../../lib/desktop/playlist-files';
	import { onMount } from 'svelte';
	import { afterNavigate } from '$app/navigation';
	import { goOffline, library, player, sources } from '../../lib/app';
	import * as core from '../../lib/core';
	import { folders, lastPart, many, trouble, type Notice } from '../../lib/desktop/folders.svelte';
	import Scrobbling from '../../lib/desktop/Scrobbling.svelte';
	import TubeAccount from '../../lib/desktop/TubeAccount.svelte';
	import TubeFallback from '../../lib/desktop/TubeFallback.svelte';
	import MornyeExtensions from '../../lib/desktop/MornyeExtensions.svelte';
	import Shortcuts from '../../lib/desktop/Shortcuts.svelte';
	import Blocked from '../../lib/desktop/Blocked.svelte';
	import { R } from '../../lib/desktop/radio.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import { eqTrouble, outName } from '../../lib/ui/equalizer';
	import { outputSentence } from '../../lib/ui/signal';
	import { L, S, ui } from '../../lib/ui/state.svelte';
	import { applyAppearance, readAppearance, saveAppearance, type Appearance, type ThemeChoice } from '../../lib/ui/theme';
	import type { SourceStatus } from '../../lib/api';
	import './settings.css';

	// ---- The groups, and the one in view

	/** Shortcuts shows only where the app can hold keys system-wide (the component says so once it knows). */
	let shortcutsShown = $state(false);
	const groups = $derived<[string, string][]>([
		['sound', 'Sound'],
		['playback', 'Playback'],
		['sources', 'Sources'],
		['library', 'Library'],
		['scrobbling', 'Scrobbling'],
		...(shortcutsShown ? ([['shortcuts', 'Shortcuts']] as [string, string][]) : []),
		['appearance', 'Appearance'],
		['about', 'About']
	]);
	let current = $state('sound');
	/** While a group's scroll is under way, the list keeps the group clicked. */
	let hold = 0;

	function go(id: string) {
		const el = document.getElementById(id);
		if (!el) return;
		current = id;
		hold = performance.now() + 1000;
		const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
		el.scrollIntoView({ behavior: still ? 'auto' : 'smooth', block: 'start' });
		el.querySelector<HTMLElement>('.set-g')?.focus({ preventScroll: true });
	}

	/** The group whose heading is nearest above the top of the page's scroller (the last one at the very bottom). */
	function spy() {
		const main = document.getElementById('main');
		if (!main || performance.now() < hold) return;
		const line = main.getBoundingClientRect().top + 120;
		let at = groups[0][0];
		for (const [id] of groups) {
			const el = document.getElementById(id);
			if (el && el.getBoundingClientRect().top <= line) at = id;
		}
		if (main.scrollTop + main.clientHeight >= main.scrollHeight - 2) at = groups[groups.length - 1][0];
		current = at;
	}

	// A link to a part of Settings (#lyrics, #youtube…): the layout puts the page back at the top after each navigation, so the
	// part is scrolled to here, once it is drawn (some parts appear when what they show has been read).
	afterNavigate(() => {
		const id = decodeURIComponent(location.hash.slice(1));
		if (!id) return;
		// Sections above it may still be loading (add-ons, extensions) and push it down: it is kept in view until the
		// page settles, for up to 2.5 s, unless the listener scrolls first.
		let tries = 0;
		let moved = false;
		const stop = () => (moved = true);
		addEventListener('wheel', stop, { once: true, passive: true });
		addEventListener('keydown', stop, { once: true });
		addEventListener('pointerdown', stop, { once: true });
		const seek = () => {
			if (moved) return;
			const el = document.getElementById(id);
			if (el) {
				const main = document.getElementById('main');
				const off = main ? el.getBoundingClientRect().top - main.getBoundingClientRect().top : 0;
				if (Math.abs(off) > 4) el.scrollIntoView({ block: 'start' });
				hold = performance.now() + 300;
				current = el.closest('.set-sec')?.id ?? current;
			}
			if (++tries < 25) setTimeout(seek, 100);
			else for (const [k, f] of [['wheel', stop], ['keydown', stop], ['pointerdown', stop]] as const) removeEventListener(k, f);
		};
		requestAnimationFrame(seek);
	});

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
	/** An output's name in the picker. */
	const deviceName = (d: core.Device) => (d.name === 'auto' ? 'System default' : outName(d));

	// ---- The equalizer of the output in use: a correction from a file, and two tones. Each output keeps its own.

	const eq = $derived(S.v.eq);
	const TONES = [
		['bass', 'Bass'],
		['treble', 'Treble']
	] as const;
	const TILTS = [
		['less', 'Less'],
		['as-is', 'Normal'],
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
	/** "Choose file…": the system's own dialog, then the file is read and its correction put on. */
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
	/** Which output the equalizer is for, when there is more than one. */
	const eqFor = $derived(outs && outs.devices.length > 1 ? (eq.output.name === 'auto' ? 'For the system default' : `For ${outName(eq.output)}`) : null);
	/** What the equalizer does to the sound now, when that is worth a line. */
	const eqSays = $derived(eq.reopen_at_pause ? 'Bit-perfect again after the next pause' : eq.on && eq.softer_db >= 0.05 ? `Plays ${eq.softer_db.toFixed(1)} dB quieter to avoid distortion` : null);
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
			addonTrouble = x instanceof core.Refused ? `Couldn’t add that link: ${trouble(x)}` : trouble(x);
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
			else addonNote = `${a.name} is up to date`;
		} catch (x) {
			addonTrouble = `Couldn’t check ${a.name}: ${trouble(x)}`;
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
	/** Said only when an add-on does one of the two. */
	const partial = (a: SourceStatus) => {
		const search = a.resources.includes('search');
		const play = a.resources.includes('stream');
		return search === play ? '' : search ? 'Search only' : 'Playback only';
	};
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
		const parts = [u.streams ? `${whole(u.streams)} played` : '', u.downloads ? `${whole(u.downloads)} saved` : ''].filter(Boolean);
		const today = parts.length ? `Today: ${parts.join(', ')}` : 'Not used today';
		return u.found_at_ms ? `${today} · last song ${ago(u.found_at_ms)}` : today;
	}
	/** Its last trouble, when that came after the last song it gave. */
	const troubleOf = (id: string) => {
		const u = used[id];
		return u?.trouble && u.trouble.at_ms > (u.found_at_ms ?? 0) ? u.trouble : null;
	};
	/** Where it is, when that is nearer than the internet: it may send Stash only as near as that. */
	const where = (a: SourceStatus) => (a.reach === 'home' ? 'on your network' : a.reach === 'here' ? 'on this computer' : '');
	const detail = (a: SourceStatus) => (a.builtIn ? [a.id.startsWith('mornye.') ? 'Mornye extension' : 'Built in', partial(a)] : [a.version ? `v${a.version}` : '', a.host ?? '', where(a), partial(a)]).filter((p) => p !== '').join(' · ');
	/** How many of the sources are the listener's own add-ons (they come first in the list). */
	const own = $derived(L.sources.filter((a) => !a.builtIn).length);

	// ---- Library: music folders, and one file that carries a library between this computer, the phone and the web player

	let importing = $state(false);
	/** What the last import or save said. */
	let fileNotice = $state<Notice | null>(null);

	/** What an import did, in a line. */
	function imported(result: core.Imported): string {
		const s = result.summary;
		const parts = [
			s.songs_added ? many(s.songs_added, 'new song', 'new songs') : '',
			s.likes_added ? many(s.likes_added, 'like', 'likes') : '',
			s.playlists_added ? many(s.playlists_added, 'playlist', 'playlists') : '',
			s.playlist_songs_added ? `${many(s.playlist_songs_added, 'song', 'songs')} in existing playlists` : '',
			s.plays_added ? many(s.plays_added, 'play', 'plays') : '',
			s.blocks_added ? `${many(s.blocks_added, 'song', 'songs')} not to play` : ''
		].filter((p) => p !== '');
		return parts.length ? `Imported ${parts.join(', ')}` : 'Nothing new to import';
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
			fileNotice = { text: `Saved ${name}: ${many(likes, 'like', 'likes')}, ${many(lists, 'playlist', 'playlists')}, ${many(plays, 'play', 'plays')}`, trouble: false };
		} catch (x) {
			fileNotice = { text: trouble(x), trouble: true };
		}
	}

	// ---- Playback: autoplay, similar music from Last.fm (radio, and autoplay that knows what is like what), lyrics from LRCLIB

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
		const main = document.getElementById('main');
		main?.addEventListener('scroll', spy, { passive: true });
		return () => {
			left = true;
			window.removeEventListener('focus', back);
			main?.removeEventListener('scroll', spy);
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
		['system', 'Automatic']
	];
</script>

<div class="view view-in view-settings">
	<h1 class="vh2">Settings</h1>

	<div class="set-layout">
		<nav class="set-nav" aria-label="Settings sections">
			{#each groups as [id, name] (id)}
				<button type="button" class="set-nav-i" aria-current={current === id ? 'true' : undefined} onclick={() => go(id)}>{name}</button>
			{/each}
		</nav>

		<div class="set-body">
			<!-- ===== Sound -->
			<section class="set-sec" id="sound" aria-labelledby="h-sound">
				<h2 class="set-g" id="h-sound" tabindex="-1">Sound</h2>
				{#if !S.v.available}
					<div class="set-card">
						<div class="set-na" role="status">
							<b>No sound</b>
							<span>{S.v.reason ?? 'The player isn’t available'}</span>
						</div>
					</div>
				{:else}
					<div class="set-card">
						{#if outs && outs.devices.length > 1}
							<div class="set-row">
								<div class="set-lab">
									<label class="set-name" for="out-pick">Output</label>
									{#if outs.missing}<span class="set-sub" role="status">{outs.missing.description || 'Your output'} isn’t connected. Using the default.</span>{/if}
									{#if outTrouble}<span class="set-sub set-trouble" role="alert">{outTrouble}</span>{/if}
								</div>
								{#key outs}
									<select id="out-pick" class="dev-input set-select" onchange={(e) => chooseOut(e.currentTarget.value)}>
										{#each outs.devices as d (d.name)}<option value={d.name} selected={outs.device === d.name}>{deviceName(d)}</option>{/each}
									</select>
								{/key}
							</div>
						{/if}
						<div class="set-row">
							<div class="set-lab"><span class="set-name" id="rg-l">Even out volume</span></div>
							<div class="chips" role="group" aria-labelledby="rg-l">
								{#each RG as [mode, label] (mode)}
									<button type="button" class="chip" class:on={S.v.replaygain === mode} aria-pressed={S.v.replaygain === mode} onclick={() => player.setReplaygain(mode)}>{label}</button>
								{/each}
							</div>
						</div>
					</div>

					<div class="set-card">
						<div class="set-row">
							<div class="set-lab">
								<span class="set-name" id="eq-l">Equalizer</span>
								{#if eqFor}<span class="set-sub">{eqFor}</span>{/if}
							</div>
							<button type="button" class="sw" role="switch" aria-checked={eq.on} aria-labelledby="eq-l" onclick={() => setEq({ on: !eq.on })}><i></i></button>
						</div>
						<div class="set-row">
							<div class="set-lab">
								<span class="set-name" id="corr-l">Headphone correction</span>
								{#if eq.correction}
									<span class="set-sub eq-name" title={eq.correction.summary}>{eq.correction.name}</span>
								{:else}
									<span class="set-sub">From autoeq.app, in Equalizer APO format</span>
								{/if}
								{#if eqRefused}<span class="set-sub set-trouble" role="alert">{eqRefused}</span>{/if}
							</div>
							<div class="set-ctl" role="group" aria-labelledby="corr-l">
								{#if eq.correction}
									<button type="button" class="btn ghost" disabled={eqPicking} onclick={chooseEqFile}>Change…</button>
									<button type="button" class="btn ghost" onclick={() => setEq({ correction: null })}>Remove</button>
								{:else}
									<button type="button" class="btn ghost" disabled={eqPicking} onclick={chooseEqFile}>Choose file…</button>
								{/if}
							</div>
						</div>
						{#each TONES as [which, name] (which)}
							<div class="set-row">
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
					</div>

					<details class="set-more">
						<summary><Icon name="expand_more" class="ic set-chev" />Advanced</summary>
						<div class="set-card">
							<div class="set-row">
								<div class="set-lab">
									<span class="set-name" id="ex-l">Exclusive mode</span>
									<span class="set-sub">Bit-perfect output; other apps go silent</span>
									{#if now}<span class="set-sub" role="status">{now}</span>{/if}
								</div>
								<button type="button" class="sw" role="switch" aria-checked={S.v.exclusive} aria-labelledby="ex-l" onclick={() => player.setExclusive(!S.v.exclusive)}><i></i></button>
							</div>
						</div>
					</details>
				{/if}
			</section>

			<!-- ===== Playback -->
			<section class="set-sec" id="playback" aria-labelledby="h-playback">
				<h2 class="set-g" id="h-playback" tabindex="-1">Playback</h2>
				<div class="set-card">
					<div class="set-row">
						<div class="set-lab">
							<span class="set-name" id="ap-l">Autoplay</span>
							<span class="set-sub">Plays similar songs when the queue ends</span>
						</div>
						<button type="button" class="sw" role="switch" aria-checked={autoplay === true} aria-labelledby="ap-l" disabled={autoplay === null} onclick={() => autoplay !== null && setAutoplay(!autoplay)}><i></i></button>
					</div>
					{#if R.available}
						<div class="set-row" id="similar">
							<div class="set-lab">
								<span class="set-name" id="lfm-l">Similar music</span>
								<span class="set-sub">Last.fm suggestions for radio and autoplay</span>
							</div>
							<button type="button" class="sw" role="switch" aria-checked={R.on} aria-labelledby="lfm-l" disabled={similarBusy} onclick={() => void setSimilar(!R.on)}><i></i></button>
						</div>
					{/if}
					<div class="set-row" id="lyrics">
						<div class="set-lab">
							<span class="set-name" id="lrc-l">Find lyrics online</span>
							<span class="set-sub">From LRCLIB when a song has none</span>
						</div>
						<button type="button" class="sw" role="switch" aria-checked={lrclib === true} aria-labelledby="lrc-l" disabled={lrclib === null} onclick={() => lrclib !== null && setLrclib(!lrclib)}><i></i></button>
					</div>
				</div>
				<Blocked />
			</section>

			<!-- ===== Sources -->
			<section class="set-sec" id="sources" aria-labelledby="h-sources">
				<h2 class="set-g" id="h-sources" tabindex="-1">Sources</h2>

				<section class="set-part" id="addons" aria-labelledby="h-addons">
					<h3 class="set-h" id="h-addons">Add-ons</h3>
					<div class="set-card">
						<div class="set-row set-col">
							{#if own > 1}<span class="set-sub">Searched in this order</span>{/if}
							{#if L.sources.length}
								<ol class="src-list">
									{#each L.sources as a, i (a.id)}
										<li class="src" class:off={!a.enabled}>
											<span class="src-n" aria-hidden="true">{i + 1}</span>
											<span class="tr-t">
												<b title={a.description ?? undefined}>{a.name}</b>
												<i>{detail(a)}</i>
												{#if useLine(a.id)}<i class="src-use">{useLine(a.id)}</i>{/if}
												{#if troubleOf(a.id)}<i class="src-problem">Problem {ago(troubleOf(a.id)!.at_ms)}: {troubleOf(a.id)!.message}</i>{/if}
												{#if a.problem}<i class="src-problem">Can’t be used: {a.problem}</i>{/if}
											</span>
											<span class="src-acts">
												<button type="button" class="sw" role="switch" aria-checked={a.enabled} aria-label={a.name} title={a.enabled ? 'On' : 'Off'} onclick={() => addonChange(() => sources.setEnabled(a.id, !a.enabled))}><i></i></button>
												{#if !a.builtIn}
													<button type="button" class="act-ib" disabled={i === 0} aria-label="Move {a.name} up" title="Move up" onclick={() => move(i, -1)}><Icon name="arrow_upward" /></button>
													<button type="button" class="act-ib" disabled={i >= own - 1} aria-label="Move {a.name} down" title="Move down" onclick={() => move(i, 1)}><Icon name="arrow_downward" /></button>
													<button type="button" class="btn ghost" aria-label="Check {a.name} for updates" title="Check for updates" disabled={checking !== null || ui.offline} onclick={() => check(a)}>{checking === a.id && offer?.id !== a.id ? 'Checking…' : 'Check'}</button>
													<button type="button" class="btn ghost" aria-label="Remove {a.name}" onclick={() => { if (offer?.id === a.id) offer = null; void addonChange(() => sources.remove(a.id)); }}>Remove</button>
												{/if}
											</span>
											{#if offer?.id === a.id}
												<div class="src-offer" role="group" aria-labelledby="offer-h">
													<b id="offer-h">{offer.version !== a.version ? `Version ${offer.version}` : `${a.name} has changed`}</b>
													<ul>
														{#each offer.changes as c (c.words)}<li class:gives={c.more}>{c.words}</li>{/each}
													</ul>
													{#if offer.changes.some((c) => c.more)}<p class="src-problem">This update asks for more access</p>{/if}
													<div class="src-offer-acts">
														<button type="button" class="btn" disabled={checking !== null} onclick={takeOffer}>{checking === a.id ? 'Updating…' : 'Update'}</button>
														<button type="button" class="btn ghost" onclick={() => (offer = null)}>Not now</button>
													</div>
												</div>
											{/if}
										</li>
									{/each}
								</ol>
							{/if}
							<form class="dev-rename" onsubmit={add}>
								<input class="dev-input" type="url" placeholder="Paste an add-on link" bind:value={link} aria-label="Add-on link" autocomplete="off" spellcheck="false" />
								<button type="submit" class="btn" disabled={adding || link.trim() === ''}>{adding ? 'Adding…' : 'Add'}</button>
							</form>
							{#if addonTrouble}<p class="set-sub set-trouble" role="alert">{addonTrouble}</p>{/if}
							{#if addonNote}<p class="set-sub" role="status">{addonNote}</p>{/if}
						</div>
					</div>
				</section>

				<section class="set-part" id="youtube" aria-labelledby="h-youtube">
					<h3 class="set-h" id="h-youtube">YouTube Music</h3>
					<div class="set-card">
						<TubeAccount />
						<TubeFallback />
					</div>
				</section>

				<section class="set-part" id="mornye" aria-labelledby="h-mornye">
					<h3 class="set-h" id="h-mornye">Mornye extensions</h3>
					<MornyeExtensions />
				</section>
			</section>

			<!-- ===== Library -->
			<section class="set-sec" id="library" aria-labelledby="h-library">
				<h2 class="set-g" id="h-library" tabindex="-1">Library</h2>
				<div class="set-card">
					<div class="set-row">
						<div class="set-lab">
							<span class="set-name">Music folders</span>
							{#if folders.list.length}
								{#each folders.list as f (f)}<span class="set-sub set-path" title={f}>{f}</span>{/each}
							{:else}
								<span class="set-sub">Your files stay where they are</span>
							{/if}
							{#if folders.scan}<span class="set-sub" role="status">Scanning {folders.scan.folder ? lastPart(folders.scan.folder) : ''}… {many(folders.scan.report?.files ?? 0, 'file', 'files')}</span>{/if}
							{#if folders.notice}<span class="set-sub" class:set-trouble={folders.notice.trouble} role="status">{folders.notice.text}</span>{/if}
						</div>
						{#if !folders.scan}
							<div class="set-ctl">
								{#if folders.list.length}<button type="button" class="btn ghost" onclick={() => folders.again()}>Rescan</button>{/if}
								<button type="button" class="btn ghost" onclick={() => folders.add()}>Add folder…</button>
							</div>
						{/if}
					</div>
				</div>

				<div class="set-card">
					<div class="set-row">
						<div class="set-lab">
							<span class="set-name" id="off-l">Offline mode</span>
							<span class="set-sub">Only play what’s on this computer</span>
						</div>
						<button type="button" class="sw" role="switch" aria-checked={ui.offline} aria-labelledby="off-l" onclick={() => void goOffline(!ui.offline)}><i></i></button>
					</div>
					{#if room}
						<div class="set-row">
							<div class="set-lab">
								<span class="set-name" id="kp-l">Keep songs you play</span>
								{#if room.songs}
									<span class="set-sub" role="status">{many(room.songs, 'song', 'songs')} kept · {roomSize(room.bytes)} <button type="button" class="set-clear" onclick={clearRoom}>Clear</button></span>
								{:else}
									<span class="set-sub">Saves streamed songs for offline play</span>
								{/if}
							</div>
							<div class="chips" role="group" aria-labelledby="kp-l">
								{#each ROOMS as [mb, label] (mb)}
									<button type="button" class="chip" class:on={room.cap_mb === mb} aria-pressed={room.cap_mb === mb} onclick={() => setRoom(mb)}>{label}</button>
								{/each}
							</div>
						</div>
					{/if}
				</div>

				<div class="set-card">
					<div class="set-row">
						<div class="set-lab">
							<span class="set-name">Transfer library</span>
							<span class="set-sub">Likes, playlists and plays, across devices</span>
							{#if fileNotice}<span class="set-sub" class:set-trouble={fileNotice.trouble} role="status">{fileNotice.text}</span>{/if}
						</div>
						<div class="set-ctl">
							<label class="btn ghost bk-pick" aria-disabled={importing}>{importing ? 'Importing…' : 'Import…'}<input class="vh" type="file" accept=".json,.zip,.db,application/json,application/zip" aria-label="Import a library file" disabled={importing} onchange={importFile} /></label>
							<button type="button" class="btn ghost" aria-label="Export a library file" onclick={saveFile}>Export</button>
						</div>
					</div>
					<div class="set-row">
						<div class="set-lab">
							<span class="set-name">Import a playlist</span>
							<span class="set-sub">M3U files from other players</span>
						</div>
						<div class="set-ctl">
							<button type="button" class="btn ghost" aria-label="Import a playlist file" onclick={() => void importPlaylistFile()}>Import…</button>
						</div>
					</div>
				</div>
			</section>

			<!-- ===== Scrobbling -->
			<section class="set-sec" id="scrobbling" aria-labelledby="h-scrobbling">
				<h2 class="set-g" id="h-scrobbling" tabindex="-1">Scrobbling</h2>
				<Scrobbling />
			</section>

			<!-- ===== Shortcuts (its own section, shown where the app can hold keys) -->
			<Shortcuts bind:shown={shortcutsShown} />

			<!-- ===== Appearance -->
			<section class="set-sec" id="appearance" aria-labelledby="h-appearance">
				<h2 class="set-g" id="h-appearance" tabindex="-1">Appearance</h2>
				<div class="set-card">
					<div class="set-row">
						<div class="set-lab"><span class="set-name" id="th-l">Theme</span></div>
						<div class="chips" role="group" aria-labelledby="th-l">
							{#each THEMES as [id, label] (id)}
								<button type="button" class="chip" class:on={look.theme === id} aria-pressed={look.theme === id} onclick={() => setLook({ ...look, theme: id })}>{label}</button>
							{/each}
						</div>
					</div>
					<div class="set-row">
						<div class="set-lab"><span class="set-name" id="bk-l">Pure black</span><span class="set-sub">For OLED screens</span></div>
						<button type="button" class="sw" role="switch" aria-checked={look.black} aria-labelledby="bk-l" onclick={() => setLook({ ...look, black: !look.black })}><i></i></button>
					</div>
					{#if !core.measuring}
						<div class="set-row">
							<div class="set-lab"><span class="set-name" id="tray-l">Keep playing when closed</span><span class="set-sub">Stash stays in the tray</span></div>
							<button type="button" class="sw" role="switch" aria-checked={tray === true} aria-labelledby="tray-l" disabled={tray === null} onclick={() => tray !== null && setTray(!tray)}><i></i></button>
						</div>
					{/if}
				</div>
			</section>

			<!-- ===== About -->
			<section class="set-sec" id="about" aria-labelledby="h-about">
				<h2 class="set-g" id="h-about" tabindex="-1">About</h2>
				<div class="set-card">
					<div class="set-row">
						<span class="set-name">Version</span>
						<span class="set-sub">{__STASH_VERSION__}</span>
					</div>
				</div>
			</section>
		</div>
	</div>
</div>
