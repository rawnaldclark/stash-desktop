<!-- The frosted player bar (L, M). Song (with its format and signal path), transport + progress, panel toggles, volume, Now playing. -->
<script lang="ts">
	import Cover from './Cover.svelte';
	import Icon from './Icon.svelte';
	import LikeButton from './LikeButton.svelte';
	import ProgressBar from './ProgressBar.svelte';
	import SignalBadge from './SignalBadge.svelte';
	import Transport from './Transport.svelte';
	import Volume from './Volume.svelte';
	import { S, ui } from './state.svelte';

	const s = $derived(S.v);
</script>

{#if s.current}
	<div class="bar" role="group" aria-label="Player">
		<div class="bar-l">
			<a class="bar-cv" href="/now" aria-label="Open Now playing"><Cover artwork={s.current.artwork} seed={s.current.title} min={104} class="cv" eager /></a>
			<span class="bar-t"><span class="bar-ti"><b dir="auto">{s.current.title}</b><i dir="auto">{s.current.artist}</i></span><SignalBadge kind="bar" /></span>
			<LikeButton item={s.current} class="bar-ib bar-like" />
		</div>
		<div class="bar-c">
			<Transport kind="bar" />
			<ProgressBar />
		</div>
		<div class="bar-r">
			<button type="button" class="bar-ib b-lyr" class:open={ui.panelOpen && ui.panelTab === 'lyrics'} aria-pressed={ui.panelOpen && ui.panelTab === 'lyrics'} aria-label="Lyrics" title="Lyrics" onclick={() => ui.toggleTab('lyrics')}><Icon name="lyrics" /></button>
			<button type="button" class="bar-ib b-que" class:open={ui.panelOpen && ui.panelTab === 'next'} aria-pressed={ui.panelOpen && ui.panelTab === 'next'} aria-label="Up next" title="Up next" onclick={() => ui.toggleTab('next')}><Icon name="queue_music" /></button>
			<Volume />
			<a class="bar-ib b-full" href="/now" aria-label="Open Now playing" title="Now playing"><Icon name="open_in_full" /></a>
		</div>
	</div>
{/if}
