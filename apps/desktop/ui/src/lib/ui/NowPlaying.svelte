<!-- The big lyrics' left column: the cover, title, format and signal path, actions, progress and transport. The lyric column is the panel's. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { albumHref, albumLink, artistLink, LIB_REF } from '../identity';
	import Cover from './Cover.svelte';
	import Credit from './Credit.svelte';
	import Icon from './Icon.svelte';
	import LikeButton from './LikeButton.svelte';
	import SaveButton from './SaveButton.svelte';
	import ProgressBar from './ProgressBar.svelte';
	import SignalBadge from './SignalBadge.svelte';
	import Transport from './Transport.svelte';
	import Volume from './Volume.svelte';
	import { origin, VERDICTS } from './signal';
	import { L, S, ui } from './state.svelte';

	const s = $derived(S.v);
	const next = $derived(s.queue[s.index - s.queueStart + 1]);
	// Artist and album link to their pages when the song has them (a song of the library); the artist falls back to a search, the album stays text.
	const artistTo = $derived(s.current ? artistLink(s.current) : null);
	const albumTo = $derived(s.current ? albumLink(s.current) : null);
	/** An add-on's stream: the add-on's name and what it said the stream is, and whether the sound agrees. */
	const addonId = $derived(s.servedBy ?? (s.current ? Object.keys(s.current.refs).find((k) => k !== LIB_REF) : undefined) ?? null);
	const from = $derived(s.servedBy ? origin(L.sources.find((a) => a.id === addonId)?.name ?? 'An add-on', s.tier, s.signal, s.sourceBitDepth) : null);
	/** While an add-on is asked where the song plays from. */
	const asking = $derived(s.buffering ? (L.sources.find((a) => a.id === addonId)?.name ?? null) : null);

	function close() {
		if (history.length > 1) history.back();
		else void goto('/');
	}
</script>

<div class="fs-amb" aria-hidden="true">
	<i class="orb o1"><b></b></i><i class="orb o2"><b></b></i><i class="orb o3"><b></b></i>
</div>

<div class="fsl">
	<div class="fs-icons">
		<button type="button" class="fs-ic" aria-label="Close Now playing" title="Close" onclick={close}><Icon name="close_fullscreen" /></button>
	</div>
	{#if s.current}
		<div class="fs-t">
			<div class="fs-art" aria-hidden="true"><Cover artwork={s.current.artwork} seed={s.current.title} min={720} class="cv fs-cov" eager /></div>
			<div class="fs-meta">
				<h2 class="fs-title" dir="auto">{s.current.title}</h2>
				<p class="fs-artist" dir="auto">
					<Credit credit={s.current.artist} to={artistTo} search />{#if albumTo && s.current.album}<span class="sep"> · </span><a href={albumHref(albumTo)} title="Go to {s.current.album}">{s.current.album}</a>{/if}
				</p>
				{#if s.buffering}
					<p class="fs-from" role="status">{asking ? `Asking ${asking} for the stream…` : 'Asking your add-ons for this song…'}</p>
				{:else}
					<SignalBadge kind="fs" />
					{#if from}<p class="fs-from" style={from.disagrees ? `color:${VERDICTS.resampled.color}` : undefined}>{from.text}</p>{/if}
				{/if}
			</div>
			<div class="fs-act">
				<LikeButton item={s.current} class="fs-ib fs-like" />
				<SaveButton item={s.current} class="fs-ib" />
				<button type="button" class="fs-ib fs-q" aria-label="Up next" title="Up next" onclick={() => ui.toggleNext()}><Icon name="queue_music" /></button>
				<Volume cls="fs-ib" class="fs-vol" />
			</div>
			{#if next}
				<div class="fs-next"><p class="caps">Up next</p><p class="fs-nx" dir="auto">{next.title} · {next.artist}</p></div>
			{/if}
		</div>
		<div class="fs-b">
			<ProgressBar class="fs-prog" />
			<Transport kind="fs" />
		</div>
	{:else}
		<p class="fs-empty">Nothing is playing. Pick a song and it lands here.</p>
	{/if}
</div>

<style>
	/* where an add-on's stream comes from, under the format badge: the artist line's quiet type, a size down */
	.fs-from { margin-top: 6px; max-width: 100%; font: 400 12px/1.4 var(--font-body); color: var(--wp-text-2); overflow: clip; text-overflow: ellipsis; white-space: nowrap; }
</style>
