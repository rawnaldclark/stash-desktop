<!-- Sidebar (L: labelled, M: a 72 px rail). Navigation, then your playlists. -->
<script lang="ts">
	import { page } from '$app/state';
	import Logo from '../brand/Logo.svelte';
	import Icon from './Icon.svelte';
	import Tile from './Tile.svelte';
	import { L } from './state.svelte';
	import { songs } from './format';
	import type { IconName } from './icons';

	const nav: [string, string, IconName][] = [
		['/', 'Home', 'home'],
		['/search', 'Search', 'search'],
		['/library', 'Library', 'library_music']
	];
	const here = (href: string) => (href === '/' ? page.url.pathname === '/' : page.url.pathname.startsWith(href));
</script>

<aside class="side" aria-label="Sidebar">
	<div class="sb-brand"><Logo size="sm" /></div>
	<nav class="sb-nav" aria-label="Main">
		{#each nav as [href, name, icon] (href)}
			<a class="nav-i" class:on={here(href)} {href} title={name} aria-current={here(href) ? 'page' : undefined}><Icon name={icon} /><span>{name}</span></a>
		{/each}
	</nav>
	<div class="sb-sec"><span class="caps">Your library</span></div>
	<nav class="sb-list" aria-label="Playlists">
		<a class="sb-rec" class:on={page.url.pathname === '/library' && page.url.searchParams.get('tab') === 'likes'} href="/library?tab=likes" title="Liked songs">
			<Tile liked /><span class="sb-t"><b>Liked songs</b><i>{songs(L.likes.length)}</i></span>
		</a>
		{#each L.playlists as p (p.id)}
			<a class="sb-rec" class:on={page.url.pathname === `/playlist/${p.id}`} href="/playlist/{p.id}" title={p.name}>
				<Tile items={p.items} seed={p.name} /><span class="sb-t"><b>{p.name}</b><i>{p.followed ? 'Mix' : 'Playlist'} · {songs(p.items.length)}</i></span>
			</a>
		{/each}
	</nav>
</aside>
