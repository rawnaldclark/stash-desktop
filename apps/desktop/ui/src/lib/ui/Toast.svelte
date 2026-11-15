<!-- A short, calm message (a song that could not be played, "Added to Up next"). One polite live region. -->
<script lang="ts">
	import { ui } from './state.svelte';
</script>

<div class="toast-wrap" role="status" aria-live="polite">
	{#if ui.toastText}<p class="toast">{ui.toastText}</p>{/if}
</div>
