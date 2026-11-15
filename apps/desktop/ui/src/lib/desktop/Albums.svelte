<!-- Library › Albums: the library's albums as the web player's cards, read from the core a page at a time as the grid is scrolled.
     A card opens the album's page. -->
<script lang="ts">
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { more } from './more';

	const PAGE = 120;
	let rows = $state.raw<core.AlbumRow[]>([]);
	let done = $state(false);
	let failure = $state<string | null>(null);

	async function load(): Promise<boolean> {
		if (done) return false;
		try {
			const page = await core.albumsAt(rows.length, PAGE);
			rows = [...rows, ...page];
			done = page.length < PAGE;
			failure = null;
			return !done;
		} catch (e) {
			failure = core.said(e);
			return false;
		}
	}
	const art = (a: core.AlbumRow) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
</script>

{#if rows.length}
	<div class="cards">
		{#each rows as a (a.id)}
			<Card title={a.title} sub={[a.year, a.artist].filter(Boolean).join(' · ')} artwork={art(a)} href="/album/library/{a.id}" />
		{/each}
	</div>
{:else if done}
	<p class="s-hint">No albums yet. Albums show up here once your library has songs.</p>
{/if}
{#if failure}
	<p class="s-hint" role="alert">{failure} <button type="button" class="lnk" onclick={() => void load()}>Try again</button></p>
{:else if !done}
	<div class="more" use:more={load} aria-hidden="true"></div>
{/if}

<style>
	.more { height: 1px; }
</style>
