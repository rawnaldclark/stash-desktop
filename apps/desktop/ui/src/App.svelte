<script lang="ts">
  import { onMount } from "svelte";
  import Addons from "./lib/Addons.svelte";
  import NowPlaying from "./lib/NowPlaying.svelte";
  import VirtualList from "./lib/VirtualList.svelte";
  import {
    addons as fetchAddons,
    counts as fetchCounts,
    IDLE,
    measuring,
    nextTrack,
    player as fetchPlayer,
    playFound,
    playFrom,
    playTracks,
    postMeasurement,
    previousTrack,
    search,
    searchAddon,
    seekTo,
    setPaused,
    tracksAt,
    type AddonGroup,
    type AddonInfo,
    type Counts,
    type FoundTrack,
    type PlayerState,
    type TrackRow,
  } from "./lib/api";
  import { measure, type Drawn } from "./lib/bench";

  const ROW_HEIGHT = 34;
  /** An add-on is asked once the listener has stopped typing for this long: its answers cost more than the library's. */
  const ADDON_SEARCH_PAUSE_MS = 350;

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

  /** The player as the core last reported it, and when (the position is carried forward between reports). */
  let playing: PlayerState = $state(IDLE);
  let reportedAt = 0;
  let now = $state(0);

  function heard(state: PlayerState) {
    playing = state;
    reportedAt = performance.now();
    now = reportedAt;
  }

  /** The player with its position moved on by the time since the core's report, so the bar runs smoothly. */
  const shown: PlayerState = $derived.by(() => {
    if (playing.status !== "playing" || !playing.duration_ms) return playing;
    const position = Math.min(playing.duration_ms, playing.position_ms + (now - reportedAt));
    return { ...playing, position_ms: position };
  });

  const act = (call: Promise<PlayerState>) => call.then(heard).catch(() => {});

  function toggle(event: KeyboardEvent) {
    const typing = event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement;
    if (event.code !== "Space" || typing || !playing.track || playing.status === "loading") return;
    event.preventDefault();
    act(setPaused(playing.status === "playing"));
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

  // ---- Add-ons

  let addons: AddonInfo[] = $state([]);
  let addonsOpen = $state(false);
  /** The add-ons a search asks: switched on, usable, and offering search. In the listener's order. */
  const asked = $derived(addons.filter((a) => a.enabled && !a.problem && a.resources.includes("search")));
  const addonName = $derived(addons.find((a) => a.id === playing.track?.addon?.addon)?.name ?? null);

  /** What each asked add-on answered for `foundFor`; an add-on with no entry yet is still being asked. */
  let found: Record<string, AddonGroup | { failed: string }> = $state({});
  let foundFor: string | null = $state(null);

  $effect(() => {
    const text = query.trim();
    const ids = asked.map((a) => a.id);
    if (text === "" || ids.length === 0) {
      found = {};
      foundFor = null;
      return;
    }
    const controller = new AbortController();
    const timer = setTimeout(() => {
      found = {};
      foundFor = text;
      for (const id of ids) {
        searchAddon(id, text, 25, controller.signal)
          .then((group) => {
            if (!controller.signal.aborted && group) found[id] = group;
          })
          .catch(() => {
            if (!controller.signal.aborted) found[id] = { failed: "The Stash core is not answering." };
          });
      }
    }, ADDON_SEARCH_PAUSE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  });

  const TIERS: Record<string, string> = { low: "Low", high: "High", lossless: "Lossless", hires: "Hi-res" };

  const whole = new Intl.NumberFormat();

  function length(ms: number | null): string {
    if (ms === null) return "";
    const seconds = Math.round(ms / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }

  /** The format as decoded: "FLAC 24/96", "Opus". */
  function format(track: TrackRow): string {
    const codec = (track.codec ?? "").toUpperCase();
    if (track.bit_depth && track.sample_rate) return `${codec} ${track.bit_depth}/${Math.round(track.sample_rate / 100) / 10}`;
    return codec === "OPUS" ? "Opus" : codec;
  }

  // Once the core answers: it is asked twice a second while something plays, and now and then when nothing does.
  $effect(() => {
    if (measuring || !counts) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const ask = async () => {
      const state = await fetchPlayer().catch(() => null);
      if (stopped) return;
      if (state) heard(state);
      timer = setTimeout(ask, state && state.status !== "idle" ? 500 : 1500);
    };
    ask();
    const tick = setInterval(() => (now = performance.now()), 250);
    return () => {
      stopped = true;
      clearTimeout(timer);
      clearInterval(tick);
    };
  });

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
      return;
    }
    addons = await fetchAddons().catch(() => []);
  });
</script>

