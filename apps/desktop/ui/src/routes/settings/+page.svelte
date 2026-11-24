<!-- Settings, as a system's own settings: the categories down the left (an icon and a name each), the chosen one's
     settings on the right, one category at a time. The category is in the address (/settings?c=sound), so back and
     forward step through them; the older links to a part (#lyrics, #youtube…) open its category and scroll to it. In a
     narrow column the list fills the page, and a category opens in its place with "Settings" to go back. -->
<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { afterNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import { sources } from '../../lib/app';
	import * as core from '../../lib/core';
	import { folders } from '../../lib/desktop/folders.svelte';
	import { R } from '../../lib/desktop/radio.svelte';
	import Scrobbling from '../../lib/desktop/Scrobbling.svelte';
	import Shortcuts from '../../lib/desktop/Shortcuts.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import type { IconName } from '../../lib/ui/icons';
	import Appearance from './Appearance.svelte';
	import Library from './Library.svelte';
	import Playback from './Playback.svelte';
	import Sound from './Sound.svelte';
	import HowStashRuns from '../../lib/desktop/HowStashRuns.svelte';
	import YourPhone from '../../lib/desktop/YourPhone.svelte';
	import { ui } from '../../lib/ui/state.svelte';
	import Sources from './Sources.svelte';
	import { prefs, readOuts, readUse } from './prefs.svelte';
	import './settings.css';

	// ---- The categories, and the one shown

	type Cat = 'general' | 'phone' | 'sound' | 'playback' | 'sources' | 'library' | 'scrobbling' | 'shortcuts' | 'appearance' | 'about';
	const CATS: [Cat, string, IconName][] = [
		['general', 'General', 'settings'],
		['phone', 'Your phone', 'smartphone'],
		['sound', 'Sound', 'volume_up'],
		['playback', 'Playback', 'play_arrow'],
		['sources', 'Sources', 'extension'],
		['library', 'Library', 'library_music'],
		['scrobbling', 'Scrobbling', 'history'],
		['shortcuts', 'Shortcuts', 'keyboard'],
		['appearance', 'Appearance', 'palette'],
		['about', 'About', 'info']
	];
	/** The parts of a category that links name by their own id. */
	const PARTS: Record<string, Cat> = { similar: 'playback', lyrics: 'playback', 'not-played': 'playback', addons: 'sources', tidal: 'sources', youtube: 'sources', mornye: 'sources' };
	const isCat = (v: string | null | undefined): v is Cat => CATS.some(([id]) => id === v);

	/** Whether this computer can hold keys system-wide (null until the core says): Shortcuts is listed only where it can. */
	let shortcutsOk = $state<boolean | null>(null);
	// On the listener's own files only, there are no sources to set up and nothing to scrobble to.
	const cats = $derived(CATS.filter(([id]) => (id !== 'shortcuts' || shortcutsOk === true) && (!ui.local || (id !== 'sources' && id !== 'scrobbling' && id !== 'phone'))));

	function idOf(hash: string): string {
		try {
			return decodeURIComponent(hash.slice(1));
		} catch {
			return '';
		}
	}
	/** The part a link names (#lyrics), if any. */
	const part = $derived(idOf(page.url.hash));
	/** The category asked for: by a part's link, or by ?c=. None: the list alone in a narrow column, Sound beside it. */
	const picked = $derived.by<Cat | null>(() => {
		const c = isCat(part) ? part : (PARTS[part] ?? page.url.searchParams.get('c'));
		if (!isCat(c) || (c === 'shortcuts' && shortcutsOk === false)) return null;
		return c;
	});
	const cat = $derived<Cat>(picked ?? 'sound');
	const title = $derived(CATS.find(([id]) => id === cat)?.[1] ?? '');
	const href = (id: Cat) => `/settings?c=${id}`;

	/** The page's own width: the window's right panel often leaves it narrow, whatever the window's size. */
	let width = $state(0);
	const narrow = $derived(width > 0 && width < 660);
	const listShown = $derived(!narrow || picked === null);
	const paneShown = $derived(!narrow || picked !== null);
	let heading = $state<HTMLElement>();

	// ---- Moving between categories, and to a part

	/** The category asked for before this navigation. */
	let was: Cat | null = null;
	/** The scroll to a part under way (a newer one stops it). */
	let seeking = 0;

	afterNavigate((nav) => {
		const before = was;
		was = picked;
		const inside = !!nav.from && nav.from.url.pathname === nav.to?.url.pathname;
		if (!inside) {
			if (part && !isCat(part)) seek(part);
			return;
		}
		// Back or forward to another category: it starts at its top (the layout does so for the other navigations).
		const main = document.getElementById('main');
		if (nav.type === 'popstate' && before !== picked && main && !part) main.scrollTop = 0;
		if (part && !isCat(part)) seek(part);
		// The link that was used is gone (a narrow column swaps the list and the category): focus goes where the eye does.
		void tick().then(() => {
			const at = document.activeElement;
			if (at && at !== document.body) return;
			if (paneShown) heading?.focus({ preventScroll: true });
			else document.querySelector<HTMLElement>(`.set-nav-i[data-cat="${before ?? cat}"]`)?.focus({ preventScroll: true });
		});
	});

	/** Scrolls a part into view once it is drawn (some appear when what they show has been read), and keeps it there
	 *  while the parts above it settle, for up to 2.5 s, unless the listener scrolls first. */
	function seek(id: string) {
		const mine = ++seeking;
		let tries = 0;
		let moved = false;
		const stop = () => (moved = true);
		const events = ['wheel', 'keydown', 'pointerdown'] as const;
		const quit = () => {
			for (const k of events) removeEventListener(k, stop);
		};
		for (const k of events) addEventListener(k, stop, { once: true, passive: true });
		const step = () => {
			if (moved || mine !== seeking) return quit();
			const el = document.getElementById(id);
			if (el) {
				const main = document.getElementById('main');
				// (16: the part's scroll margin)
				const off = main ? el.getBoundingClientRect().top - main.getBoundingClientRect().top - 16 : 0;
				if (Math.abs(off) > 4) el.scrollIntoView({ block: 'start' });
			}
			if (++tries < 25) setTimeout(step, 100);
			else quit();
		};
		requestAnimationFrame(step);
	}

	onMount(() => {
		void folders.init();
		void sources.reload().catch(() => {});
		void core.lyricsSettings().then(
			(s) => (prefs.lrclib = s.lrclib),
			() => {}
		);
		void core.windowSettings().then(
			(s) => (prefs.tray = s.tray),
			() => {}
		);
		void core.autoplaySettings().then(
			(s) => (prefs.autoplay = s.on),
			() => {}
		);
		void core.shortcuts().then(
			(s) => (shortcutsOk = s.available),
			() => (shortcutsOk = false)
		);
		void R.refresh();
		void readUse();
		const useTimer = setInterval(() => void readUse(), 15_000);
		void core.playedRoom().then(
			(r) => (prefs.room = r),
			() => {}
		);
		void readOuts();
		const back = () => {
			void readOuts();
			void readUse();
		};
		window.addEventListener('focus', back);
		return () => {
			seeking++;
			window.removeEventListener('focus', back);
			clearInterval(useTimer);
		};
	});
</script>

<div class="view view-in view-settings" class:narrow bind:clientWidth={width}>
	{#if listShown}
		<div class="set-side">
			<h1 class="vh2 set-h1">Settings</h1>
			<nav aria-label="Settings categories">
				<ul class="set-nav">
					{#each cats as [id, name, icon] (id)}
						{@const on = !narrow && cat === id}
						<li>
							<a class="set-nav-i" class:on href={href(id)} data-cat={id} aria-current={on ? 'page' : undefined} data-sveltekit-keepfocus data-sveltekit-noscroll>
								<Icon name={icon} />
								<span>{name}</span>
								{#if narrow}<Icon name="chevron_right" class="ic set-nav-go" />{/if}
							</a>
						</li>
					{/each}
				</ul>
			</nav>
		</div>
	{:else}
		<h1 class="vh">Settings</h1>
	{/if}

	{#if paneShown}
		<div class="set-pane">
			{#if narrow}
				<a class="set-back" href="/settings" data-sveltekit-keepfocus data-sveltekit-noscroll><Icon name="chevron_left" /><span>Settings</span></a>
			{/if}
			{#key cat}
				<section class="set-cat view-in" id={cat} aria-labelledby="set-title">
					<h2 class="set-title" id="set-title" tabindex="-1" bind:this={heading}>{title}</h2>
					<div class="set-body">
						{#if cat === 'general'}
							<section class="set-card">
								<p class="set-name" style="margin: 0 0 10px">How Stash runs</p>
								<HowStashRuns />
							</section>
						{:else if cat === 'phone'}
							<YourPhone />
						{:else if cat === 'sound'}
							<Sound />
						{:else if cat === 'playback'}
							<Playback />
						{:else if cat === 'sources'}
							<Sources />
						{:else if cat === 'library'}
							<Library />
						{:else if cat === 'scrobbling'}
							<Scrobbling />
						{:else if cat === 'shortcuts'}
							<Shortcuts />
						{:else if cat === 'appearance'}
							<Appearance />
						{:else}
							<div class="set-card">
								<div class="set-row">
									<span class="set-name">Version</span>
									<span class="set-sub">{__STASH_VERSION__}</span>
								</div>
							</div>
						{/if}
					</div>
				</section>
			{/key}
		</div>
	{/if}
</div>
