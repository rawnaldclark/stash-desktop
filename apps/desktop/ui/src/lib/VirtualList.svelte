<script lang="ts" generics="Row">
  // A list of any length that only ever holds the rows on screen (and a few around them). Rows come in pages from
  // `fetchPage`, asked for as the list scrolls; a row whose page has not arrived yet is drawn as a placeholder, so
  // dragging the scrollbar anywhere never waits. Every row has the same height: the position of row n is n × height.
  //
  // The rows on screen are a fixed set of boxes that are used again: when the list scrolls, the box of a row that left
  // is given the row that arrived (its text and picture change in place) and moved there. Nothing is built or thrown
  // away while scrolling, which is what keeps a fast scroll on time when a row is more than a line of text.
  //
  // The list scrolls by itself (it fills its parent), or, given `scroller`, it lies in a page that scrolls (the
  // window's main column): it is then as tall as all its rows and draws the ones the page shows.
  import type { Snippet } from "svelte";

  type Props = {
    count: number;
    rowHeight?: number;
    pageSize?: number;
    fetchPage: (offset: number, limit: number, signal: AbortSignal) => Promise<Row[]>;
    row: Snippet<[Row | undefined, number]>;
    /** The element that scrolls, when the list does not scroll by itself. */
    scroller?: HTMLElement | null;
    /** Called after rows were drawn: how many of the rows on screen had their data. */
    onDrawn?: (state: { first: number; last: number; loaded: number }) => void;
    /** A row was double-clicked. */
    onActivate?: (index: number, data: Row | undefined) => void;
  };

  let { count, rowHeight = 34, pageSize = 200, fetchPage, row, scroller = null, onDrawn, onActivate }: Props = $props();

  const OVERSCAN = 12;
  const KEEP_PAGES = 24;

  let viewport: HTMLDivElement | undefined = $state();
  /** How far the list is scrolled: its own scroll, or how far its top is above the top of the page's scroller. */
  let scrollTop = $state(0);
  let height = $state(0);
  /** Where the list begins inside the page's scroller (what is above it there: a heading, the tabs). */
  let offsetTop = 0;
  /** Bumped whenever a page arrives, so the rows are drawn again. The pages themselves are plain data. */
  let arrivals = $state(0);

  /** Bumped to read the rows again. */
  let epoch = $state(0);

  const pages = new Map<number, Row[]>();
  /** Pages whose rows stay on screen while they are read again. */
  const stale = new Set<number>();
  const asked = new Map<number, AbortController>();

  const first = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN));
  const last = $derived(Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + OVERSCAN));

  /** How many boxes there are: never fewer than the rows drawn at once, and the same until the window is resized. */
  const slots = $derived(Math.ceil(height / rowHeight) + 2 * OVERSCAN + 2);

  /** The rows drawn, in the order of their boxes: row n is always in box n mod `slots`, so a box changes rows only when its row leaves. */
  const visible = $derived.by(() => {
    void arrivals;
    const out: { slot: number; index: number; data: Row | undefined }[] = [];
    for (let i = first; i < last; i++) out.push({ slot: i % slots, index: i, data: pages.get(Math.floor(i / pageSize))?.[i % pageSize] });
    return out.sort((a, b) => a.slot - b.slot);
  });

  /** Which way the list last moved: the page beyond the rows drawn, on that side, is asked for before it is needed. */
  let before = 0;
  let down = true;

  // In a page that scrolls: the list follows the page's scroller.
  $effect(() => {
    const outer = scroller;
    const own = viewport;
    if (!outer || !own) return;
    const place = () => {
      offsetTop = own.getBoundingClientRect().top - outer.getBoundingClientRect().top + outer.scrollTop;
      height = outer.clientHeight;
      scrollTop = Math.max(0, outer.scrollTop - offsetTop);
    };
    const scrolled = () => (scrollTop = Math.max(0, outer.scrollTop - offsetTop));
    place();
    outer.addEventListener("scroll", scrolled, { passive: true });
    // What is above the list can change its height (a line wraps, a notice appears), and the window can be resized.
    const resized = new ResizeObserver(place);
    resized.observe(outer);
    if (own.parentElement) resized.observe(own.parentElement);
    return () => {
      outer.removeEventListener("scroll", scrolled);
      resized.disconnect();
    };
  });

  $effect(() => {
    void epoch;
    if (count === 0 || last <= first) return;
    const from = Math.floor(first / pageSize);
    const to = Math.floor((last - 1) / pageSize);
    if (first !== before) down = first > before;
    before = first;
    const lastPage = Math.floor((count - 1) / pageSize);
    const ahead = down ? Math.min(lastPage, to + 1) : Math.max(0, from - 1);
    const wanted = [...Array.from({ length: to - from + 1 }, (_, i) => from + i), ...(ahead < from || ahead > to ? [ahead] : [])];
    // A page asked for a place the list has left is let go: its answer would be thrown away anyway.
    for (const [page, controller] of asked) {
      if (page < from - 1 || page > to + 1) {
        controller.abort();
        asked.delete(page);
      }
    }
    for (const page of wanted) {
      if ((pages.has(page) && !stale.has(page)) || asked.has(page)) continue;
      const controller = new AbortController();
      asked.set(page, controller);
      fetchPage(page * pageSize, pageSize, controller.signal)
        .then((rows) => {
          pages.set(page, rows);
          stale.delete(page);
          // Far-away pages are dropped, the farthest first, so memory does not grow with scrolling.
          if (pages.size > KEEP_PAGES) {
            const farthest = [...pages.keys()].sort((a, b) => Math.abs(b - page) - Math.abs(a - page))[0];
            if (farthest !== page) pages.delete(farthest);
          }
          arrivals++;
        })
        .catch(() => {})
        .finally(() => {
          if (asked.get(page) === controller) asked.delete(page);
        });
    }
  });

  $effect(() => {
    void visible;
    if (!onDrawn) return;
    const firstOnScreen = Math.floor(scrollTop / rowHeight);
    const lastOnScreen = Math.min(count, Math.ceil((scrollTop + height) / rowHeight));
    let loaded = 0;
    for (let i = firstOnScreen; i < lastOnScreen; i++) if (pages.get(Math.floor(i / pageSize))?.[i % pageSize] !== undefined) loaded++;
    onDrawn({ first: firstOnScreen, last: lastOnScreen, loaded });
  });

  /** Scrolls so that row `index` is at the top. */
  export function scrollToIndex(index: number) {
    if (scroller) scroller.scrollTop = offsetTop + index * rowHeight;
    else if (viewport) viewport.scrollTop = index * rowHeight;
  }

  /** Reads the rows again where the list is (something in them changed); what is on screen stays until it arrives. */
  export function refresh() {
    for (const page of pages.keys()) stale.add(page);
    epoch++;
  }

  /** Draws the rows again from the data the list has (a row was changed in place). */
  export function redraw() {
    arrivals++;
  }

  export function scrollBy(pixels: number) {
    if (scroller) scroller.scrollTop += pixels;
    else if (viewport) viewport.scrollTop += pixels;
  }

  // Scrolling by itself: the list's own box is measured.
  function own(node: HTMLDivElement) {
    const measure = () => {
      if (!scroller) height = node.clientHeight;
    };
    const resized = new ResizeObserver(measure);
    resized.observe(node);
    measure();
    return { destroy: () => resized.disconnect() };
  }
