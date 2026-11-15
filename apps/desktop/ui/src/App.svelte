<script lang="ts">
  import { onMount } from "svelte";
  import Addons from "./lib/Addons.svelte";
  import NowPlaying from "./lib/NowPlaying.svelte";
  import Queue from "./lib/Queue.svelte";
  import Sound from "./lib/Sound.svelte";
  import RowMenu from "./lib/RowMenu.svelte";
  import VirtualList from "./lib/VirtualList.svelte";
  import {
    addons as fetchAddons,
    addToPlaylist,
    counts as fetchCounts,
    createPlaylist,
    deletePlaylist,
    exportLibrary,
    IDLE,
    importLibrary,
    jumpTo,
    keepFound,
    likedAt,
    likeTrack,
    measuring,
    moveInPlaylist,
    nextTrack,
    pickFolder,
    player as fetchPlayer,
    playFound,
    playFrom,
    playLiked,
    playlists as fetchPlaylists,
    playlistTracksAt,
    playPlaylist,
    playTracks,
    queueAt,
    postMeasurement,
    previousTrack,
    Refused,
    removeFromPlaylist,
    renamePlaylist,
    scan,
    scanning as fetchScanning,
    search,
    searchAddon,
    seekTo,
    setExclusive,
    setPaused,
    setVolume,
    tracksAt,
    type AddonGroup,
    type AddonInfo,
    type Changed,
    type Counts,
    type FoundTrack,
    type Imported,
    type PlayerState,
    type PlaylistRow,
    type QueueRow,
    type Scanning,
    type TrackRow,
  } from "./lib/api";
  import { measure, type Drawn } from "./lib/bench";

  const ROW_HEIGHT = 34;
  /** An add-on is asked once the listener has stopped typing for this long: its answers cost more than the library's. */
  const ADDON_SEARCH_PAUSE_MS = 350;

  let counts: Counts | null = $state(null);
  let failure: string | null = $state(null);
  let query = $state("");
  /** The search results on screen, and the query they answer (null: a list is showing). */
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

  // ---- What the list shows: everything, the liked songs, or one playlist

  type View = { kind: "all" } | { kind: "liked" } | { kind: "playlist"; id: number };
  let view = $state({ kind: "all" } as View);
  let playlists: PlaylistRow[] = $state([]);
  /** Goes up when the library changed under the list (an import), so the list reads it again from the top. */
  let libraryVersion = $state(0);

  const shownPlaylist = $derived.by(() => {
    const shown = view;
    return shown.kind === "playlist" ? playlists.find((p) => p.id === shown.id) : undefined;
  });
  /** How many rows the list has, how a page of them is read, and what plays when a row is started. */
  const listing = $derived.by(() => {
    if (!counts) return null;
    if (view.kind === "liked") {
      return { key: `liked:${libraryVersion}`, count: counts.liked, fetchPage: likedAt, play: (index: number) => playLiked(index), empty: "Nothing is liked yet." };
    }
    if (view.kind === "playlist") {
      const id = view.id;
      return {
        key: `playlist:${id}:${libraryVersion}`,
        count: shownPlaylist?.tracks ?? 0,
        fetchPage: (offset: number, limit: number, signal?: AbortSignal) => playlistTracksAt(id, offset, limit, signal),
        play: (index: number) => playPlaylist(id, index),
        empty: "This playlist is empty. Add songs to it from the + beside any song.",
      };
    }
    return {
      key: `all:${libraryVersion}`,
      count: counts.tracks,
      fetchPage: tracksAt,
      play: (index: number) => playFrom(index),
      empty: "Your library is empty. Scan a music folder, or import a library from Stash on your phone or the web.",
    };
  });

  function show(next: View) {
    view = next;
    query = "";
    editing = null;
  }

  // ---- Bringing a library in from another Stash, and writing this one out for them

  let picker: HTMLInputElement | undefined = $state();
  let importing = $state(false);
  /** What the last import, save or edit said. */
  let notice: { text: string; trouble: boolean } | null = $state(null);

  const whole = new Intl.NumberFormat();
  const many = (n: number, one: string, more: string) => `${whole.format(n)} ${n === 1 ? one : more}`;
  const trouble = (e: unknown) => ({ text: e instanceof Refused ? `${e.message}.` : "The Stash core is not answering.", trouble: true });

  /** What an import did, in a sentence or two. */
  function said(result: Imported): string {
    const s = result.summary;
    const parts = [
      s.songs_added ? many(s.songs_added, "new song", "new songs") : "",
      s.likes_added ? many(s.likes_added, "like", "likes") : "",
      s.playlists_added ? many(s.playlists_added, "playlist", "playlists") : "",
      s.playlist_songs_added ? `${many(s.playlist_songs_added, "song", "songs")} added to playlists that were here` : "",
      s.plays_added ? many(s.plays_added, "play", "plays") : "",
    ].filter((p) => p !== "");
    const from = result.generator ? ` from ${result.generator}` : "";
    if (parts.length === 0) return `Nothing new${from}: everything in that file was here already.`;
    const here = s.songs_already ? ` ${many(s.songs_already, "song was", "songs were")} here already.` : "";
    return `Imported${from}: ${parts.join(", ")}.${here}`;
  }

  async function importFile() {
    const file = picker?.files?.[0];
    if (!file || importing) return;
    importing = true;
    notice = null;
    try {
      const result = await importLibrary(file);
      counts = result.counts;
      playlists = await fetchPlaylists().catch(() => playlists);
      libraryVersion += 1;
      notice = { text: said(result), trouble: false };
    } catch (e) {
      notice = trouble(e);
    } finally {
      importing = false;
      if (picker) picker.value = "";
    }
  }

  /** Saves the library as the file the Android app and the web player import. */
  async function saveFile() {
    try {
      const { blob, likes, lists, plays } = await exportLibrary();
      const name = `stash-library-${new Date().toISOString().slice(0, 10)}.json`;
      const link = document.createElement("a");
      link.href = URL.createObjectURL(blob);
      link.download = name;
      link.click();
      setTimeout(() => URL.revokeObjectURL(link.href), 10_000);
      notice = {
        text: `Saved ${name}: ${many(likes, "like", "likes")}, ${many(lists, "playlist", "playlists")}, ${many(plays, "play", "plays")}. Import it in Stash on your phone or the web.`,
        trouble: false,
      };
    } catch (e) {
      notice = trouble(e);
    }
  }

  // ---- Music folders on this computer

  let folders: string[] = $state([]);
  /** The scan that is running, as the core last told it; null when none is. */
  let scanned: Scanning["scan"] | null = $state(null);

  const lastPart = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;

  /** Follows a scan until it ends, then takes in what it changed and says so. */
  async function follow(started: Scanning) {
    folders = started.folders;
    scanned = started.scan;
    while (scanned?.running) {
      await new Promise((resolve) => setTimeout(resolve, 400));
      const state = await fetchScanning().catch(() => null);
      if (!state) continue;
      folders = state.folders;
      scanned = state.scan;
    }
    const ended = scanned;
    scanned = null;
    counts = await fetchCounts().catch(() => counts);
    libraryVersion += 1;
    if (ended?.error) {
      notice = { text: `The scan stopped: ${ended.error}.`, trouble: true };
    } else if (ended?.report) {
      const r = ended.report;
      const parts = [
        many(r.added, "new song", "new songs"),
        r.changed ? `${whole.format(r.changed)} read again` : "",
        r.missing ? `${whole.format(r.missing)} no longer there` : "",
        r.unreadable ? `${many(r.unreadable, "file", "files")} that could not be read` : "",
      ].filter((p) => p !== "");
      const nothing = r.added === 0 && parts.length === 1;
      notice = { text: `Scanned ${many(r.files, "file", "files")}: ${nothing ? "nothing has changed" : parts.join(", ")}.`, trouble: false };
    }
  }

  async function addFolder() {
    try {
      const folder = await pickFolder();
      if (folder) await follow(await scan(folder));
    } catch (e) {
      notice = trouble(e);
    }
  }

  async function scanAgain() {
    try {
      await follow(await scan());
    } catch (e) {
      notice = trouble(e);
    }
  }

  // ---- The queue, and the sound

  let queueOpen = $state(false);
  let queue: QueueRow[] = $state([]);
  let soundOpen = $state(false);

  // The queue is read when it is opened and whenever it changes or moves on.
  $effect(() => {
    void playing.queue_len;
    void playing.index;
    if (!queueOpen) return;
    queueAt(0, 500)
      .then((rows) => (queue = rows))
      .catch(() => {});
  });

  // ---- Liking, and playlists

  /** The core's answer to a change: the playlists and the counts as they now are. */
  function took(changed: Changed) {
    playlists = changed.playlists;
    counts = changed.counts;
  }

  /** Something the listener changed: the answer is taken in; a refusal is said. */
  async function change(call: Promise<Changed>): Promise<Changed | null> {
    try {
      const changed = await call;
      took(changed);
      return changed;
    } catch (e) {
      notice = trouble(e);
      return null;
    }
  }

  /** The add-on tracks the listener liked from search results in this sitting, as `addon:track`. */
  let likedFound: Record<string, boolean> = $state({});

  async function toggleLike(track: TrackRow) {
    const liked = !track.liked;
    if (!(await change(likeTrack(track.id, liked)))) return;
    track.liked = liked;
    // The rows of a long list are plain data: it is told to draw them again, or (the liked songs) to read them again.
    if (view.kind === "liked") list?.refresh();
    else list?.redraw();
  }

  async function toggleFound(addon: string, track: FoundTrack) {
    const key = `${addon}:${track.id}`;
    const liked = !likedFound[key];
    const kept = await change(keepFound(addon, track));
    if (kept?.id === undefined || !(await change(likeTrack(kept.id, liked)))) return;
    likedFound[key] = liked;
  }

  /** The menu beside a song: where it is, and which song (a library row, or an add-on's track that is kept first). */
  let menu: { x: number; y: number; id?: number; found?: { addon: string; track: FoundTrack }; index?: number } | null = $state(null);

  function openMenu(event: MouseEvent, what: { id?: number; found?: { addon: string; track: FoundTrack }; index?: number }) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: box.right, y: box.bottom + 4, ...what };
  }

  /** The library's id of the song the menu is for. */
  async function menuSong(): Promise<number | undefined> {
    if (menu?.id !== undefined) return menu.id;
    if (!menu?.found) return undefined;
    return (await change(keepFound(menu.found.addon, menu.found.track)))?.id;
  }

  async function addTo(playlist: number) {
    const id = await menuSong();
    menu = null;
    if (id === undefined) return;
    const changed = await change(addToPlaylist(playlist, [id]));
    const name = playlists.find((p) => p.id === playlist)?.name ?? "the playlist";
    if (changed) notice = { text: changed.added ? `Added to ${name}.` : `${name} has that song already.`, trouble: false };
    if (view.kind === "playlist" && view.id === playlist) list?.refresh();
  }

  async function addToNew(name: string) {
    const made = await change(createPlaylist(name));
    if (made?.id !== undefined) await addTo(made.id);
    else menu = null;
  }

  /** The place in the shown playlist of the song the menu is for, when that playlist can be edited. */
  const menuPlace = $derived.by(() => {
    if (!menu || menu.index === undefined || view.kind !== "playlist" || resultsFor !== null || !shownPlaylist || shownPlaylist.followed) return null;
    return { first: menu.index === 0, last: menu.index === shownPlaylist.tracks - 1 };
  });

  async function moveBy(by: number) {
    if (!menu || menu.index === undefined || view.kind !== "playlist") return;
    const [id, from] = [view.id, menu.index];
    menu = null;
    if (await change(moveInPlaylist(id, from, from + by))) list?.refresh();
  }

  async function takeOut() {
    if (!menu || menu.index === undefined || view.kind !== "playlist") return;
    const [id, index] = [view.id, menu.index];
    menu = null;
    if (await change(removeFromPlaylist(id, index))) list?.refresh();
  }

  /** What is being done to the shown playlist or the list of playlists: naming a new one, renaming, or deleting. */
  let editing: { kind: "new" | "rename" | "delete"; name: string } | null = $state(null);

  async function finishEditing(event?: SubmitEvent) {
    event?.preventDefault();
    const doing = editing;
    if (!doing) return;
    if (doing.kind === "new") {
      const made = await change(createPlaylist(doing.name));
      if (made?.id !== undefined) show({ kind: "playlist", id: made.id });
    } else if (view.kind === "playlist") {
      if (doing.kind === "rename") await change(renamePlaylist(view.id, doing.name));
      else if (await change(deletePlaylist(view.id))) show({ kind: "all" });
    }
    editing = null;
  }

  // ---- The player

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

  /** Asks the core for the player's state soon: set while the asking runs. */
  let askSoon: (() => void) | undefined;
  /** Something the listener did to the player: its answer is shown, and the core is asked again shortly after. */
  const act = (call: Promise<PlayerState>) =>
    call
      .then((state) => {
        heard(state);
        askSoon?.();
      })
      .catch(() => {});

  function toggle(event: KeyboardEvent) {
    const typing = event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement;
    if (event.code !== "Space" || typing || event.target instanceof HTMLButtonElement || !playing.track || playing.status === "loading") return;
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
  /** The add-on the playing stream comes from, by name. */
  const addonName = $derived(addons.find((a) => a.id === (playing.served_by ?? playing.track?.addon?.addon))?.name ?? null);

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

  function length(ms: number | null): string {
    if (!ms) return "";
    const seconds = Math.round(ms / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }

  /** The format as decoded: "FLAC 24/96", "Opus". A song the library knows by its description has none of its own. */
  function format(track: TrackRow): string {
    if (track.origin === "described") return "Add-ons";
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
      // Often while a stream is being asked for, twice a second while something plays, now and then otherwise.
      const pause = state?.status === "loading" ? 250 : state && state.status !== "idle" ? 500 : 1500;
      timer = setTimeout(ask, pause);
    };
    askSoon = () => {
      clearTimeout(timer);
      timer = setTimeout(ask, 150);
    };
    ask();
    const tick = setInterval(() => (now = performance.now()), 250);
    return () => {
      stopped = true;
      askSoon = undefined;
      clearTimeout(timer);
      clearInterval(tick);
    };
  });

  // A stream from an add-on the window has not heard of (added from somewhere else): the list is read again.
  $effect(() => {
    const id = playing.served_by;
    if (id && !addons.some((a) => a.id === id)) {
      fetchAddons()
        .then((listed) => (addons = listed))
        .catch(() => {});
    }
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
    [addons, playlists] = await Promise.all([fetchAddons().catch(() => []), fetchPlaylists().catch(() => [])]);
    // A scan that was running when the window opened is followed to its end.
    const state = await fetchScanning().catch(() => null);
    if (state) {
      folders = state.folders;
      if (state.scan.running) follow(state);
    }
  });
