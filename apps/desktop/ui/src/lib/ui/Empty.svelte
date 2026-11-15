<!-- The empty state: one calm line and a search box. No demo music in here. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import Icon from './Icon.svelte';
	import { ui } from './state.svelte';

	let { line = 'Search for a song to begin.' }: { line?: string } = $props();
	let q = $state('');
</script>

<section class="empty" aria-label="Get started">
	<p class="empty-line">{line}</p>
	<form
		class="empty-search"
		role="search"
		onsubmit={(e) => {
			e.preventDefault();
			ui.q = q;
			void goto(q.trim() ? `/search?q=${encodeURIComponent(q.trim())}` : '/search');
		}}
	>
		<Icon name="search" />
		<input type="search" bind:value={q} placeholder="Search songs, albums and artists" aria-label="Search" autocomplete="off" />
	</form>
</section>
