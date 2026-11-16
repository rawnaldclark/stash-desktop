<!-- The command palette (Ctrl+K): one box that finds anything in the library and does anything the window can. Commands
     first (play, skip, shuffle, a page), then the library's songs, albums and artists, then playlists. Arrow keys move,
     Enter chooses, Escape closes and gives focus back. -->
<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { goOffline, itemOfRow, player } from '../app';
	import * as core from '../core';
	import Icon from './Icon.svelte';
	import { R } from '../desktop/radio.svelte';
	import type { IconName } from './icons';
	import { L, S, ui } from './state.svelte';

	type Choice = { id: string; group: string; icon: IconName; label: string; sub?: string; run: () => void };

	let q = $state('');
	let active = $state(0);
	let input = $state<HTMLInputElement>();
	let list = $state<HTMLUListElement>();
	let found = $state<{ rows: core.TrackRow[]; albums: core.AlbumRow[]; artists: core.ArtistRow[] } | null>(null);
	let canMini = $state(false);
	const before = document.activeElement as HTMLElement | null;

	const close = () => {
		ui.palette = false;
		void tick().then(() => before?.focus?.());
	};
	const go = (href: string) => () => void goto(href);

	/** What the window can do, in words a listener would type. */
	const commands = $derived.by((): Choice[] => {
		const s = S.v;
		const list: Choice[] = [];
		if (s.current) {
			list.push({ id: 'toggle', group: 'Commands', icon: s.playing ? 'pause' : 'play_arrow', label: s.playing ? 'Pause' : 'Play', run: () => player.toggle() });
			list.push({ id: 'next', group: 'Commands', icon: 'skip_next', label: 'Next song', run: () => player.next() });
			list.push({ id: 'prev', group: 'Commands', icon: 'skip_previous', label: 'Previous song', run: () => player.prev() });
			const liked = L.isLiked(s.current);
			list.push({ id: 'like', group: 'Commands', icon: liked ? 'favorite' : 'favorite_border', label: liked ? 'Remove the song playing from Likes' : 'Like the song playing', sub: s.current.title, run: () => void L.toggleLike(s.current!) });
			list.push({ id: 'now', group: 'Commands', icon: 'open_in_full', label: 'Now playing', run: go('/now') });
			if (R.ready && player.canRadio(s.current)) {
				const current = s.current;
				list.push({ id: 'radio', group: 'Commands', icon: 'radio', label: 'Start radio from the song playing', sub: current.title, run: () => void player.startRadio(current) });
			}
		}
		if (s.station) {
			const autoplay = s.station.kind === 'autoplay';
			list.push({
				id: 'radio-stop',
				group: 'Commands',
				icon: 'close',
				label: autoplay ? 'Stop autoplay' : 'Stop radio',
				run: () => void player.stopRadio().then((stopped) => stopped && ui.toast(autoplay ? 'Autoplay stopped' : 'Radio stopped'))
			});
		}
		list.push({ id: 'shuffle', group: 'Commands', icon: 'shuffle', label: s.shuffle ? 'Shuffle off' : 'Shuffle on', run: () => player.setShuffle(!s.shuffle) });
		const repeat = s.repeat === 'off' ? 'all' : s.repeat === 'all' ? 'one' : 'off';
		list.push({ id: 'repeat', group: 'Commands', icon: repeat === 'one' ? 'repeat_one' : 'repeat', label: repeat === 'off' ? 'Repeat off' : repeat === 'all' ? 'Repeat the queue' : 'Repeat the song', run: () => player.setRepeat(repeat) });
		if (canMini) list.push({ id: 'mini', group: 'Commands', icon: 'picture_in_picture_alt', label: 'Mini player', run: () => void core.showView('mini').catch(() => {}) });
		list.push({ id: 'upnext', group: 'Commands', icon: 'queue_music', label: 'Up next', run: () => ui.setTab('next') });
		list.push({ id: 'smart', group: 'Commands', icon: 'add', label: 'New smart playlist', run: () => (ui.smart = { id: null }) });
		list.push({ id: 'offline', group: 'Commands', icon: 'download', label: ui.offline ? 'Go back online' : 'Go offline (only what is on this computer)', run: () => void goOffline(!ui.offline) });
		list.push({ id: 'lyrics', group: 'Commands', icon: 'lyrics', label: 'Lyrics', run: () => ui.setTab('lyrics') });
		const pages: [string, string, IconName][] = [
			['Home', '/', 'home'],
			['Search', '/search', 'search'],
			['Library › Songs', '/library?tab=songs', 'library_music'],
			['Library › Liked', '/library?tab=likes', 'favorite'],
			['Library › Playlists', '/library?tab=playlists', 'queue_music'],
			['Library › Albums', '/library?tab=albums', 'album'],
			['Library › Artists', '/library?tab=artists', 'person'],
			['Library › Downloads', '/library?tab=downloads', 'download'],
			['Library › Not found', '/library?tab=notfound', 'search'],
			['Library › History', '/library?tab=history', 'history'],
			['Settings › Sound', '/settings#sound', 'settings'],
			...(R.available ? [['Settings › Similar music from Last.fm', '/settings#similar', 'settings'] as [string, string, IconName]] : []),
			['Settings › Add-ons', '/settings#addons', 'settings'],
			['Settings › Lyrics', '/settings#lyrics', 'settings'],
			['Settings › Your library', '/settings#library', 'settings'],
			['Settings › Scrobbling', '/settings#scrobbling', 'settings'],
			['Settings › Not played', '/settings#not-played', 'settings'],
			['Settings › Shortcuts', '/settings#shortcuts', 'settings'],
			['Settings › Appearance', '/settings#appearance', 'settings']
		];
		for (const [label, href, icon] of pages) list.push({ id: `go:${href}`, group: 'Go to', icon, label, run: go(href) });
		return list;
	});

	/** Every word typed starts a word of the label (or the label holds what was typed as it is). */
	function matches(label: string, typed: string) {
		const words = label.toLowerCase().split(/[\s›·/]+/).filter(Boolean);
		const asked = typed.toLowerCase().split(/\s+/).filter(Boolean);
		return label.toLowerCase().includes(typed.toLowerCase()) || asked.every((a) => words.some((w) => w.startsWith(a)));
	}

	const choices = $derived.by((): Choice[] => {
		const typed = q.trim();
		if (!typed) return commands.filter((c) => c.group === 'Commands').concat(commands.filter((c) => c.group === 'Go to').slice(0, 6));
		const out: Choice[] = commands.filter((c) => matches(c.label, typed)).slice(0, 6);
		if (found) {
			const songs = found.rows.slice(0, 6);
			songs.forEach((r) =>
				out.push({ id: `song:${r.id}`, group: 'Songs', icon: 'play_arrow', label: r.title, sub: r.artist, run: () => void player.playItems([itemOfRow(r)], 0, { from: 'Search' }) })
			);
			found.albums.slice(0, 3).forEach((a) => out.push({ id: `album:${a.id}`, group: 'Albums', icon: 'album', label: a.title, sub: a.artist, run: go(`/album/library/${a.id}`) }));
			found.artists.slice(0, 3).forEach((a) => out.push({ id: `artist:${a.id}`, group: 'Artists', icon: 'person', label: a.name, run: go(`/artist/library/${a.id}`) }));
		}
		L.playlists
			.filter((p) => matches(p.name, typed))
			.slice(0, 3)
			.forEach((p) => out.push({ id: `pl:${p.id}`, group: 'Playlists', icon: 'library_music', label: p.name, run: go(`/playlist/${p.id}`) }));
		return out;
	});

	// The library is asked a moment after typing stops; an answer to older words is dropped.
	let asking: AbortController | null = null;
	let timer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => {
		const typed = q.trim();
		clearTimeout(timer);
		asking?.abort();
		if (!typed) {
			found = null;
			return;
		}
		timer = setTimeout(() => {
			const mine = (asking = new AbortController());
			core.searchLibrary(typed, 6, mine.signal).then(
				(r) => {
					if (mine === asking) found = r;
				},
				() => {}
			);
		}, 120);
	});
	$effect(() => {
		void choices.length;
		if (active >= choices.length) active = Math.max(0, choices.length - 1);
	});
	$effect(() => {
		list?.querySelector<HTMLElement>(`[data-i="${active}"]`)?.scrollIntoView({ block: 'nearest' });
	});

	function choose(c: Choice | undefined) {
		if (!c) return;
		ui.palette = false;
		c.run();
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			close();
		} else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			const n = choices.length;
			if (n) active = (active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n;
		} else if (e.key === 'Enter') {
			e.preventDefault();
			choose(choices[active]);
		}
	}

	onMount(() => {
		input?.focus();
		core.windowInfo().then(
			(w) => (canMini = w.mini),
			() => {}
		);
		return () => {
			clearTimeout(timer);
			asking?.abort();
		};
	});
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="pal-veil" role="presentation" onclick={close}></div>
<div class="pal" role="dialog" aria-modal="true" aria-label="Find or do anything">
	<label class="pal-q">
		<Icon name="search" />
		<input
			bind:this={input}
			bind:value={q}
			onkeydown={key}
			type="text"
			role="combobox"
			aria-expanded="true"
			aria-controls="pal-list"
			aria-activedescendant={choices.length ? `pal-${active}` : undefined}
			aria-autocomplete="list"
			placeholder="Find a song, or do something"
			maxlength="100"
			spellcheck="false"
			autocomplete="off"
		/>
		<kbd>Esc</kbd>
	</label>
	<ul class="pal-list" id="pal-list" role="listbox" aria-label="Choices" bind:this={list}>
		{#each choices as c, i (c.id)}
			{#if i === 0 || choices[i - 1].group !== c.group}<li class="caps pal-g" role="presentation">{c.group}</li>{/if}
			<!-- The input moves through the choices with the arrow keys (a combobox); a click chooses too. -->
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<li id="pal-{i}" data-i={i} role="option" aria-selected={i === active} class="pal-o" class:on={i === active} onmousemove={() => (active = i)} onclick={() => choose(c)}>
				<Icon name={c.icon} />
				<span class="pal-t"><b dir="auto">{c.label}</b>{#if c.sub}<i dir="auto">{c.sub}</i>{/if}</span>
			</li>
		{/each}
		{#if q.trim() && !choices.length}<li class="pal-none" role="presentation">Nothing for “{q.trim()}”.</li>{/if}
	</ul>
</div>

<style>
	.pal-veil { position: fixed; inset: 0; z-index: 80; background: rgb(0 0 0 / .32); }
	.pal { position: fixed; z-index: 81; left: 50%; top: 12vh; transform: translateX(-50%); width: min(620px, calc(100vw - 32px)); max-height: 72vh; display: flex; flex-direction: column; border-radius: 14px; background: var(--wp-main); border: 1px solid var(--wp-line-2); box-shadow: var(--shadow-card); overflow: hidden; color: var(--wp-text); }
	.pal-q { display: flex; align-items: center; gap: 10px; height: 52px; padding: 0 14px; border-bottom: 1px solid var(--wp-line); color: var(--wp-text-2); }
	.pal-q :global(.ic) { width: 20px; height: 20px; flex: none; }
	.pal-q input { flex: 1; min-width: 0; height: 100%; border: 0; background: none; color: var(--wp-text); font: 400 16px/1 var(--font-body); outline: none; }
	.pal-q kbd { padding: 2px 6px; border-radius: 6px; border: 1px solid var(--wp-line-2); color: var(--wp-text-2); font: 500 11px/1.4 var(--font-body); }
	.pal-list { overflow-y: auto; padding: 6px; margin: 0; list-style: none; }
	.pal-g { margin: 10px 10px 4px; }
	.pal-g:first-child { margin-top: 4px; }
	.pal-o { display: flex; align-items: center; gap: 12px; min-height: 40px; padding: 6px 10px; border-radius: 8px; cursor: pointer; }
	.pal-o.on { background: var(--wp-hover); }
	.pal-o :global(.ic) { width: 18px; height: 18px; flex: none; color: var(--wp-text-2); }
	.pal-t { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
	.pal-t b { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font: 500 14px/1.35 var(--font-body); }
	.pal-t i { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; color: var(--wp-text-2); font: 400 13px/1.35 var(--font-body); }
	.pal-none { padding: 14px 10px; color: var(--wp-text-2); font: 400 14px/1.4 var(--font-body); }
	@media (prefers-reduced-motion: no-preference) { .pal { animation: pal-in 140ms var(--ease-out-expo, ease-out) both; } }
	@keyframes pal-in { from { opacity: 0; transform: translate(-50%, -6px); } }
</style>