{#snippet trackRow(track: TrackRow | undefined, index: number)}
  {#if track}
    <span class="n">{index + 1}</span>
    <span class="title" class:now={track.id === playing.track?.id}>{track.title}</span>
    <span class="artist">{track.artist}</span>
    <span class="album">{track.album ?? ""}</span>
    <span class="format">{format(track)}</span>
    <span class="time">{length(track.duration_ms)}</span>
  {:else}
    <span class="n">{index + 1}</span>
    <span class="placeholder"></span>
  {/if}
{/snippet}

{#snippet foundRow(addon: string, track: FoundTrack, index: number)}
  <span class="n">{index + 1}</span>
  <span class="title" class:now={playing.track?.addon?.addon === addon && playing.track.addon.track === track.id}>{track.title}</span>
  <span class="artist">{track.artist}</span>
  <span class="album">{track.album ?? ""}</span>
  <span class="format">{track.max_tier ? TIERS[track.max_tier] : ""}</span>
  <span class="time">{length(track.duration_ms)}</span>
{/snippet}

<header>
  <h1>Stash</h1>
  <input
    type="search"
    placeholder={asked.length ? "Search your library and add-ons" : "Search your library"}
    bind:value={query}
    aria-label={asked.length ? "Search your library and add-ons" : "Search your library"}
  />
  {#if counts}
    <p class="counts">{whole.format(counts.tracks)} tracks · {whole.format(counts.albums)} albums · {whole.format(counts.artists)} artists</p>
  {/if}
  {#if counts && !measuring}
    <button type="button" class="addons" onclick={() => (addonsOpen = true)}>Add-ons{addons.length ? ` (${addons.length})` : ""}</button>
  {/if}
</header>

<div class="columns" aria-hidden="true">
  <span class="n">#</span><span>Title</span><span>Artist</span><span>Album</span><span>Format</span><span class="time">Time</span>
</div>

<main>
  {#if failure}
    <p class="message">{failure}</p>
  {:else if resultsFor !== null}
    <div class="results">
      {#if asked.length}
        <h2>Your library</h2>
      {/if}
      {#if results.length === 0}
        <p class="message" class:slim={asked.length > 0}>Nothing in your library matches "{resultsFor}".</p>
      {:else}
        <div role="list">
          {#each results as track, i (track.id)}
            <div
              class="row"
              role="listitem"
              style:height="{ROW_HEIGHT}px"
              ondblclick={() => act(playTracks(results.slice(i).map((r) => r.id)))}
            >
              {@render trackRow(track, i)}
            </div>
          {/each}
        </div>
      {/if}
      {#each asked as addon (addon.id)}
        {@const group = foundFor === null ? undefined : found[addon.id]}
        <h2>{addon.name}</h2>
        {#if group === undefined}
          <p class="message slim">Asking {addon.name}…</p>
        {:else if "failed" in group}
          <p class="message slim trouble">{group.failed}</p>
        {:else if group.error}
          <p class="message slim trouble">{addon.name} gave no results: {group.error.message}.</p>
        {:else if group.tracks.length === 0}
          <p class="message slim">{addon.name} found nothing for "{foundFor}".</p>
        {:else}
          <div role="list">
            {#each group.tracks as track, i (track.id)}
              <div
                class="row"
                role="listitem"
                style:height="{ROW_HEIGHT}px"
                ondblclick={() => act(playFound(addon.id, group.tracks.slice(i)))}
              >
                {@render foundRow(addon.id, track, i)}
              </div>
            {/each}
          </div>
        {/if}
      {/each}
    </div>
  {:else if counts}
    <VirtualList
      bind:this={list}
      count={counts.tracks}
      rowHeight={ROW_HEIGHT}
      fetchPage={tracksAt}
      row={trackRow}
      onDrawn={noteDrawn}
      onActivate={(index) => act(playFrom(index))}
    />
  {/if}
</main>

<NowPlaying
  player={shown}
  {addonName}
  onPause={(paused) => act(setPaused(paused))}
  onNext={() => act(nextTrack())}
  onPrevious={() => act(previousTrack())}
  onSeek={(seconds) => act(seekTo(seconds))}
/>

{#if addonsOpen}
  <Addons {addons} onChange={(changed) => (addons = changed)} onClose={() => (addonsOpen = false)} />
{/if}

<svelte:window onkeydown={toggle} />

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
  input:focus-visible,
  .addons:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .counts {
    margin: 0 0 0 auto;
    color: var(--quiet);
    font-variant-numeric: tabular-nums;
  }
  .addons {
    padding: 6px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--field);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .addons:hover {
    background: var(--hover);
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
  h2 {
    margin: 0;
    padding: 18px 16px 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--soft);
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
  :global(.row .title.now) {
    color: var(--accent);
    font-weight: 600;
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
    margin: 0;
    padding: 32px 16px;
    color: var(--quiet);
  }
  .message.slim {
    padding: 6px 16px 6px 92px;
  }
  .message.trouble {
    color: var(--warn);
  }
</style>
