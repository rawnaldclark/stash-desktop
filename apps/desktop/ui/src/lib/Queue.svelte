<script lang="ts">
  // What is playing and what comes after it, in order. A double-click plays from that place on.
  import type { QueueRow } from "./api";

  type Props = {
    rows: QueueRow[];
    /** The place of the track that is playing. */
    current: number | null;
    /** How many tracks the queue holds in all (the panel shows the first few hundred). */
    total: number;
    onJump: (place: number) => void;
    onClose: () => void;
  };

  let { rows, current, total, onJump, onClose }: Props = $props();

  let list: HTMLOListElement | undefined = $state();

  // The playing track is kept in view as the queue moves on.
  $effect(() => {
    void rows.length;
    if (current === null) return;
    list?.querySelector<HTMLElement>(`[data-place="${current}"]`)?.scrollIntoView({ block: "nearest" });
  });
</script>

<aside aria-label="The queue">
  <header>
    <h2>Queue</h2>
    <span class="count">{total === 1 ? "1 track" : `${total} tracks`}</span>
    <button type="button" class="act" onclick={onClose} aria-label="Close the queue">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.4 5 12 10.6 17.6 5 19 6.4 13.4 12l5.6 5.6-1.4 1.4-5.6-5.6L6.4 19 5 17.6 10.6 12 5 6.4z" /></svg>
    </button>
  </header>
  {#if rows.length === 0}
    <p class="empty">Nothing is queued. Double-click a track to play from there.</p>
  {:else}
    <ol bind:this={list}>
      {#each rows as row (row.place)}
        <li data-place={row.place} class:now={row.place === current} class:played={current !== null && row.place < current} ondblclick={() => onJump(row.place)}>
          <span class="n">{row.place + 1}</span>
          <span class="what">
            <span class="title">{row.title}</span>
            <span class="artist">{row.artist}</span>
          </span>
        </li>
      {/each}
    </ol>
    {#if total > rows.length}
      <p class="empty">And {total - rows.length} more.</p>
    {/if}
  {/if}
</aside>

<style>
  aside {
    flex: 0 0 300px;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--line);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 8px 8px 14px;
    border-bottom: 1px solid var(--line);
  }
  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 650;
  }
  .count {
    margin-right: auto;
    color: var(--quiet);
    font-size: 12px;
  }
  ol {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 4px 0;
    overflow-y: auto;
    list-style: none;
  }
  li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 14px;
    cursor: default;
  }
  li:hover {
    background: var(--hover);
  }
  li.played {
    opacity: 0.55;
  }
  .n {
    flex: 0 0 28px;
    color: var(--quiet);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .what {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .what span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .artist {
    color: var(--quiet);
    font-size: 12px;
  }
  li.now .title {
    color: var(--accent);
    font-weight: 600;
  }
  .empty {
    margin: 0;
    padding: 14px;
    color: var(--quiet);
  }
</style>