</script>

<div
  class="viewport"
  class:paged={!!scroller}
  bind:this={viewport}
  use:own
  onscroll={() => {
    if (!scroller) scrollTop = viewport?.scrollTop ?? 0;
  }}
  role="list"
>
  <div class="spacer" style:height="{count * rowHeight}px">
    {#each visible as item (item.slot)}
      <div
        class="row"
        role="listitem"
        aria-posinset={item.index + 1}
        aria-setsize={count}
        style:height="{rowHeight}px"
        style:transform="translateY({item.index * rowHeight}px)"
        ondblclick={() => onActivate?.(item.index, item.data)}
      >
        {@render row(item.data, item.index)}
      </div>
    {/each}
  </div>
</div>

<style>
  .viewport {
    height: 100%;
    overflow-y: auto;
    contain: strict;
  }
  /* in a page that scrolls, the list is as tall as its rows and the page does the scrolling */
  .viewport.paged {
    height: auto;
    overflow: visible;
    contain: layout style;
  }
  .spacer {
    position: relative;
    width: 100%;
  }
  /* each box is placed by itself (its row's place in the list), so a box can change rows without the others moving */
  .row {
    position: absolute;
    inset: 0 0 auto 0;
    contain: layout paint style;
    overflow: hidden;
  }
</style>
