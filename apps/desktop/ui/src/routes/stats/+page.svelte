<!-- Library › Statistics: the files on this computer in numbers (how much, in what quality and format, from which
     decades, in which genres, by whom, how complete their tags are) and what has been listened to: the songs and the
     artists played most, and the song of each of the last twelve months. Laid out as the cards of Namida's "Your year"
     (StoryCard.svelte), with their pictures, in the album's colour; every value is printed beside its bar or named on
     hover and focus. -->
<script lang="ts">
	import * as core from '../../lib/core';
	import { player } from '../../lib/app';
	import { L, ui } from '../../lib/ui/state.svelte';
	import BarList from '../../lib/desktop/stats/BarList.svelte';
	import Columns from '../../lib/desktop/stats/Columns.svelte';
	import MonthGrid from '../../lib/desktop/stats/MonthGrid.svelte';
	import StoryCard from '../../lib/desktop/stats/StoryCard.svelte';
	import TopList, { type TopEntry } from '../../lib/desktop/stats/TopList.svelte';
	import { diskSize } from '../../lib/desktop/local-home';
	import { fromFirstPlay, percent, playsWord, rowHref, songsWord, wholeHours } from '../../lib/desktop/stats/stats';
	import { folders } from '../../lib/desktop/folders.svelte';
	import Icon from '../../lib/ui/Icon.svelte';

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
	/** The big numbers, each its value over its words (Namida's big stats). */
	const big = $derived(
		t
			? [
					...(t.songs
						? [
								{ value: t.songs.toLocaleString(), label: t.songs === 1 ? 'song' : 'songs' },
								{ value: wholeHours(t.duration_ms), label: 'hours of music' },
								{ value: diskSize(t.bytes), label: 'on disk' },
								{ value: percent(t.lossless, t.songs), label: 'lossless' },
								{ value: t.hi_res.toLocaleString(), label: 'in hi-res' }
							]
						: []),
					...(t.plays
						? [
								{ value: t.plays.toLocaleString(), label: t.plays === 1 ? 'play' : 'plays' },
								{ value: wholeHours(t.played_ms), label: 'hours played' }
							]
						: [])
				]
			: []
	);
	/** Hi-res, CD quality, lossy: one bar in three steps of the album's colour, the best the strongest. */
	const quality = $derived((s?.quality ?? []).map((q, i) => ({ ...q, step: i, share: percent(q.value, t?.songs ?? 0) })));

	const songsPlayed = $derived((s?.most_played ?? []).filter((m) => m.key));
	const topSongs = $derived<TopEntry[]>(songsPlayed.map((m) => ({ label: m.label, detail: m.detail, value: m.value, art: m.art })));
	const topArtists = $derived<TopEntry[]>((s?.artists_by_plays ?? []).map((a) => ({ label: a.label, value: a.value, art: a.art, href: rowHref('artists', a) })));
	const playTop = (i: number) => void player.playRows(songsPlayed.map((m) => Number(m.key)), i, { from: 'Most played' });
	const months = $derived(fromFirstPlay(s?.months ?? []));
	const yearPlays = $derived(months.reduce((sum, m) => sum + m.plays, 0));

	const formatRows = $derived((s?.formats ?? []).map((f) => ({ label: f.label, value: f.value, figure: `${songsWord(f.value)} · ${diskSize(f.bytes ?? 0)}` })));
	const genreRows = $derived(
		(s?.genres ?? []).map((g) => ({ label: g.label, value: g.value, figure: g.value.toLocaleString(), href: rowHref('genres', g), muted: g.label === 'Other genres' }))
	);
	const decadePoints = $derived((s?.decades ?? []).filter((d) => d.key).map((d) => ({ short: d.label, full: d.label, value: d.value })));
	const noYear = $derived(s?.decades.find((d) => !d.key)?.value ?? 0);
	const artistSongRows = $derived((s?.artists_by_songs ?? []).map((a) => ({ label: a.label, value: a.value, figure: songsWord(a.value), href: rowHref('artists', a), art: a.art ?? null, round: true })));
	const tagRows = $derived((s?.tags ?? []).map((g) => ({ label: g.label, value: g.have, figure: `${percent(g.have, g.of)} · ${g.have.toLocaleString()} of ${g.of.toLocaleString()}` })));
	const tagsMissing = $derived((s?.tags ?? []).slice(0, 4).some((g) => g.have < g.of));
</script>

