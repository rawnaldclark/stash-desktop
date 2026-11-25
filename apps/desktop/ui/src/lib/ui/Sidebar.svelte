<!-- Sidebar (L: labelled, M: a 72 px rail). Navigation, then your library: Liked songs, the folders (each folded shut or
     open, its playlists under it), then the playlists in none. A playlist is carried onto a folder with the mouse (or out
     of one, onto "Your library" or the playlists in none); right-click a playlist or a folder for its menu. -->
<script lang="ts">
	import { page } from '$app/state';
	import Logo from '../brand/Logo.svelte';
	import Icon from './Icon.svelte';
	import NavIcon, { type NavIconName } from './NavIcon.svelte';
	import Tile from './Tile.svelte';
	import { library } from '../app';
	import type { Playlist } from '../api';
	import { carry } from './carry';
	import { atEvent, folderMenu, moveTo, newMenu, playlistMenu, playlistsWord, under } from '../desktop/playlist-folders';
	import { L, ui } from './state.svelte';
	import { songs } from './format';
	import { isCollection } from '../desktop/library-sections';
	import { NF } from '../desktop/unfound.svelte';
	import { T } from '../desktop/youtube.svelte';

	// As Stash for Android's tab bar has them (Sync is the phone's own).
	const nav: [string, string, NavIconName][] = [
		['/', 'Home', 'home'],
		['/library', 'Library', 'library'],
		['/search', 'Search', 'search'],
		['/settings', 'Settings', 'settings']
	];
	/** The collections are rows of their own below (Liked songs, History…): on theirs, Library is not lit too. */
	const onTab = (t: string) => page.url.pathname === '/library' && page.url.searchParams.get('tab') === t;
	const here = (href: string) =>
		href === '/' ? page.url.pathname === '/' : href === '/library' ? page.url.pathname.startsWith('/library') && !isCollection(page.url.searchParams.get('tab')) : page.url.pathname.startsWith(href);
	const folderIds = $derived(new Set(L.folders.map((f) => f.id)));
	/** The playlists in no folder (or in one that is gone). */
	const loose = $derived(L.playlists.filter((p) => !p.folder || !folderIds.has(p.folder)));
	const inside = (id: number) => L.playlists.filter((p) => p.folder === id);
	/** A playlist carried onto a folder, or out of one. */
	const drop = (id: string, folder: number | null) => {
		const p = L.playlists.find((x) => x.id === id);
		if (p) moveTo(p, folder, false);
	};
</script>

{#snippet row(p: Playlist)}
	<a class="sb-rec" class:on={page.url.pathname === `/playlist/${p.id}`} href="/playlist/{p.id}" title={p.name} use:carry={{ id: p.id, name: p.name, drop }} oncontextmenu={(e) => { e.preventDefault(); playlistMenu(p, atEvent(e)); }}>
		<Tile items={p.items} seed={p.name} /><span class="sb-t"><b>{p.name}</b><i>{#if p.download}<Icon name="download" class="kept-mk" label="On this computer" />{/if}{p.rules ? 'Smart playlist' : p.followsTube ? 'YouTube Music' : p.followed ? 'Mix' : 'Playlist'} · {songs(p.items.length)}</i></span>
	</a>
{/snippet}

<aside class="side" aria-label="Sidebar">
	<div class="sb-brand">
		<Logo size="sm" />
		<button type="button" class="sb-fold-side" aria-label={ui.sideFolded ? 'Show the sidebar' : 'Fold the sidebar to icons'} title={ui.sideFolded ? 'Show the sidebar' : 'Fold the sidebar'} aria-pressed={ui.sideFolded} onclick={() => ui.foldSide(!ui.sideFolded)}><Icon name={ui.sideFolded ? 'chevron_right' : 'chevron_left'} /></button>
	</div>
	<nav class="sb-nav" aria-label="Main">
		{#each nav as [href, name, icon] (href)}
			<a class="nav-i" class:on={here(href)} {href} title={name} aria-current={here(href) ? 'page' : undefined}><NavIcon name={icon} /><span>{name}</span></a>
		{/each}
	</nav>
	<div class="sb-sec" data-drop-folder=""><span class="caps">Your library</span><button type="button" class="sb-new" aria-label="New playlist or folder" title="New playlist or folder" aria-haspopup="menu" onclick={(e) => newMenu(under(e.currentTarget))}><Icon name="add" /></button></div>
	<nav class="sb-list" aria-label="Playlists">
		<a class="sb-rec" class:on={onTab('likes')} href="/library?tab=likes" title="Liked songs">
			<Tile liked /><span class="sb-t"><b>Liked songs</b><i>{#if L.likedKept}<Icon name="download" class="kept-mk" label="On this computer" />{/if}{songs(L.likes.length)}</i></span>
		</a>
		<!-- The collections, each a page of its own (Library's chips are the ways to look through everything). -->
		<a class="sb-rec" class:on={onTab('history')} href="/library?tab=history" title="History">
			<span class="tile fold-tile"><Icon name="history" /></span><span class="sb-t"><b>History</b><i>What you played</i></span>
		</a>
		{#if !ui.local}
			<a class="sb-rec" class:on={onTab('downloads')} href="/library?tab=downloads" title="Downloads">
				<span class="tile fold-tile"><Icon name="download" /></span><span class="sb-t"><b>Downloads</b><i>Saved for offline</i></span>
			</a>
		{/if}
		<a class="sb-rec" class:on={page.url.pathname === '/stats'} href="/stats" title="Statistics">
			<span class="tile fold-tile"><Icon name="leaderboard" /></span><span class="sb-t"><b>Statistics</b><i>Your numbers</i></span>
		</a>
		{#if !ui.local && (NF.open > 0 || onTab('notfound'))}
			<a class="sb-rec" class:on={onTab('notfound')} href="/library?tab=notfound" title="Not found">
				<span class="tile fold-tile"><Icon name="search" /></span><span class="sb-t"><b>Not found</b><i>{NF.open > 0 ? `${songs(NF.open)} to find` : 'Nothing to find'}</i></span>
			</a>
		{/if}
		{#if !ui.local && ((T.on && T.signedIn) || onTab('youtube'))}
			<a class="sb-rec" class:on={onTab('youtube')} href="/library?tab=youtube" title="YouTube Music">
				<span class="tile fold-tile"><Icon name="library_music" /></span><span class="sb-t"><b>YouTube Music</b><i>Your library there</i></span>
			</a>
		{/if}
		{#each L.folders as f (f.id)}
			{@const its = inside(f.id)}
			<div class="sb-folder" data-drop-folder={f.id}>
				<button type="button" class="sb-rec sb-fold" aria-expanded={!f.collapsed} title={f.name} onclick={() => void library.foldFolder(f.id, !f.collapsed)} oncontextmenu={(e) => { e.preventDefault(); folderMenu(f, atEvent(e)); }}>
					<span class="tile fold-tile"><Icon name={f.collapsed ? 'folder' : 'folder_open'} /></span>
					<span class="sb-t"><b>{f.name}</b><i>{playlistsWord(its.length)}</i></span>
					<Icon name="expand_more" class="sb-chev" />
				</button>
				{#if !f.collapsed}
					{#each its as p (p.id)}{@render row(p)}{/each}
				{/if}
			</div>
		{/each}
		<div class="sb-loose" data-drop-folder="">
			{#each loose as p (p.id)}{@render row(p)}{/each}
		</div>
	</nav>
</aside>
