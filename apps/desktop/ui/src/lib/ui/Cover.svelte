<!-- A cover: the artwork when there is one, else a calm placeholder tile coloured from the title. Decorative (the row names the song). -->
<script lang="ts">
	import type { Artwork } from '../api';
	import { coreUrl } from '../core';
	import { pickArt, hueOf } from './format';

	let {
		artwork,
		seed = '',
		min = 0,
		class: cls = 'cv',
		eager = false,
		ini
	}: { artwork?: Artwork[]; seed?: string; min?: number; class?: string; eager?: boolean; /** initials drawn on the placeholder */ ini?: string } = $props();

	const url = $derived(pickArt(artwork, min));
	/** A picture the core gives (a file's own cover) is asked for by name (CORS, with the window's Origin): the core
	 *  answers no other site's picture, so a web page cannot use it. Other pictures are asked for as any page's are. */
	const mine = $derived(!!url && (url.startsWith('/') || (!!coreUrl && url.startsWith(`${coreUrl}/`))));
	/** The URL that failed to load (a failure is per URL: the next song's cover gets its own chance). */
	let failed = $state<string>();
	const hue = $derived(hueOf(seed || 'stash'));
</script>

{#if url && failed !== url}
	<!-- A new picture is a new element: a row that is given another song never shows the last one's cover while the new one loads. -->
	{#key url}
		<img class={cls} crossorigin={mine ? 'anonymous' : undefined} referrerpolicy="no-referrer" loading={eager ? 'eager' : 'lazy'} src={url} alt="" decoding="async" draggable="false" onerror={() => (failed = url)} />
	{/key}
{:else}
	<span class="{cls} cv-ph" style="--h:{hue}" data-ini={ini || undefined} aria-hidden="true"></span>
{/if}