<div class="view view-in st">
	<div class="pg-head"><h1 class="pg-title">Statistics</h1></div>

	{#if failure && !s}
		<p class="s-hint" role="alert">{failure}</p>
	{:else if !s || !t}
		<p class="s-hint">Counting…</p>
	{:else if t.songs === 0 && t.plays === 0}
		<p class="s-hint">No music files on this computer yet. Add a music folder and its numbers show up here.</p>
		<button type="button" class="btn ghost" onclick={() => void folders.add()}><Icon name="create_new_folder" />Add a music folder…</button>
	{:else}
		<div class="st-grid">
			<StoryCard title="Your library" sub="The music files on this computer, and everything you have played in Stash." wide>
				<ul class="st-big" aria-label="In all">
					{#each big as b (b.label)}
						<li><b>{b.value}</b><span>{b.label}</span></li>
					{/each}
				</ul>
				{#if t.songs}
					<div class="st-q" role="img" aria-label="Sound quality: {quality.map((q) => `${q.label} ${q.value.toLocaleString()}`).join(', ')}">
						{#each quality.filter((q) => q.value > 0) as q (q.label)}<i class="q{q.step}" style:flex-grow={q.value}></i>{/each}
					</div>
					<ul class="st-ql" aria-hidden="true">
						{#each quality as q (q.label)}
							<li><span class="sw q{q.step}"></span>{q.label} <b>{q.value.toLocaleString()}</b> · {q.share}</li>
						{/each}
					</ul>
				{/if}
			</StoryCard>

			{#if t.plays}
				{#if topSongs.length}
					<StoryCard title="Most played" sub="The songs you have played most. Choose one to play from it.">
						<TopList entries={topSongs} unit={['play', 'plays']} onplay={playTop} />
					</StoryCard>
				{/if}
				{#if topArtists.length}
					<StoryCard title="Top artists" sub="The artists you have played most.">
						<TopList entries={topArtists} round unit={['play', 'plays']} />
					</StoryCard>
				{/if}
				<StoryCard title="Month by month" sub="{playsWord(yearPlays)} in the last twelve months, and the song you played most in each." wide>
					{#if months.length}
						<MonthGrid {months} onplay={(id) => void player.playRows([id], 0, { from: 'Statistics' })} />
					{:else}
						<p class="st-note">No plays in the last twelve months.</p>
					{/if}
				</StoryCard>
			{/if}

			{#if t.songs}
				<StoryCard title="Formats" sub="What your files are, and the room they take.">
					<BarList label="Formats" rows={formatRows} />
				</StoryCard>
				{#if decadePoints.length}
					<StoryCard title="By decade" sub="When your songs came out.">
						<Columns label="Songs by decade" points={decadePoints} unit={songsWord} />
						{#if noYear}<p class="st-note">{songsWord(noYear)} without a year are not shown.</p>{/if}
					</StoryCard>
				{/if}
				{#if genreRows.length || s.no_genre}
					<StoryCard title="Genres" sub="Choose one to see its songs.">
						{#if genreRows.length}<BarList label="Songs by genre" rows={genreRows} />{/if}
						{#if s.no_genre}
							<p class="st-note"><a href="/library?tab=songs&genre=">{songsWord(s.no_genre)} without a genre</a> are not shown.</p>
						{/if}
					</StoryCard>
				{/if}
				{#if artistSongRows.length}
					<StoryCard title="Artists with the most songs" sub="On this computer.">
						<BarList label="Artists with the most songs" rows={artistSongRows} />
					</StoryCard>
				{/if}
				<StoryCard title="How complete your tags are" sub="The files that have each tag." wide>
					{#snippet tools()}
						{#if tagsMissing}<button type="button" class="btn ghost" onclick={() => (ui.filling = true)}><Icon name="edit" />Fill in missing tags…</button>{/if}
					{/snippet}
					<BarList label="Files with each tag" rows={tagRows} whole={t.songs} />
				</StoryCard>
			{/if}
		</div>
	{/if}
</div>

<style>
	/* Every bar in the album's colour, as Namida tints its cards with the artwork's. */
	.st { --st-bar: var(--acc); --st-bar-on: color-mix(in srgb, var(--acc) 72%, var(--wp-text)); --st-quiet: color-mix(in srgb, var(--acc) 24%, transparent); }
	.st-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(440px, 100%), 1fr)); gap: 18px; }
	/* Namida's big stats: the value large in the display face, its words under it. */
	.st-big { display: grid; grid-template-columns: repeat(auto-fit, minmax(128px, 1fr)); gap: 18px 20px; margin: 0; padding: 0; list-style: none; }
	.st-big li { display: grid; gap: 4px; min-width: 0; }
	.st-big b { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 700 30px/1.1 var(--font-display); letter-spacing: -.02em; color: var(--wp-text); font-variant-numeric: tabular-nums; }
	.st-big span { font: 600 12.5px/1.3 var(--font-body); color: var(--wp-text-2w); }
	/* Sound quality: one bar, 2px apart between its steps. */
	.st-q { display: flex; gap: 2px; height: 10px; margin: 24px 0 10px; overflow: hidden; border-radius: 5px; }
	.st-q i { min-width: 4px; }
	.st-ql { display: flex; flex-wrap: wrap; gap: 6px 20px; margin: 0; padding: 0; list-style: none; color: var(--wp-text-2w); font: 400 13px/1.4 var(--font-body); }
	.st-ql li { display: inline-flex; align-items: center; gap: 7px; }
	.st-ql b { color: var(--wp-text); font-weight: 600; font-variant-numeric: tabular-nums; }
	.sw { width: 10px; height: 10px; border-radius: 3px; flex: none; }
	.q0 { background: var(--acc); }
	.q1 { background: color-mix(in oklab, var(--acc) 55%, var(--wp-main)); }
	.q2 { background: color-mix(in srgb, var(--wp-text) 30%, transparent); }
	.st-note a { color: inherit; text-decoration: underline; text-underline-offset: 2px; }
	.st-note { margin: 10px 0 0; color: var(--wp-text-2w); font: 400 12.5px/1.4 var(--font-body); }
</style>
