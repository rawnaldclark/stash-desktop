<!-- The song menu: Play next, Add to Up next, Add to playlist, Go to artist / album (a song of the library), Like. A real menu (arrow keys, Escape, focus returns).
     For a group (a Home album card, or the songs picked in a list) the queue, playlist and like entries act on every song in it. -->
<script lang="ts">
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { library, notPlayed, player, saveSongs, unsaveSongs } from '../app';
	import { NF } from '../desktop/unfound.svelte';
	import { LIB_REF, sameCopy } from '../identity';
	import { P } from './pick.svelte';
	import { albumHref, albumLink, artistHref, artistLink } from '../identity';
	import Icon from './Icon.svelte';
	import type { IconName } from './icons';
	import { L, ui } from './state.svelte';

	let view = $state<'main' | 'playlists'>('main');
	let el = $state<HTMLElement>();
	let pos = $state({ x: 0, y: 0 });
	let naming = $state(false);
	let name = $state('');

	const m = $derived(ui.menu);
	/** What the queue, playlist and like entries act on: the group's songs, else the one song. */
	const targets = $derived(m ? (m.group?.length ? m.group : [m.item]) : []);
	const isGroup = $derived(!!m?.group?.length);
	/** The group is the songs the listener picked in a list (not an album): no one artist or album to go to. */
	const picked = $derived(isGroup && !!m?.picks);
	/** Every picked song is one the listener chose not to play (by itself). */
	const allBlocked = $derived(picked && targets.every((t) => L.blockedOf(t) === 'song'));
	const allLiked = $derived(targets.length > 0 && targets.every((t) => L.isLiked(t)));
	/** A song no add-on could find: to Library › Not found, opened on it (back on the list if it was set aside). */
	function findIt(row: string) {
		ui.sheet = false;
		void NF.show(Number(row));
	}
	function likeAll() {
		const like = !allLiked;
		for (const t of targets) if (L.isLiked(t) !== like) void (like ? library.like(t) : library.unlike(t));
	}

	$effect(() => {
		if (!m) return;
		view = m.view ?? 'main';
		naming = false;
		name = '';
		pos = { x: m.x, y: m.y };
		const up = !!m.up;
		tick().then(() => {
			if (!el) return;
			const r = el.getBoundingClientRect();
			const x = Math.max(8, Math.min(pos.x, window.innerWidth - r.width - 8));
			const y = Math.max(8, Math.min(up ? pos.y - r.height : pos.y, window.innerHeight - r.height - 8));
			pos = { x, y };
			el.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
		});
	});

	// The other view (the playlists, or back) can be taller: it stays inside the window.
	$effect(() => {
		void view;
		void naming;
		tick().then(() => {
			if (!el) return;
			const h = el.getBoundingClientRect().height;
			if (pos.y + h > window.innerHeight - 8) pos = { ...pos, y: Math.max(8, window.innerHeight - h - 8) };
		});
	});

	function close(refocus = true) {
		const a = m?.anchor;
		ui.closeMenu();
		if (refocus && a?.isConnected) a.focus();
	}

	/** Go to a page, keeping the song playing; a bottom sheet (Up next under compact Now playing) gets out of the way. */
	function go(href: string) {
		ui.sheet = false;
		void goto(href);
	}

	function run(fn: () => void | Promise<unknown>, msg?: string) {
		void fn();
		if (msg) ui.toast(msg);
		close();
	}
	/** A change the core can refuse: the words of success only once it is done (a refusal is said by the shell, in the core's words). */
	function change(fn: () => Promise<unknown>, msg: string) {
		void fn().then(
			() => ui.toast(msg),
			() => {}
		);
		close();
	}

	function onkey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			if (view === 'playlists') {
				view = 'main';
				tick().then(() => el?.querySelector<HTMLElement>('[role="menuitem"]')?.focus());
			} else close();
			return;
		}
		if (naming) return;
		const items = [...(el?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
		const i = items.indexOf(document.activeElement as HTMLElement);
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			items[(i + 1) % items.length]?.focus();
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			items[(i - 1 + items.length) % items.length]?.focus();
		} else if (e.key === 'Home') {
			e.preventDefault();
			items[0]?.focus();
		} else if (e.key === 'End') {
			e.preventDefault();
			items[items.length - 1]?.focus();
		} else if (e.key === 'Tab') close(false);
	}

	/** Chooses not to play the song, or its artist's songs; or to play them again. Said with a way back. */
	function dontPlay(kind: 'song' | 'artist', on: boolean) {
		if (!m) return;
		const item = m.item;
		const what = kind === 'artist' ? item.artist : `“${item.title}”`;
		void notPlayed(kind, item, on).then(
			() =>
				on
					? ui.toast(`Stash won’t play ${what} on its own`, { label: 'Undo', run: () => void notPlayed(kind, item, false) })
					: ui.toast(`${what} plays again`),
			() => {}
		);
		close();
	}

	/** Chooses not to play each picked song (by its words), or to play them again; said with a way back. Songs chosen
	 * not to be played before are left as they were. */
	function dontPlayPicked(on: boolean) {
		const songs = on ? targets.filter((t) => !L.blockedOf(t)) : targets;
		const each = async (to: boolean) => {
			for (const t of songs) await notPlayed('song', t, to);
		};
		void each(on).then(
			() =>
				on
					? ui.toast(`Stash won’t play ${songs.length === 1 ? 'that song' : `these ${songs.length} songs`} on its own`, { label: 'Undo', run: () => void each(false) })
					: ui.toast(`${songs.length === 1 ? 'That song plays' : `These ${songs.length} songs play`} again`),
			() => {}
		);
		close();
	}

	/** Takes the picked songs out of the playlist, the last first so the places before stay where they are. Only while the
	 * playlist still has them at those places: it may have changed since they were picked. */
	function removePicked() {
		if (!m?.playlistId || !m.picks) return;
		const id = m.playlistId;
		const picks = m.picks;
		const songs = targets;
		const now = L.playlists.find((p) => p.id === id)?.items ?? [];
		P.clear();
		if (!picks.every((place, i) => now[place] && songs[i] && sameCopy(now[place], songs[i]))) {
			ui.toast('That playlist changed after the songs were picked. Pick them again.');
			close();
			return;
		}
		const last = [...picks].sort((a, b) => b - a);
		change(async () => {
			for (const place of last) await library.removeFromPlaylist(id, place);
		}, `Removed ${picks.length} songs from this playlist`);
	}

	function createAndAdd() {
		if (!m) return;
		const songs = targets;
		const called = name;
		change(async () => {
			const p = await library.createPlaylist(called);
			await library.addToPlaylist(p.id, songs);
		}, `Added to ${called.trim()}`);
	}