</script>

{#snippet heart()}
  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 21 4.3 13.4a5 5 0 0 1 7.1-7.1l.6.6.6-.6a5 5 0 0 1 7.1 7.1z" /></svg>
{/snippet}

{#snippet plus()}
  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z" /></svg>
{/snippet}

{#snippet trackRow(track: TrackRow | undefined, index: number)}
  {#if track}
    <span class="n">{index + 1}</span>
    <span class="title" class:now={track.id === playing.track?.id}>{track.title}</span>
    <span class="artist">{track.artist}</span>
    <span class="album">{track.album ?? ""}</span>
    <span class="format" class:quiet={track.origin === "described"}>{format(track)}</span>
    <span class="time">{length(track.duration_ms)}</span>
    {#if !measuring}
      <span class="acts" role="presentation" ondblclick={(e) => e.stopPropagation()}>
        <button type="button" class="act" class:on={track.liked} aria-pressed={track.liked} aria-label={track.liked ? `Take the like back from ${track.title}` : `Like ${track.title}`} onclick={() => toggleLike(track)}>
          {@render heart()}
        </button>
        <button type="button" class="act" aria-label={`Add ${track.title} to a playlist`} onclick={(e) => openMenu(e, { id: track.id, index })}>
          {@render plus()}
        </button>
      </span>
    {/if}
  {:else}
    <span class="n">{index + 1}</span>
    <span class="placeholder"></span>
  {/if}
{/snippet}

{#snippet foundRow(addon: string, track: FoundTrack, index: number)}
  {@const liked = likedFound[`${addon}:${track.id}`] === true}
  <span class="n">{index + 1}</span>
  <span class="title" class:now={playing.track?.addon?.addon === addon && playing.track.addon.track === track.id}>{track.title}</span>
  <span class="artist">{track.artist}</span>
  <span class="album">{track.album ?? ""}</span>
  <span class="format">{track.max_tier ? TIERS[track.max_tier] : ""}</span>
  <span class="time">{length(track.duration_ms)}</span>
  <span class="acts" role="presentation" ondblclick={(e) => e.stopPropagation()}>
    <button type="button" class="act" class:on={liked} aria-pressed={liked} aria-label={liked ? `Take the like back from ${track.title}` : `Like ${track.title}`} onclick={() => toggleFound(addon, track)}>
      {@render heart()}
    </button>
    <button type="button" class="act" aria-label={`Add ${track.title} to a playlist`} onclick={(e) => openMenu(e, { found: { addon, track } })}>
      {@render plus()}
    </button>
  </span>
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
    <button type="button" class="plain-button" onclick={() => (soundOpen = true)}>Sound</button>
    <button type="button" class="plain-button" onclick={() => (addonsOpen = true)}>Add-ons{addons.length ? ` (${addons.length})` : ""}</button>
  {/if}
</header>

<div class="body">
  {#if counts && !measuring}
    <nav aria-label="Your library">
      <button type="button" class="place" class:here={view.kind === "all" && resultsFor === null} onclick={() => show({ kind: "all" })}>
        <span>All tracks</span><span class="count">{whole.format(counts.tracks)}</span>
      </button>
      <button type="button" class="place" class:here={view.kind === "liked" && resultsFor === null} onclick={() => show({ kind: "liked" })}>
        <span>Liked</span><span class="count">{whole.format(counts.liked)}</span>
      </button>
      <div class="heading">
        <h2>Playlists</h2>
        <button type="button" class="act" aria-label="Make a playlist" onclick={() => (editing = { kind: "new", name: "" })}>{@render plus()}</button>
      </div>
      {#if editing?.kind === "new"}
        <form class="naming" onsubmit={finishEditing}>
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" placeholder="Its name" bind:value={editing.name} maxlength="100" aria-label="The new playlist's name" autofocus onkeydown={(e) => e.key === "Escape" && (editing = null)} />
        </form>
      {/if}
      <div class="lists">
        {#each playlists as playlist (playlist.id)}
          <button
            type="button"
            class="place"
            class:here={view.kind === "playlist" && view.id === playlist.id && resultsFor === null}
            onclick={() => show({ kind: "playlist", id: playlist.id })}
            title={playlist.followed ? `${playlist.name} (a shared mix)` : playlist.name}
          >
            <span>{playlist.name}</span><span class="count">{whole.format(playlist.tracks)}</span>
          </button>
        {/each}
      </div>
      <div class="bring">
        {#if scanned}
          <p class="scanning" role="status">Scanning {scanned.folder ? lastPart(scanned.folder) : ""}… {whole.format(scanned.report?.files ?? 0)} files</p>
        {:else}
          <button type="button" class="plain-button" onclick={addFolder}>Add a music folder…</button>
          {#if folders.length}
            <button type="button" class="plain-button" onclick={scanAgain} title={folders.join("\n")}>
              Scan {folders.length === 1 ? lastPart(folders[0]) : `${folders.length} folders`} again
            </button>
          {/if}
        {/if}
        <input bind:this={picker} type="file" accept=".json,.zip,.db,application/json,application/zip" onchange={importFile} hidden />
        <button type="button" class="plain-button" disabled={importing} onclick={() => picker?.click()}>
          {importing ? "Importing…" : "Import a library…"}
        </button>
        <button type="button" class="plain-button" onclick={saveFile}>Save a library file</button>
        <p>One file carries likes, playlists and plays between this computer, Stash on your phone and the web player. The phone's Backup (.zip) can be imported too.</p>
      </div>
    </nav>
  {/if}

  <div class="content">
    {#if notice}
      <p class="notice" class:trouble={notice.trouble} role="status">
        <span>{notice.text}</span>
        <button type="button" class="act" onclick={() => (notice = null)} aria-label="Close this message">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.4 5 12 10.6 17.6 5 19 6.4 13.4 12l5.6 5.6-1.4 1.4-5.6-5.6L6.4 19 5 17.6 10.6 12 5 6.4z" /></svg>
        </button>
      </p>
    {/if}
    {#if shownPlaylist && resultsFor === null}
      <div class="about">
        {#if editing?.kind === "rename"}
          <form onsubmit={finishEditing}>
            <!-- svelte-ignore a11y_autofocus -->
            <input type="text" bind:value={editing.name} maxlength="100" aria-label="The playlist's name" autofocus onkeydown={(e) => e.key === "Escape" && (editing = null)} />
            <button type="submit" class="plain-button">Save</button>
            <button type="button" class="plain-button" onclick={() => (editing = null)}>Cancel</button>
          </form>
        {:else if editing?.kind === "delete"}
          <span class="ask">Delete {shownPlaylist.name}? Its songs stay in your library.</span>
          <button type="button" class="plain-button" onclick={() => finishEditing()}>Delete</button>
          <button type="button" class="plain-button" onclick={() => (editing = null)}>Keep</button>
        {:else}
          <h2>{shownPlaylist.name}</h2>
          <span class="count">{many(shownPlaylist.tracks, "song", "songs")}{shownPlaylist.followed ? " · a shared mix" : ""}</span>
          <button type="button" class="plain-button" onclick={() => (editing = { kind: "rename", name: shownPlaylist.name })}>Rename</button>
          <button type="button" class="plain-button" onclick={() => (editing = { kind: "delete", name: "" })}>Delete</button>
        {/if}
      </div>
    {/if}
    <div class="columns" aria-hidden="true">
      <span class="n">#</span><span>Title</span><span>Artist</span><span>Album</span><span>Format</span><span class="time">Time</span><span></span>
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
      {:else if listing}
        {#if listing.count === 0}
          <p class="message">{listing.empty}</p>
        {:else}
          {#key listing.key}
            <VirtualList
              bind:this={list}
              count={listing.count}
              rowHeight={ROW_HEIGHT}
              fetchPage={listing.fetchPage}
              row={trackRow}
              onDrawn={noteDrawn}
              onActivate={(index) => act(listing.play(index))}
            />
          {/key}
        {/if}
      {/if}
    </main>
  </div>
  {#if queueOpen}
    <Queue rows={queue} current={playing.index} total={playing.queue_len} onJump={(place) => act(jumpTo(place))} onClose={() => (queueOpen = false)} />
  {/if}
</div>

<NowPlaying
  player={shown}
  {addonName}
  onPause={(paused) => act(setPaused(paused))}
  onNext={() => act(nextTrack())}
  onPrevious={() => act(previousTrack())}
  onSeek={(seconds) => act(seekTo(seconds))}
  onVolume={(percent) => act(setVolume(percent))}
  onQueue={() => (queueOpen = !queueOpen)}
  {queueOpen}
/>

{#if soundOpen}
  <Sound player={playing} onExclusive={(exclusive) => act(setExclusive(exclusive))} onClose={() => (soundOpen = false)} />
{/if}

{#if addonsOpen}
  <Addons {addons} onChange={(changed) => (addons = changed)} onClose={() => (addonsOpen = false)} />
{/if}

{#if menu}
  <RowMenu
    x={menu.x}
    y={menu.y}
    {playlists}
    place={menuPlace}
    onAdd={addTo}
    onCreate={addToNew}
    onMove={moveBy}
    onRemove={takeOut}
    onClose={() => (menu = null)}
  />
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
  input[type="search"],
  input[type="text"] {
    padding: 7px 12px;
    border-radius: 8px;
    border: 1px solid var(--line);
    background: var(--field);
    color: inherit;
    font: inherit;
  }
  input[type="search"] {
    flex: 0 1 360px;
  }
  input:focus-visible,
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .counts {
    margin: 0 0 0 auto;
    color: var(--quiet);
    font-variant-numeric: tabular-nums;
  }
  button {
    font: inherit;
    color: var(--text);
    cursor: pointer;
  }
  .plain-button {
    padding: 6px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--field);
  }
  .plain-button:hover:not(:disabled) {
    background: var(--hover);
  }
  .plain-button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  :global(.act) {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--quiet);
    cursor: pointer;
  }
  :global(.act:hover) {
    background: var(--line);
    color: var(--text);
  }
  :global(.act svg) {
    width: 15px;
    height: 15px;
    fill: currentColor;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  nav {
    flex: 0 0 216px;
    display: flex;
    flex-direction: column;
    min-height: 0;
    padding: 0 8px 8px;
    border-right: 1px solid var(--line);
  }
  .place {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    text-align: left;
  }
  .place span:first-child {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .place:hover {
    background: var(--hover);
  }
  .place.here {
    background: var(--field);
    font-weight: 600;
  }
  .count {
    color: var(--quiet);
    font-variant-numeric: tabular-nums;
    font-weight: 400;
  }
  .heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 2px 2px 0;
  }
  .naming {
    padding: 2px 0 6px;
  }
  .naming input {
    width: 100%;
    box-sizing: border-box;
  }
  .lists {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .bring {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: auto;
    padding-top: 8px;
  }
  .bring .scanning {
    color: var(--soft);
    font-size: 12px;
  }
  .bring p {
    margin: 0 2px;
    color: var(--quiet);
    font-size: 11px;
  }
  .content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    padding: 6px 8px 6px 16px;
    border-bottom: 1px solid var(--line);
    background: var(--field);
  }
  .notice span {
    flex: 1;
  }
  .notice.trouble {
    color: var(--warn);
  }
  .about,
  .about form {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .about {
    padding: 10px 16px;
    border-bottom: 1px solid var(--line);
  }
  .about h2 {
    padding: 0;
    font-size: 16px;
    font-weight: 650;
    color: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .about .count,
  .about .ask {
    margin-right: auto;
  }
  .about form {
    flex: 1;
  }
  .about form input {
    flex: 0 1 320px;
  }
  .columns,
  :global(.row) {
    display: grid;
    grid-template-columns: 64px minmax(0, 3fr) minmax(0, 2fr) minmax(0, 2fr) 110px 56px 60px;
    gap: 12px;
    align-items: center;
    padding: 0 16px;
  }
  .columns {
    flex: 0 0 30px;
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
  nav h2 {
    padding: 6px 8px;
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
  :global(.row .format.quiet) {
    color: var(--quiet);
  }
  :global(.row .title.now) {
    color: var(--accent);
    font-weight: 600;
  }
  :global(.row .acts) {
    display: flex;
    justify-content: flex-end;
    gap: 2px;
    overflow: visible;
  }
  /* The buttons beside a song show when the row is pointed at or one of them has the keyboard; a like always shows. */
  :global(.row .act) {
    opacity: 0;
  }
  :global(.row:hover .act),
  :global(.row .act:focus-visible),
  :global(.row .act.on) {
    opacity: 1;
  }
  :global(.row .act.on) {
    color: var(--accent);
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
