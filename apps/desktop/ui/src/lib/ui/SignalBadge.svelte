<!-- Format and signal path of the song playing: the format badge ("FLAC · 24-bit/44.1 kHz") and the verdict word beside it
     (Bit-perfect, Lossless, Lossless + your EQ, Resampled, Lossy), from what the core's audio engine reports. While the equalizer
     changes the sound, the tooltip names what it does. `kind` 'bar' is the player bar's third line (the web player's small badge),
     'fs' the Now playing one. -->
<script lang="ts">
	import { formatLabel, verdict } from './signal';
	import { S } from './state.svelte';

	let { kind = 'bar' }: { kind?: 'bar' | 'fs' } = $props();

	const fmt = $derived(formatLabel(S.v.signal, S.v.sourceBitDepth));
	const says = $derived(verdict(S.v.signal));
	const tuned = $derived(S.v.signal?.equalized && S.v.eq.describe ? `. Your equalizer: ${S.v.eq.describe}.` : '');
	const title = $derived(fmt ? `Playing as ${fmt}${says ? ` · ${says.word}` : ''}${tuned}` : undefined);
</script>

{#if fmt}
	{@const [codec, detail] = fmt.split(' · ')}
	<span class="sig sig-{kind}" {title}>
		<span class="fmt" class:b-fmt={kind === 'bar'}>{codec}{#if detail}<span class="fmt-d">{` · ${detail}`}</span>{/if}</span>
		{#if says}<span class="sig-v" style="color:{says.color}">{says.word}</span>{/if}
	</span>
{/if}

<style>
	/* the badge is the web player's own (.fmt, .b-fmt in shell.css); the verdict word sits beside it in the same type */
	.sig { display: flex; align-items: center; gap: 6px; min-width: 0; max-width: 100%; }
	.sig .fmt { flex: 0 1 auto; min-width: 0; }
	.sig-v { flex: none; font: 600 11px/1.4 var(--font-body); letter-spacing: .02em; white-space: nowrap; }
	.sig-bar { margin-top: 3px; }
	.sig-bar .b-fmt { margin-top: 0; }
	.sig-bar .sig-v { font-size: 10px; line-height: 15px; }
	.sig-fs { justify-content: center; margin-top: 8px; }
	/* the verdict colours are the app's, made for its dark surfaces: on the light theme they are darkened to hold their contrast
	   (vite.config.ts's themeSwitch makes this follow Settings › Appearance as well as the system) */
	@media (prefers-color-scheme: light) { .sig-v { filter: brightness(.55) saturate(1.6); } }
</style>
