<script lang="ts">
  import { onMount } from "svelte";
  import VirtualList from "./lib/VirtualList.svelte";
  import { counts as fetchCounts, measuring, postMeasurement, search, tracksAt, type Counts, type TrackRow } from "./lib/api";
  import { measure, type Drawn } from "./lib/bench";

  const ROW_HEIGHT = 34;

  let counts: Counts | null = $state(null);
  let failure: string | null = $state(null);
  let query = $state("");
  /** The search results on screen, and the query they answer (null: the whole library is showing). */
  let results: TrackRow[] = $state([]);
  let resultsFor: string | null = $state(null);
  let list: VirtualList<TrackRow> | undefined = $state();
  let drawn: Drawn = { first: 0, last: 0, loaded: 0 };
  /** When things first happened, in ms since the page began to load (not tied to frames, for the measurement). */
  const marks: Record<string, number> = { scriptMs: Math.round(performance.now()) };

  function noteDrawn(state: Drawn) {
    drawn = state;
    if (marks.firstRowsDataMs === undefined && state.last > state.first && state.loaded === state.last - state.first) {
      marks.firstRowsDataMs = Math.round(performance.now());
    }
  }

  let searching: AbortController | undefined;

  function runSearch(text: string) {
    searching?.abort();
    if (text.trim() === "") {
      results = [];
      resultsFor = null;
      return;
    }
    const controller = new AbortController();
    searching = controller;
    search(text, 100, controller.signal)
      .then((rows) => {
        results = rows;
        resultsFor = text;
      })
      .catch(() => {});
  }

  $effect(() => runSearch(query));

  const whole = new Intl.NumberFormat();

  function length(ms: number): string {
    const seconds = Math.round(ms / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }

  /** The format as decoded: "FLAC 24/96", "Opus". */
  function format(track: TrackRow): string {
    const codec = (track.codec ?? "").toUpperCase();
    if (track.bit_depth && track.sample_rate) return `${codec} ${track.bit_depth}/${Math.round(track.sample_rate / 100) / 10}`;
    return codec === "OPUS" ? "Opus" : codec;
  }

  onMount(async () => {
    try {
      counts = await fetchCounts();
      marks.countsMs = Math.round(performance.now());
    } catch (e) {
      failure = `The Stash core is not answering (${e instanceof Error ? e.message : e}).`;
      return;
    }
    if (measuring) {
      const result = await measure({
        count: counts.tracks,
        rowHeight: ROW_HEIGHT,
        drawn: () => drawn,
        scrollToIndex: (index) => list?.scrollToIndex(index),
        scrollBy: (pixels) => list?.scrollBy(pixels),
        type: (text) => (query = text),
        showing: () => resultsFor,
        marks,
      });
      await postMeasurement(result).catch(() => {});
      document.title = "Stash (measured)";
    }
  });
</script>

{#snippet trackRow(track: TrackRow | undefined, index: number)}
  {#if track}
    <span class="n">{index + 1}</span>
    <span class="title">{track.title}</span>
    <span class="artist">{track.artist}</span>
    <span class="album">{track.album ?? ""}</span>
    <span class="format">{format(track)}</span>
    <span class="time">{length(track.duration_ms)}</span>
  {:else}
    <span class="n">{index + 1}</span>
    <span class="placeholder"></span>
  {/if}
{/snippet}

<header>
  <h1>Stash</h1>
  <input type="search" placeholder="Search your library" bind:value={query} aria-label="Search your library" />
  {#if counts}
    <p class="counts">{whole.format(counts.tracks)} tracks · {whole.format(counts.albums)} albums · {whole.format(counts.artists)} artists</p>
  {/if}
</header>

<div class="columns" aria-hidden="true">
  <span class="n">#</span><span>Title</span><span>Artist</span><span>Album</span><span>Format</span><span class="time">Time</span>
</div>

<main>
  {#if failure}
    <p class="message">{failure}</p>
  {:else if resultsFor !== null}
    {#if results.length === 0}
      <p class="message">Nothing in your library matches "{resultsFor}".</p>
    {:else}
      <div class="results" role="list">
        {#each results as track, i (track.id)}
          <div class="row" role="listitem" style:height="{ROW_HEIGHT}px">{@render trackRow(track, i)}</div>
        {/each}
      </div>
    {/if}
  {:else if counts}
    <VirtualList
      bind:this={list}
      count={counts.tracks}
      rowHeight={ROW_HEIGHT}
      fetchPage={tracksAt}
      row={trackRow}
      onDrawn={noteDrawn}
    />
  {/if}
</main>

<style>
  header {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 12px 16px;
  }
  h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 650;
  }
  input {
    flex: 0 1 360px;
    padding: 7px 12px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: var(--field);
    color: inherit;
    font: inherit;
  }
  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .counts {
    margin: 0 0 0 auto;
    color: var(--quiet);
    font-variant-numeric: tabular-nums;
  }
  .columns,
  :global(.row) {
    display: grid;
    grid-template-columns: 64px minmax(0, 3fr) minmax(0, 2fr) minmax(0, 2fr) 110px 56px;
    gap: 12px;
    align-items: center;
    padding: 0 16px;
  }
  .columns {
    height: 30px;
    color: var(--quiet);
    font-size: 12px;
    border-bottom: 1px solid var(--line);
  }
  main {
    flex: 1;
    min-height: 0;
  }
  .results {
    height: 100%;
    overflow-y: auto;
  }
  :global(.row span) {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  :global(.row .n),
  :global(.row .time),
  .columns .n,
  .columns .time {
    color: var(--quiet);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  :global(.row .artist),
  :global(.row .album),
  :global(.row .format) {
    color: var(--soft);
  }
  :global(.row:hover) {
    background: var(--hover);
  }
  :global(.row .placeholder) {
    grid-column: 2 / -1;
    height: 10px;
    width: 38%;
    border-radius: 5px;
    background: var(--line);
  }
  .message {
    padding: 32px 16px;
    color: var(--quiet);
  }
</style>
