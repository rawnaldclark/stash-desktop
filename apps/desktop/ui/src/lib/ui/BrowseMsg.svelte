<!-- The loading and failure lines of an album or artist page. -->
<script lang="ts">
	let { kind, state, retry }: { kind: 'album' | 'artist'; state: 'loading' | 'missing' | 'failed'; retry: () => void } = $props();
</script>

{#if state === 'loading'}
	<p class="s-hint" role="status">Loading</p>
{:else if state === 'missing'}
	<p class="s-hint" role="status">That {kind} isn’t here. <a class="lnk" href="/search">Search instead</a></p>
{:else}
	<p class="s-hint" role="alert">This {kind} didn’t load. The Stash core isn’t answering right now. <button type="button" class="lnk" onclick={retry}>Try again</button></p>
{/if}
