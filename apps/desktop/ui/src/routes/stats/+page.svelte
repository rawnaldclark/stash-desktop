<!-- Library › Statistics: the files on this computer in numbers (how much, in what quality and format, from which
     decades, in which genres, by whom, how complete their tags are) and what has been listened to. One hue for every
     bar; every value is printed beside its bar or named on hover and focus. -->
<script lang="ts">
	import * as core from '../../lib/core';
	import { L, ui } from '../../lib/ui/state.svelte';
	import BarList from '../../lib/desktop/stats/BarList.svelte';
	import Columns from '../../lib/desktop/stats/Columns.svelte';
	import { diskSize } from '../../lib/desktop/local-home';
	import { hours, monthNames, percent, playsWord, rowHref, songsWord } from '../../lib/desktop/stats/stats';

	let s = $state.raw<core.LibraryStats | null>(null);
	let failure = $state<string | null>(null);
	$effect(() => {
		void L.changed;
		let gone = false;
		core.libraryStats().then(
			(x) => {
				if (!gone) {
					s = x;
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

	const t = $derived(s?.totals);
	const tiles = $derived(
		t
			? [
					{ value: t.songs.toLocaleString(), label: t.songs === 1 ? 'song' : 'songs' },
					{ value: hours(t.duration_ms), label: 'of music' },
					{ value: diskSize(t.bytes), label: 'on disk' },
					{ value: percent(t.lossless, t.songs), label: 'lossless' },
					{ value: t.hi_res.toLocaleString(), label: 'in hi-res' },
					{ value: t.plays.toLocaleString(), label: `plays · ${hours(t.played_ms)}` }
				]
			: []
	);
	const formatRows = $derived((s?.formats ?? []).map((f) => ({ label: f.label, value: f.value, figure: `${songsWord(f.value)} · ${diskSize(f.bytes ?? 0)}` })));
	const qualityRows = $derived((s?.quality ?? []).map((q) => ({ label: q.label, value: q.value, figure: `${q.value.toLocaleString()} · ${percent(q.value, t?.songs ?? 0)}` })));
	const genreRows = $derived(
		(s?.genres ?? []).map((g) => ({ label: g.label, value: g.value, figure: g.value.toLocaleString(), href: rowHref('genres', g), muted: g.label === 'Other genres' }))
	);
	const decadePoints = $derived((s?.decades ?? []).filter((d) => d.key).map((d) => ({ short: d.label, full: d.label, value: d.value })));
	const noYear = $derived(s?.decades.find((d) => !d.key)?.value ?? 0);
	const artistRows = (bars: core.StatBar[], word: (n: number) => string) => bars.map((a) => ({ label: a.label, value: a.value, figure: word(a.value), href: rowHref('artists', a) }));
	const monthPoints = $derived((s?.months ?? []).map((m) => ({ ...monthNames(m.label), value: m.value })));
	const tagRows = $derived((s?.tags ?? []).map((g) => ({ label: g.label, value: g.have, figure: `${percent(g.have, g.of)} · ${g.have.toLocaleString()} of ${g.of.toLocaleString()}` })));
	const tagsMissing = $derived((s?.tags ?? []).slice(0, 4).some((g) => g.have < g.of));
</script>

<div class="view view-in st">
	<h1 class="st-h">Your library in numbers</h1>
	<p class="st-sub">The music files on this computer, and everything you have played in Stash.</p>

	{#if failure && !s}
		<p class="s-hint" role="alert">{failure}</p>
	{:else if !s || !t}
		<p class="s-hint">Counting…</p>
	{:else if t.songs === 0 && t.plays === 0}
		<p class="s-hint">No music files on this computer yet. Add a music folder in Settings › Library and its numbers show up here.</p>
	{:else}
		<ul class="st-tiles" aria-label="In all">
			{#each tiles as tile (tile.label)}
				<li><b>{tile.value}</b><span>{tile.label}</span></li>
			{/each}
		</ul>

		<div class="st-grid">
			{#if t.songs}
				<section class="st-card">
					<h2>Sound quality</h2>
					<BarList label="Sound quality" rows={qualityRows} />
				</section>
				<section class="st-card">
					<h2>Formats</h2>
					<BarList label="Formats" rows={formatRows} />
				</section>
			{/if}

			{#if decadePoints.length}
				<section class="st-card wide">
					<h2>By decade</h2>
					<Columns label="Songs by decade" points={decadePoints} unit={songsWord} />
					{#if noYear}<p class="st-note">{songsWord(noYear)} without a year are not shown.</p>{/if}
				</section>
			{/if}

			{#if genreRows.length || s.no_genre}
				<section class="st-card">
					<h2>Genres</h2>
					{#if genreRows.length}<BarList label="Songs by genre" rows={genreRows} />{/if}
					{#if s.no_genre}
						<p class="st-note"><a href="/library?tab=songs&genre=">{songsWord(s.no_genre)} without a genre</a> are not shown.</p>
					{/if}
				</section>
			{/if}

			{#if s.artists_by_songs.length}
				<section class="st-card">
					<h2>Artists with the most songs</h2>
					<BarList label="Artists with the most songs" rows={artistRows(s.artists_by_songs, songsWord)} />
				</section>
			{/if}

			{#if t.plays}
				<section class="st-card wide">
					<h2>Listening, the last twelve months</h2>
					<Columns label="Plays a month" points={monthPoints} unit={playsWord} />
				</section>
				<section class="st-card">
					<h2>Most played artists</h2>
					<BarList label="Most played artists" rows={artistRows(s.artists_by_plays, playsWord)} />
				</section>
				<section class="st-card">
					<h2>Most played songs</h2>
					<BarList label="Most played songs" rows={s.most_played.map((m) => ({ label: m.label, detail: m.detail, value: m.value, figure: playsWord(m.value) }))} />
				</section>
			{/if}

			{#if t.songs}
				<section class="st-card wide">
					<h2>
						How complete your tags are
						{#if tagsMissing}<button type="button" class="vh2-more st-fill" onclick={() => (ui.filling = true)}>Fill in missing tags…</button>{/if}
					</h2>
					<BarList label="Files with each tag" rows={tagRows} whole={t.songs} />
				</section>
			{/if}
		</div>
	{/if}
</div>

<style>
	/* One hue for every bar, fixed rather than the accent that follows the song playing: #7c3aed on the light
	   surface, #8b5cf6 on the dark one (both checked for 3:1 against their surface). */
	.st { --st-bar: #8b5cf6; --st-bar-on: #a78bfa; --st-quiet: color-mix(in oklab, #8b5cf6 35%, transparent); }
	@media (prefers-color-scheme: light) {
		:global(:root:not([data-theme='dark'])) .st { --st-bar: #7c3aed; --st-bar-on: #6d28d9; --st-quiet: color-mix(in oklab, #7c3aed 30%, transparent); }
	}
	:global(:root[data-theme='light']) .st { --st-bar: #7c3aed; --st-bar-on: #6d28d9; --st-quiet: color-mix(in oklab, #7c3aed 30%, transparent); }

	.st-h { margin: 0; font: 700 28px/1.2 var(--font-display, var(--font-body)); color: var(--wp-text); }
	.st-sub { margin: 6px 0 18px; color: var(--wp-text-2); font: 400 14px/1.4 var(--font-body); }
	.st-tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(112px, 1fr)); gap: 10px; margin: 0 0 22px; padding: 0; list-style: none; }
	.st-tiles li { display: grid; gap: 4px; padding: 14px 16px; border-radius: 14px; background: var(--wp-hover); border: 1px solid var(--wp-line); }
	.st-tiles b { font: 700 20px/1.15 var(--font-display, var(--font-body)); color: var(--wp-text); font-variant-numeric: tabular-nums; }
	.st-tiles span { color: var(--wp-text-2); font: 500 13px/1.25 var(--font-body); }
	.st-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(340px, 1fr)); gap: 14px; }
	.st-card { min-width: 0; padding: 16px 18px 14px; border-radius: 16px; border: 1px solid var(--wp-line-2); }
	.st-card.wide { grid-column: 1 / -1; }
	.st-card h2 { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin: 0 0 10px; font: 600 15px/1.3 var(--font-body); color: var(--wp-text); }
	.st-fill { padding: 0; background: none; border: 0; cursor: pointer; }
	.st-note a { color: inherit; text-decoration: underline; text-underline-offset: 2px; }
	.st-note { margin: 8px 0 0; color: var(--wp-text-2); font: 400 12.5px/1.4 var(--font-body); }
</style>
