<!-- The heart. Filled red with a quick pop when liked; a real toggle button for screen readers. -->
<script lang="ts">
	import type { LibraryItem } from '../api';
	import Icon from './Icon.svelte';
	import { L } from './state.svelte';

	let { item, class: cls = '' }: { item: LibraryItem | null | undefined; class?: string } = $props();
	const liked = $derived(item ? L.isLiked(item) : false);
</script>

<button
	type="button"
	class="heart {cls}"
	class:on={liked}
	disabled={!item}
	aria-pressed={liked}
	aria-label={liked ? 'Unlike' : 'Like'}
	title={liked ? 'Unlike' : 'Like'}
	onclick={(e) => {
		e.stopPropagation();
		if (item) void L.toggleLike(item);
	}}
>
	<Icon name={liked ? 'favorite' : 'favorite_border'} />
</button>
