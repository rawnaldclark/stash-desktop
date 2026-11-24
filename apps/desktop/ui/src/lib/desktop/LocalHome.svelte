<!-- Home, when Stash runs for local files only: what is on this computer. How much there is, the albums that came
     lately, what was played lately and what is played most, the genres, and the listener's playlists. Nothing here
     needs a connection. -->
<script lang="ts">
	import { itemOfRow, player } from '../app';
	import type { LibraryItem } from '../api';
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { songs } from '../ui/format';
	import { shelf } from '../ui/shelf';
	import { L } from '../ui/state.svelte';
	import { statTiles } from './local-home';

	let home = $state.raw<core.LocalHome | null>(null);
	let failure = $state<string | null>(null);

	$effect(() => {
		void L.changed;
		let gone = false;
		core.localHome().then(
			(h) => {
				if (!gone) {
					home = h;
					failure = null;
				}
			},
			(e) => {
				if (!gone) failure = core.said(e);
			}
		);
		return () => {
			gone = true;
		};
	});

	/** The files played lately, each once, newest first. */
	const recent = $derived.by(() => {
		const seen = new Set<string>();
		const out: LibraryItem[] = [];
		for (const h of L.history) {
			if (!h.item.saved || seen.has(h.item.key)) continue;
			seen.add(h.item.key);
			out.push(h.item);
			if (out.length === 12) break;
		}
		return out;
	});
	const most = $derived((home?.played ?? []).map(itemOfRow));
	const art = (a: core.AlbumRow) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
	const play = (items: LibraryItem[], i: number, from: string) => void player.playItems(items, i, { from });
</script>

{#if failure && !home}
	<p class="s-hint" role="alert">{failure}</p>
{:else if !home}
	<div class="view"></div>
{:else if home.stats.songs === 0}
	<section class="empty" aria-label="Get started">
		<p class="empty-line">Add a music folder and your files show up here.</p>
		<a class="btn" href="/settings#library">Open Settings</a>
	</section>
{:else}
	<div class="view view-in">
		<h1 class="lh-h">Your music</h1>
		<ul class="lh-stats" aria-label="On this computer">
			{#each statTiles(home.stats) as t (t.label)}
				<li>
					{#if t.href}
						<a class="lh-stat" href={t.href}><b>{t.value}</b><span>{t.label}</span></a>
					{:else}
						<div class="lh-stat"><b>{t.value}</b><span>{t.label}</span></div>
					{/if}
				</li>
			{/each}
		</ul>

		{#if home.added.length}
			<h2 class="vh2">Recently added <a class="vh2-more" href="/library?tab=songs">Show all</a></h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each home.added as a (a.id)}
						<Card title={a.title} sub={[a.year, a.artist].filter(Boolean).join(' · ')} artwork={art(a)} href="/album/library/{a.id}" />
					{/each}
				</div>
			</div>
		{/if}

		{#if recent.length}
			<h2 class="vh2">Recently played</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each recent as it, i (it.key)}
						<Card title={it.title} sub={it.artist} artwork={it.artwork} label="Play {it.title} by {it.artist}" song={it} onclick={() => play(recent, i, 'Recently played')} />
					{/each}
				</div>
			</div>
		{/if}

		{#if most.length}
			<h2 class="vh2">Most played</h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each most as it, i (it.key)}
						<Card title={it.title} sub={it.artist} artwork={it.artwork} label="Play {it.title} by {it.artist}" song={it} onclick={() => play(most, i, 'Most played')} />
					{/each}
				</div>
			</div>
		{/if}

		{#if home.genres.length}
			<h2 class="vh2">Genres <a class="vh2-more" href="/library?tab=genres">Show all</a></h2>
			<div class="lh-genres">
				{#each home.genres as g (g.genre)}
					<a class="chip" href="/library?tab=songs&genre={encodeURIComponent(g.genre)}">{g.genre} <span class="lh-n">{g.songs.toLocaleString()}</span></a>
				{/each}
			</div>
		{/if}

		{#if L.playlists.length}
			<h2 class="vh2">Your playlists <a class="vh2-more" href="/library?tab=playlists">Show all</a></h2>
			<div class="shelf-w" use:shelf>
				<div class="shelf">
					{#each L.playlists.slice(0, 8) as p (p.id)}
						<Card title={p.name} sub={songs(p.items.length)} items={p.items} href="/playlist/{p.id}" kept={p.download} />
					{/each}
				</div>
			</div>
		{/if}
	</div>
{/if}

<style>
	.lh-h { margin: 0 0 14px; font: 700 28px/1.2 var(--font-display, var(--font-body)); color: var(--wp-text); }
	.lh-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(118px, 1fr)); gap: 10px; margin: 0 0 26px; padding: 0; list-style: none; }
	.lh-stat { display: grid; gap: 4px; padding: 14px 16px; border-radius: 14px; background: var(--wp-hover); border: 1px solid var(--wp-line); color: var(--wp-text); text-decoration: none; }
	.lh-stat b { font: 700 20px/1.15 var(--font-display, var(--font-body)); font-variant-numeric: tabular-nums; }
	.lh-stat span { color: var(--wp-text-2); font: 500 13px/1.2 var(--font-body); }
	.lh-genres { display: flex; flex-wrap: wrap; gap: 8px; margin: 0 0 26px; }
	.lh-n { margin-left: 4px; color: var(--wp-text-2); font-variant-numeric: tabular-nums; }
	@media (hover: hover) and (pointer: fine) { a.lh-stat:hover { border-color: var(--acc); } }
	a.lh-stat:focus-visible { outline: 2px solid var(--acc); outline-offset: 2px; }
</style>