</script>

<svelte:window onpointerdown={(e) => { if (m && el && !el.contains(e.target as Node)) close(false); }} onresize={() => m && close(false)} />

{#snippet entry(icon: IconName, label: string, fn: () => void)}
	<button type="button" role="menuitem" class="mi" onclick={fn}><Icon name={icon} /><span>{label}</span></button>
{/snippet}

{#if m}
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div class="menu" role="menu" aria-label={picked ? `Options for ${targets.length} songs` : isGroup ? 'Album options' : 'Song options'} tabindex="-1" bind:this={el} style="left:{pos.x}px;top:{pos.y}px" onkeydown={onkey}>
		{#if view === 'main'}
			{@render entry('queue_play_next', 'Play next', () => run(() => player.enqueue(targets, { next: true }), 'Playing next'))}
			{@render entry('queue_music', 'Add to Up next', () => run(() => player.enqueue(targets), 'Added to Up next'))}
			{@render entry('playlist_add', 'Add to playlist', () => {
				view = 'playlists';
				tick().then(() => el?.querySelector<HTMLElement>('[role="menuitem"]')?.focus());
			})}
			{#if picked}
				<!-- Songs picked in a list: no one artist or album. -->
			{:else if artistLink(m.item)}
				{@render entry('person', 'Go to artist', () => run(() => go(artistHref(artistLink(m.item)!))))}
			{:else}
				{@render entry('search', `More from ${m.item.artist}`, () => run(() => go(`/search?q=${encodeURIComponent(m.item.artist)}`)))}
			{/if}
			{#if !picked && albumLink(m.item) && m.item.album}
				{@render entry('album', 'Go to album', () => run(() => go(albumHref(albumLink(m.item)!))))}
			{/if}
			{#if isGroup}
				{@render entry(allLiked ? 'favorite' : 'favorite_border', allLiked ? 'Remove from Likes' : `Like all ${targets.length}`, () => run(likeAll))}
			{:else}
				{@render entry(L.isLiked(m.item) ? 'favorite' : 'favorite_border', L.isLiked(m.item) ? 'Remove from Likes' : 'Like', () => run(() => L.toggleLike(m.item)))}
			{/if}
			{#if m.download === 'saved'}
				{@render entry('delete', 'Remove from this computer', () => {
					if (picked) P.clear();
					change(() => unsaveSongs(targets), 'Removed from this computer');
				})}
			{:else if m.download === 'waiting'}
				{@render entry('close', 'Don’t save', () => {
					if (picked) P.clear();
					change(() => unsaveSongs(targets), 'Taken off Downloads');
				})}
			{:else}
				{@render entry('download', isGroup ? `Download all ${targets.length}` : 'Download', () => {
					void saveSongs(targets).then((said) => ui.toast(said), () => {});
					close();
				})}
			{/if}
			{#if picked}
				{#if allBlocked}
					{@render entry('block', 'Play these songs again', () => dontPlayPicked(false))}
				{:else}
					{@render entry('block', `Don’t play these ${targets.length} songs`, () => dontPlayPicked(true))}
				{/if}
				{#if m.playlistId !== undefined}
					{@render entry('delete', `Remove ${targets.length} from this playlist`, removePicked)}
				{/if}
			{/if}
			{#if !isGroup}
				{@const how = L.blockedOf(m.item)}
				{#if how === 'song'}
					{@render entry('block', 'Play this song again', () => dontPlay('song', false))}
				{:else if how === 'artist'}
					{@render entry('block', `Play ${m.item.artist} again`, () => dontPlay('artist', false))}
				{:else}
					{@render entry('block', 'Don’t play this song', () => dontPlay('song', true))}
					{@render entry('block', `Don’t play ${m.item.artist}`, () => dontPlay('artist', true))}
				{/if}
			{/if}
			{#if !isGroup && NF.has(m.item)}
				{@render entry('search', 'Find on your add-ons…', () => run(() => void findIt(m.item.refs[LIB_REF])))}
			{/if}
			{#if m.queueIndex !== undefined}
				{#if m.queueIndex > 0}{@render entry('arrow_upward', 'Move up', () => run(() => player.move(m.queueIndex!, m.queueIndex! - 1)))}{/if}
				{#if m.queueLength !== undefined && m.queueIndex < m.queueLength - 1}{@render entry('arrow_downward', 'Move down', () => run(() => player.move(m.queueIndex!, m.queueIndex! + 1)))}{/if}
				{@render entry('close', 'Remove from Up next', () => run(() => player.removeAt(m.queueIndex!)))}
			{/if}
			{#if m.playlistId !== undefined && m.playlistIndex !== undefined}
				{#if m.playlistIndex > 0}{@render entry('arrow_upward', 'Move up', () => run(() => library.moveInPlaylist(m.playlistId!, m.playlistIndex!, m.playlistIndex! - 1)))}{/if}
				{#if m.playlistLength !== undefined && m.playlistIndex < m.playlistLength - 1}{@render entry('arrow_downward', 'Move down', () => run(() => library.moveInPlaylist(m.playlistId!, m.playlistIndex!, m.playlistIndex! + 1)))}{/if}
				{@render entry('delete', 'Remove from this playlist', () => run(() => library.removeFromPlaylist(m.playlistId!, m.playlistIndex!)))}
			{/if}
		{:else}
			<p class="mh">Add to playlist</p>
			{#each L.playlists.filter((x) => !x.followed && !x.rules) as p (p.id)}
				{@render entry('library_music', p.name, () => change(() => library.addToPlaylist(p.id, targets), `Added to ${p.name}`))}
			{/each}
			{#if naming}
				<form class="mnew" onsubmit={(e) => { e.preventDefault(); createAndAdd(); }}>
					<!-- svelte-ignore a11y_autofocus -->
					<input aria-label="Playlist name" placeholder="Playlist name" bind:value={name} autofocus maxlength="80" />
					<button type="submit" disabled={!name.trim()}>Create</button>
				</form>
			{:else}
				{@render entry('add', 'New playlist', () => (naming = true))}
			{/if}
		{/if}
	</div>
{/if}
