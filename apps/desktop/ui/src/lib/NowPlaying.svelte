<script lang="ts">
  // The bar at the bottom: what is playing, the controls, how far in, and the signal path. The signal path says what
  // the engine reports about the sound on its way to the sound card, not what a file's label claims.
  import type { PlayerState, Signal } from "./api";

  type Props = {
    player: PlayerState;
    onPause: (paused: boolean) => void;
    onNext: () => void;
    onPrevious: () => void;
    onSeek: (seconds: number) => void;
  };

  let { player, onPause, onNext, onPrevious, onSeek }: Props = $props();

  function clock(ms: number): string {
    const seconds = Math.floor(ms / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }

  const CODECS: Record<string, string> = { flac: "FLAC", alac: "ALAC", mp3: "MP3", aac: "AAC", opus: "Opus", vorbis: "Vorbis", wavpack: "WavPack" };

  function codecName(codec: string | null): string {
    if (!codec) return "";
    if (codec.startsWith("pcm")) return "PCM";
    return CODECS[codec] ?? codec.toUpperCase();
  }

  /** "s16" → "16-bit", "floatp" → "32-bit float". */
  function sampleFormat(format: string | null): string {
    const plain = (format ?? "").replace(/p$/, "");
    if (plain === "float") return "32-bit float";
    if (plain === "double") return "64-bit float";
    const bits = /^[su](\d+)$/.exec(plain);
    return bits ? `${bits[1]}-bit` : plain;
  }

  function kHz(rate: number | null): string {
    return rate ? `${Math.round(rate / 100) / 10} kHz` : "";
  }

  function words(...parts: string[]): string {
    return parts.filter((p) => p !== "").join(" ");
  }

  /** "FLAC 16-bit 44.1 kHz → shared 32-bit float 48 kHz". */
  function path(signal: Signal): { from: string; to: string; note: string } {
    const from = words(codecName(signal.codec), sampleFormat(signal.decoded.format), kHz(signal.decoded.sample_rate));
    const to = words(signal.mode ?? signal.driver ?? "", sampleFormat(signal.output.format), kHz(signal.output.sample_rate));
    const changes = [signal.resampled ? "resampled" : "", signal.converted ? "format changed" : ""].filter((c) => c !== "");
    return { from, to, note: changes.length ? changes.join(", ") : "unchanged" };
  }

  let bar: HTMLDivElement | undefined = $state();

  function seekFrom(event: MouseEvent) {
    if (!bar || player.duration_ms <= 0) return;
    const box = bar.getBoundingClientRect();
    const share = Math.min(1, Math.max(0, (event.clientX - box.left) / box.width));
    onSeek((share * player.duration_ms) / 1000);
  }

  const progress = $derived(player.duration_ms > 0 ? Math.min(100, (player.position_ms / player.duration_ms) * 100) : 0);
  const signal = $derived(player.signal ? path(player.signal) : null);
</script>

<footer class:idle={!player.track}>
  {#if !player.available}
    <p class="notice">No sound: {player.reason ?? "the player is not available"}.</p>
  {:else if player.track}
    <div class="what">
      <span class="title">{player.track.title}</span>
      <span class="by">{player.track.artist}{player.track.album ? ` · ${player.track.album}` : ""}</span>
    </div>
    <div class="middle">
      <div class="controls">
        <button type="button" onclick={onPrevious} aria-label="Previous track">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 5h2v14H6zM20 5v14L9 12z" /></svg>
        </button>
        <button type="button" class="main" onclick={() => onPause(player.status === "playing")} aria-label={player.status === "playing" ? "Pause" : "Play"}>
          {#if player.status === "playing"}
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 5h4v14H7zM13 5h4v14h-4z" /></svg>
          {:else}
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" /></svg>
          {/if}
        </button>
        <button type="button" onclick={onNext} aria-label="Next track">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M16 5h2v14h-2zM4 5v14l11-7z" /></svg>
        </button>
      </div>
      <div class="timeline">
        <span class="time">{clock(player.position_ms)}</span>
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="track"
          bind:this={bar}
          onclick={seekFrom}
          role="slider"
          tabindex="-1"
          aria-label="Position in the track"
          aria-valuemin="0"
          aria-valuemax={Math.round(player.duration_ms / 1000)}
          aria-valuenow={Math.round(player.position_ms / 1000)}
        >
          <div class="fill" style:width="{progress}%"></div>
        </div>
        <span class="time">{clock(player.duration_ms)}</span>
      </div>
    </div>
    <div class="signal" title="What the audio engine reports between the file and the sound card">
      {#if signal}
        <span>{signal.from} → {signal.to}</span>
        <span class="note" class:changed={signal.note !== "unchanged"}>{signal.note}</span>
      {/if}
    </div>
  {:else}
    <p class="notice">{player.error ?? "Double-click a track to play it."}</p>
  {/if}
</footer>

<style>
  footer {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(280px, 2fr) minmax(0, 1fr);
    align-items: center;
    gap: 16px;
    height: 68px;
    padding: 0 16px;
    border-top: 1px solid var(--line);
    background: var(--field);
  }
  footer.idle {
    grid-template-columns: 1fr;
  }
  .notice {
    margin: 0;
    color: var(--quiet);
  }
  .what,
  .signal {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .what span,
  .signal span {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .title {
    font-weight: 600;
  }
  .by,
  .signal {
    color: var(--soft);
    font-size: 12px;
  }
  .signal {
    text-align: right;
  }
  .note {
    color: var(--quiet);
  }
  .note.changed {
    color: var(--warn);
  }
  .middle {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .controls {
    display: flex;
    justify-content: center;
    gap: 6px;
  }
  button {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text);
    cursor: pointer;
  }
  button:hover {
    background: var(--hover);
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
  }
  button.main {
    background: var(--text);
    color: var(--bg);
  }
  svg {
    width: 18px;
    height: 18px;
    fill: currentColor;
  }
  .timeline {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .time {
    width: 40px;
    color: var(--quiet);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    text-align: center;
  }
  .track {
    flex: 1;
    height: 12px;
    display: flex;
    align-items: center;
    cursor: pointer;
    background: linear-gradient(var(--line), var(--line)) center / 100% 4px no-repeat;
    border-radius: 2px;
  }
  .fill {
    height: 4px;
    background: var(--accent);
    border-radius: 2px;
  }
</style>
