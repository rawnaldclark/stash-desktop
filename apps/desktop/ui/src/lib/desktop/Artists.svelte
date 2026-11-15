<!-- Library › Artists: the library's artists as the web player's round artist cards, read from the core a page at a time as the grid
     is scrolled. A card opens the artist's page. -->
<script lang="ts">
	import * as core from '../core';
	import Card from '../ui/Card.svelte';
	import { songs } from '../ui/format';
	import { more } from './more';

	const PAGE = 120;
	let rows = $state.raw<core.ArtistRow[]>([]);
	let done = $state(false);
	let failure = $state<string | null>(null);

	async function load(): Promise<boolean> {
		if (done) return false;
		try {
			const page = await core.artistsAt(rows.length, PAGE);
			rows = [...rows, ...page];
			done = page.length < PAGE;
			failure = null;
			return !done;
		} catch (e) {
			failure = core.said(e);
			return false;
		}
	}
	const art = (a: core.ArtistRow) => {
		const url = core.artUrl(a.art);
		return url ? [{ url }] : undefined;
	};
	/** Two letters for an artist with no picture ("Vince Staples" -> "VS"). */
	const initials = (name: string) => name.replace(/[([{].*?[)\]}]/g, ' ').split(/\s+/).filter((w) => /\p{L}/u.test(w)).slice(0, 2).map((w) => w.match(/\p{L}/u)![0].toUpperCase()).join('');
	const sub = (a: core.ArtistRow) => [a.albums ? (a.albums === 1 ? '1 album' : `${a.albums} albums`) : '', songs(a.tracks)].filter(Boolean).join(' · ');
</script>

{#if rows.length}
	<div class="cards cards-artists">
		{#each rows as a (a.id)}
			<Card title={a.name} sub={sub(a)} artwork={art(a)} href="/artist/library/{a.id}" ini={initials(a.name)} />
		{/each}
	</div>
{:else if done}
	<p class="s-hint">No artists yet. Artists show up here once your library has songs.</p>
{/if}
{#if failure}
	<p class="s-hint" role="alert">{failure} <button type="button" class="lnk" onclick={() => void load()}>Try again</button></p>
{:else if !done}
	<div class="more" use:more={load} aria-hidden="true"></div>
{/if}

<style>
	.more { height: 1px; }
	/* the web player's artist cards (its Artists shelf): centred, the picture a circle */
	.cards-artists :global(.card) { text-align: center; }
	.cards-artists :global(.card > .cv) { border-radius: 50%; }
</style>
