<script lang="ts" generics="Row">
  // A list of any length that only ever holds the rows on screen (and a few around them). Rows come in pages from
  // `fetchPage`, asked for as the list scrolls; a row whose page has not arrived yet is drawn as a placeholder, so
  // dragging the scrollbar anywhere never waits. Every row has the same height: the position of row n is n × height.
  import type { Snippet } from "svelte";

  type Props = {
    count: number;
    rowHeight?: number;
    pageSize?: number;
    fetchPage: (offset: number, limit: number, signal: AbortSignal) => Promise<Row[]>;
    row: Snippet<[Row | undefined, number]>;
    /** Called after rows were drawn: how many of the rows on screen had their data. */
    onDrawn?: (state: { first: number; last: number; loaded: number }) => void;
  };

  let { count, rowHeight = 34, pageSize = 200, fetchPage, row, onDrawn }: Props = $props();

  const OVERSCAN = 12;
  const KEEP_PAGES = 24;

  let viewport: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let height = $state(0);
  /** Bumped whenever a page arrives, so the rows are drawn again. The pages themselves are plain data. */
  let arrivals = $state(0);

  const pages = new Map<number, Row[]>();
  const asked = new Map<number, AbortController>();

  const first = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - OVERSCAN));
  const last = $derived(Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + OVERSCAN));

  const visible = $derived.by(() => {
    void arrivals;
    const out: { index: number; data: Row | undefined }[] = [];
    for (let i = first; i < last; i++) out.push({ index: i, data: pages.get(Math.floor(i / pageSize))?.[i % pageSize] });
    return out;
  });

  $effect(() => {
    if (count === 0 || last <= first) return;
    const from = Math.floor(first / pageSize);
    const to = Math.floor((last - 1) / pageSize);
    // A page asked for a place the list has left is let go: its answer would be thrown away anyway.
    for (const [page, controller] of asked) {
      if (page < from - 1 || page > to + 1) {
        controller.abort();
        asked.delete(page);
      }
    }
    for (let page = from; page <= to; page++) {
      if (pages.has(page) || asked.has(page)) continue;
      const controller = new AbortController();
      asked.set(page, controller);
      fetchPage(page * pageSize, pageSize, controller.signal)
        .then((rows) => {
          pages.set(page, rows);
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
    if (viewport) viewport.scrollTop = index * rowHeight;
  }

  export function scrollBy(pixels: number) {
    if (viewport) viewport.scrollTop += pixels;
  }
</script>

<div
  class="viewport"
  bind:this={viewport}
  bind:clientHeight={height}
  onscroll={() => (scrollTop = viewport?.scrollTop ?? 0)}
  role="list"
>
  <div class="spacer" style:height="{count * rowHeight}px">
    <div class="rows" style:transform="translateY({first * rowHeight}px)">
      {#each visible as item (item.index)}
        <div class="row" role="listitem" style:height="{rowHeight}px">{@render row(item.data, item.index)}</div>
      {/each}
    </div>
  </div>
</div>

<style>
  .viewport {
    height: 100%;
    overflow-y: auto;
    contain: strict;
  }
  .spacer {
    position: relative;
    width: 100%;
  }
  .rows {
    position: absolute;
    inset: 0 0 auto 0;
    will-change: transform;
  }
  .row {
    contain: layout paint style;
    overflow: hidden;
  }
</style>
