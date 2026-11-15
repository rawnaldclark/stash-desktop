<script lang="ts">
  // What can be done with a song besides playing and liking it: put it in a playlist (one that is there, or a new
  // one), and, in a playlist, move it or take it out.
  import type { PlaylistRow } from "./api";

  type Props = {
    /** Where the button that opened the menu is. */
    x: number;
    y: number;
    playlists: PlaylistRow[];
    /** Set when the song is shown in a playlist that can be edited: whether it is the first or the last there. */
    place: { first: boolean; last: boolean } | null;
    onAdd: (playlist: number) => void;
    onCreate: (name: string) => void;
    onMove: (by: number) => void;
    onRemove: () => void;
    onClose: () => void;
  };

  let { x, y, playlists, place, onAdd, onCreate, onMove, onRemove, onClose }: Props = $props();

  let name = $state("");
  let menu: HTMLDivElement | undefined = $state();
  /** A shared mix someone else owns is theirs to fill. */
  const own = $derived(playlists.filter((p) => !p.followed));
  /** Opens upward from a button in the lower half of the window, so it never runs off the bottom. */
  const upward = $derived(y > window.innerHeight / 2);

  $effect(() => {
    menu?.querySelector<HTMLElement>("button, input")?.focus();
  });

  function create(event: SubmitEvent) {
    event.preventDefault();
    if (name.trim() !== "") onCreate(name);
  }

  function outside(event: MouseEvent) {
    if (menu && !menu.contains(event.target as Node)) onClose();
  }
</script>

<svelte:window onmousedown={outside} onkeydown={(e) => e.key === "Escape" && onClose()} />

<div
  class="menu"
  role="menu"
  bind:this={menu}
  style:right="{Math.max(8, window.innerWidth - x)}px"
  style:top={upward ? "auto" : `${y}px`}
  style:bottom={upward ? `${window.innerHeight - y}px` : "auto"}
>
  <p>Add to a playlist</p>
  <div class="own">
    {#each own as playlist (playlist.id)}
      <button type="button" role="menuitem" onclick={() => onAdd(playlist.id)}>{playlist.name}</button>
    {/each}
  </div>
  <form onsubmit={create}>
    <input type="text" placeholder="A new playlist" bind:value={name} maxlength="100" aria-label="The new playlist's name" />
    <button type="submit" disabled={name.trim() === ""}>Make</button>
  </form>
  {#if place}
    <hr />
    <button type="button" role="menuitem" disabled={place.first} onclick={() => onMove(-1)}>Move up</button>
    <button type="button" role="menuitem" disabled={place.last} onclick={() => onMove(1)}>Move down</button>
    <button type="button" role="menuitem" onclick={onRemove}>Take out of this playlist</button>
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 10;
    width: 240px;
    padding: 6px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--field);
    box-shadow: 0 8px 28px rgb(0 0 0 / 0.35);
  }
  p {
    margin: 4px 8px 6px;
    color: var(--quiet);
    font-size: 12px;
  }
  .own {
    max-height: 220px;
    overflow-y: auto;
  }
  button {
    font: inherit;
    color: var(--text);
    cursor: pointer;
  }
  .menu > button,
  .own button {
    display: block;
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu > button:hover:not(:disabled),
  .own button:hover {
    background: var(--hover);
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  button:focus-visible,
  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  form {
    display: flex;
    gap: 6px;
    padding: 6px 2px 2px;
  }
  input {
    flex: 1;
    min-width: 0;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--bg);
    color: inherit;
    font: inherit;
  }
  form button {
    padding: 5px 10px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--bg);
  }
  hr {
    margin: 6px 0;
    border: none;
    border-top: 1px solid var(--line);
  }
</style>
