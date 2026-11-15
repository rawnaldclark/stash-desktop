<script lang="ts">
  // The listener's add-ons: where music beyond their own files comes from. Stash ships with none; each is added by
  // its link. The order is the listener's: the first add-on is asked first.
  import { addAddon, orderAddons, Refused, removeAddon, setAddonEnabled, type AddonInfo } from "./api";

  type Props = {
    addons: AddonInfo[];
    onChange: (addons: AddonInfo[]) => void;
    onClose: () => void;
  };

  let { addons, onChange, onClose }: Props = $props();

  let dialog: HTMLDialogElement | undefined = $state();
  let link = $state("");
  let adding = $state(false);
  let trouble: string | null = $state(null);

  let field: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (dialog && !dialog.open) {
      dialog.showModal();
      // The link field is what the listener came for, not the close button.
      field?.focus();
    }
  });

  function said(e: unknown): string {
    if (e instanceof Refused) return e.message;
    return "The Stash core is not answering.";
  }

  async function change(call: Promise<AddonInfo[]>) {
    trouble = null;
    try {
      onChange(await call);
    } catch (e) {
      trouble = said(e);
    }
  }

  async function add(event: SubmitEvent) {
    event.preventDefault();
    const wanted = link.trim();
    if (wanted === "" || adding) return;
    adding = true;
    trouble = null;
    try {
      onChange(await addAddon(wanted));
      link = "";
    } catch (e) {
      trouble = e instanceof Refused ? `That link was not added: ${e.message}.` : said(e);
    } finally {
      adding = false;
    }
  }

  function move(index: number, by: number) {
    const ids = addons.map((a) => a.id);
    const [id] = ids.splice(index, 1);
    ids.splice(index + by, 0, id);
    change(orderAddons(ids));
  }

  /** What an add-on does, from what its manifest lists. */
  function does(addon: AddonInfo): string {
    const parts = [addon.resources.includes("search") ? "search" : "", addon.resources.includes("stream") ? "playing" : ""].filter((p) => p !== "");
    return parts.join(" and ");
  }
</script>

<dialog bind:this={dialog} onclose={onClose} aria-labelledby="addons-title">
  <header>
    <h2 id="addons-title">Add-ons</h2>
    <button type="button" class="plain" onclick={() => dialog?.close()} aria-label="Close">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.4 5 12 10.6 17.6 5 19 6.4 13.4 12l5.6 5.6-1.4 1.4-5.6-5.6L6.4 19 5 17.6 10.6 12 5 6.4z" /></svg>
    </button>
  </header>

  {#if addons.length === 0}
    <p class="about">
      Stash plays your own files and comes with no add-ons. An add-on is a server that finds music and plays it; you
      add one by the link its maker gives you.
    </p>
  {:else}
    <ol>
      {#each addons as addon, index (addon.id)}
        <li class:off={!addon.enabled}>
          <div class="who">
            <span class="name">{addon.name}</span>
            <span class="detail">
              {[addon.version ? `version ${addon.version}` : "", addon.host ?? "", does(addon)].filter((p) => p !== "").join(" · ")}
            </span>
            {#if addon.problem}
              <span class="problem">Cannot be used: {addon.problem}.</span>
            {:else if addon.description}
              <span class="detail">{addon.description}</span>
            {/if}
          </div>
          <label class="switch">
            <input type="checkbox" checked={addon.enabled} onchange={(e) => change(setAddonEnabled(addon.id, e.currentTarget.checked))} />
            On
          </label>
          <button type="button" class="plain" disabled={index === 0} onclick={() => move(index, -1)} aria-label="Ask {addon.name} earlier">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 7 7 7-1.4 1.4L12 9.8l-5.6 5.6L5 14z" /></svg>
          </button>
          <button type="button" class="plain" disabled={index === addons.length - 1} onclick={() => move(index, 1)} aria-label="Ask {addon.name} later">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 17-7-7 1.4-1.4 5.6 5.6 5.6-5.6L19 10z" /></svg>
          </button>
          <button type="button" class="remove" onclick={() => change(removeAddon(addon.id))}>Remove</button>
        </li>
      {/each}
    </ol>
    {#if addons.length > 1}
      <p class="about">The first add-on is asked first.</p>
    {/if}
  {/if}

  <form onsubmit={add}>
    <input
      type="url"
      placeholder="https://…/manifest.json"
      bind:value={link}
      bind:this={field}
      aria-label="An add-on's link"
      autocomplete="off"
      spellcheck="false"
    />
    <button type="submit" class="add" disabled={adding || link.trim() === ""}>{adding ? "Reading…" : "Add"}</button>
  </form>
  {#if trouble}
    <p class="trouble" role="alert">{trouble}</p>
  {/if}
</dialog>

<style>
  dialog {
    width: min(620px, calc(100vw - 48px));
    max-height: calc(100vh - 96px);
    padding: 20px;
    border: 1px solid var(--line);
    border-radius: 12px;
    background: var(--bg);
    color: var(--text);
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.5);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 650;
  }
  .about {
    margin: 0 0 16px;
    color: var(--soft);
  }
  ol {
    list-style: none;
    margin: 0 0 12px;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 8px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
  }
  li + li {
    border-top: 1px solid var(--line);
  }
  .who {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }
  .who span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name {
    font-weight: 600;
  }
  li.off .name {
    color: var(--quiet);
  }
  .detail {
    color: var(--quiet);
    font-size: 12px;
  }
  .problem,
  .trouble {
    color: var(--warn);
  }
  .problem {
    font-size: 12px;
  }
  .trouble {
    margin: 10px 0 0;
  }
  .switch {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--soft);
    font-size: 12px;
  }
  .switch input {
    accent-color: var(--accent);
  }
  button {
    font: inherit;
    color: var(--text);
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  button:focus-visible,
  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .plain {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
  }
  .plain:hover:not(:disabled) {
    background: var(--hover);
  }
  svg {
    width: 18px;
    height: 18px;
    fill: currentColor;
  }
  .remove,
  .add {
    padding: 6px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--field);
  }
  .remove {
    font-size: 12px;
  }
  .add:not(:disabled) {
    border-color: var(--accent);
  }
  form {
    display: flex;
    gap: 8px;
  }
  form input {
    flex: 1;
    min-width: 0;
    padding: 7px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--field);
    color: inherit;
    font: inherit;
  }
</style>
