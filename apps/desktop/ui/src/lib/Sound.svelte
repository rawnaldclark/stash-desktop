<script lang="ts">
  // How the sound leaves the computer: shared with other programs through the system's mixer, or to the sound card
  // for Stash alone. The choice is the listener's, and what it does is said in full.
  import type { PlayerState } from "./api";

  type Props = {
    player: PlayerState;
    onExclusive: (exclusive: boolean) => void;
    onClose: () => void;
  };

  let { player, onExclusive, onClose }: Props = $props();

  let dialog: HTMLDialogElement | undefined = $state();

  $effect(() => {
    if (dialog && !dialog.open) dialog.showModal();
  });

  function kHz(rate: number | null): string {
    return rate ? `${Math.round(rate / 100) / 10} kHz` : "";
  }

  /** What the sound card is being sent right now, in a sentence. */
  const now = $derived.by(() => {
    const signal = player.signal;
    if (!signal || !signal.output.sample_rate) return null;
    const how = signal.mode === "exclusive" ? "for Stash alone" : signal.mode === "shared" ? "shared with other programs" : "";
    const kept = !signal.resampled && !signal.converted;
    return `Right now the sound card is open at ${kHz(signal.output.sample_rate)}${how ? `, ${how}` : ""}. The track is ${kept ? "going out as it is" : "being changed on the way"} (${kHz(signal.decoded.sample_rate)} in).`;
  });
</script>

<dialog bind:this={dialog} onclose={onClose} aria-labelledby="sound-title">
  <header>
    <h2 id="sound-title">Sound</h2>
    <button type="button" class="act" onclick={() => dialog?.close()} aria-label="Close">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6.4 5 12 10.6 17.6 5 19 6.4 13.4 12l5.6 5.6-1.4 1.4-5.6-5.6L6.4 19 5 17.6 10.6 12 5 6.4z" /></svg>
    </button>
  </header>

  {#if !player.available}
    <p>No sound: {player.reason ?? "the player is not available"}.</p>
  {:else}
    <label>
      <input type="checkbox" checked={player.exclusive} onchange={(e) => onExclusive(e.currentTarget.checked)} />
      <span>Take the sound card for Stash alone</span>
    </label>
    <p>
      Each track then goes to the sound card at its own sample rate, with the system's mixer out of the way. While
      Stash plays, other programs make no sound. If the sound card cannot take a track's format, the nearest one it
      can take is used, and the bar at the bottom says so.
    </p>
    <p>
      Off, the system mixes Stash with everything else at one fixed rate, and tracks at another rate are resampled.
    </p>
    <p>The volume here changes the samples themselves. To send a track out untouched, leave it at 100.</p>
    {#if now}
      <p class="now">{now}</p>
    {/if}
  {/if}
</dialog>

<style>
  dialog {
    width: min(520px, calc(100vw - 48px));
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
  label {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
  }
  input {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }
  p {
    margin: 10px 0 0;
    color: var(--soft);
  }
  .now {
    padding-top: 10px;
    border-top: 1px solid var(--line);
    color: var(--quiet);
  }
</style>
