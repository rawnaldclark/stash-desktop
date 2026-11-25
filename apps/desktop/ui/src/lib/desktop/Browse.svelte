<!-- Library › Genres and Library › Folders: the library seen by genre, or by where its files are. Each row says how
     many songs it holds and opens the song table showing just those. The rows are the sidebar's and the search page's
     (a tile, a name, a line under it): genres side by side, folders as their tree. -->
<script lang="ts">
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { fold } from '../identity';
	import { L } from '../ui/state.svelte';
	import { folderRows } from './browse';
	import { folders as musicFolders } from './folders.svelte';

	let { by, q = '', here = false }: { by: 'genre' | 'folder'; q?: string; here?: boolean } = $props();
	const needle = $derived(fold(q));

	let genres = $state.raw<core.GenreRow[] | null>(null);
	let places = $state.raw<{ rows: core.PlaceRow[]; roots: string[] } | null>(null);
	let failure = $state<string | null>(null);

	$effect(() => {
		void L.changed;
		const what = by;
		let gone = false;
		const fail = (e: unknown) => {
			if (!gone) failure = core.said(e);
		};
		if (what === 'genre')
			core.genres(here).then((g) => {
				if (!gone) genres = g;
			}, fail);
		else
			core.places().then((p) => {
				if (!gone) places = p;
			}, fail);
		return () => {
			gone = true;
		};
	});

	const genreRows = $derived((genres ?? []).filter((g) => !needle || fold(g.genre ?? 'No genre').includes(needle)));
	const folders = $derived(places ? folderRows(places.rows, places.roots).filter((f) => !needle || fold(f.path).includes(needle)) : []);
	const songs = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'song' : 'songs'}`;
</script>

{#if failure}
	<p class="s-hint" role="alert">{failure}</p>
{:else if by === 'genre'}
	{#if genres === null}
		<p class="s-hint">Reading the genres…</p>
	{:else if !genreRows.length}
		<p class="s-hint">{needle ? `No genre matches “${q.trim()}”.` : 'No songs yet.'}</p>
	{:else}
		<ul class="br br-grid">
			{#each genreRows as g (g.genre ?? '')}
				<li>
					<a class="br-r" href="/library?tab=songs&genre={encodeURIComponent(g.genre ?? '')}">
						<span class="tile fold-tile br-tile"><Icon name="sell" /></span>
						<span class="br-t"><b class:none={g.genre === null}>{g.genre ?? 'No genre'}</b><i>{songs(g.songs)}</i></span>
					</a>
				</li>
			{/each}
		</ul>
	{/if}
{:else if places === null}
	<p class="s-hint">Reading the folders…</p>
{:else if !folders.length}
	{#if needle}
		<p class="s-hint">No folder matches “{q.trim()}”.</p>
	{:else}
		<p class="s-hint">No music files on this computer yet. Add the folder your music is in, and its folders show up here.</p>
		<button type="button" class="btn ghost" onclick={() => void musicFolders.add()}><Icon name="create_new_folder" />Add a music folder…</button>
	{/if}
{:else}
	<ul class="br">
		{#each folders as f (f.path)}
			<li>
				<a class="br-r" href="/library?tab=songs&folder={encodeURIComponent(f.path)}" title={f.path} style:padding-left="{6 + f.depth * 24}px">
					<span class="tile fold-tile br-tile"><Icon name="folder" /></span>
					<span class="br-t"><b>{f.name}</b><i>{songs(f.songs)}{f.below > f.songs ? ` · ${f.below.toLocaleString()} with the folders inside` : ''}</i></span>
				</a>
			</li>
		{/each}
	</ul>
{/if}

<style>
	.br { display: grid; grid-template-columns: minmax(0, 1fr); gap: 2px; margin: 4px -6px 0; padding: 0; list-style: none; }
	.br-grid { grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 2px 8px; }
	.br-r { display: flex; align-items: center; gap: 12px; height: 56px; padding: 6px; border-radius: 8px; color: var(--wp-text); text-decoration: none; transition: background-color 150ms var(--ease-standard); }
	.br-tile { width: 44px; height: 44px; border-radius: 8px; flex: none; }
	.br-tile :global(.ic) { width: 22px; height: 22px; }
	.br-t { display: grid; grid-template-columns: minmax(0, 1fr); min-width: 0; }
	.br-t b { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 600 14px/1.3 var(--font-body); color: var(--wp-text); }
	.br-t b.none { color: var(--wp-text-2); font-style: italic; font-weight: 500; }
	.br-t i { overflow: clip; text-overflow: ellipsis; white-space: nowrap; font: 400 12px/1.3 var(--font-body); color: var(--wp-text-2); font-variant-numeric: tabular-nums; }
	@media (hover: hover) and (pointer: fine) { .br-r:hover { background: var(--wp-hover); } }
	.br-r:focus-visible { outline: 2px solid var(--acc); outline-offset: -2px; }
</style>
