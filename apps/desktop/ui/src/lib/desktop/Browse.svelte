<!-- Library › Genres and Library › Folders: the library seen by genre, or by where its files are. Each row says how
     many songs it holds and opens the song table showing just those. -->
<script lang="ts">
	import * as core from '../core';
	import Icon from '../ui/Icon.svelte';
	import { fold } from '../identity';
	import { L, ui } from '../ui/state.svelte';
	import { folderRows } from './browse';

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
	const most = $derived(Math.max(1, ...genreRows.map((g) => g.songs)));
</script>

{#if failure}
	<p class="s-hint" role="alert">{failure}</p>
{:else if by === 'genre'}
	{#if genres === null}
		<p class="s-hint">Reading the genres…</p>
	{:else if !genreRows.length}
		<p class="s-hint">{needle ? `No genre matches “${q.trim()}”.` : 'No songs yet.'}</p>
	{:else}
		<ul class="br">
			{#each genreRows as g (g.genre ?? '')}
				<li>
					<a class="br-r" href="/library?tab=songs&genre={encodeURIComponent(g.genre ?? '')}">
						<span class="br-n" class:none={g.genre === null}>{g.genre ?? 'No genre'}</span>
						<span class="br-bar" aria-hidden="true"><i style:width="{Math.max(2, (g.songs / most) * 100)}%"></i></span>
						<span class="br-c">{songs(g.songs)}</span>
					</a>
				</li>
			{/each}
		</ul>
		{#if here && genres.some((g) => g.genre === null) && !needle}
			<p class="s-hint br-fill">Songs with no genre say none in their tags. <button type="button" class="br-link" onclick={() => (ui.filling = true)}>Fill in missing tags…</button></p>
		{/if}
	{/if}
{:else if places === null}
	<p class="s-hint">Reading the folders…</p>
{:else if !folders.length}
	<p class="s-hint">{needle ? `No folder matches “${q.trim()}”.` : 'No music files on this computer yet. Add a music folder in Settings › Library.'}</p>
{:else}
	<ul class="br">
		{#each folders as f (f.path)}
			<li>
				<a class="br-r" href="/library?tab=songs&folder={encodeURIComponent(f.path)}" title={f.path} style:padding-left="{10 + f.depth * 18}px">
					<Icon name="folder" />
					<span class="br-n">{f.name}</span>
					<span class="br-c">{songs(f.songs)}{f.below > f.songs ? ` · ${f.below.toLocaleString()} with the folders inside` : ''}</span>
				</a>
			</li>
		{/each}
	</ul>
{/if}

<style>
	.br { margin: 8px -8px 0; padding: 0; list-style: none; }
	.br-r { display: flex; align-items: center; gap: 12px; height: 40px; padding: 0 10px; border-radius: 8px; color: var(--wp-text); text-decoration: none; font: 400 14.5px/1 var(--font-body); }
	.br-r :global(svg) { width: 18px; height: 18px; flex: none; color: var(--wp-text-2); }
	.br-n { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
	.br-n.none { color: var(--wp-text-2); font-weight: 400; font-style: italic; }
	.br-bar { flex: 1 1 120px; height: 6px; border-radius: 3px; background: var(--wp-hover); overflow: hidden; }
	.br-bar i { display: block; height: 100%; border-radius: 3px; background: var(--acc); opacity: .7; }
	.br-c { margin-left: auto; flex: none; color: var(--wp-text-2); font-size: 13px; font-variant-numeric: tabular-nums; white-space: nowrap; }
	.br-bar + .br-c { margin-left: 0; min-width: 84px; text-align: right; }
	@media (hover: hover) and (pointer: fine) { .br-r:hover { background: var(--wp-hover); } }
	.br-fill { margin-top: 14px; }
	.br-link { padding: 0; color: var(--acc-text); font: inherit; font-weight: 600; }
	.br-r:focus-visible { outline: 2px solid var(--acc); outline-offset: -2px; }
</style>
