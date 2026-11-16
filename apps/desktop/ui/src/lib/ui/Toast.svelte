<!-- A short, calm message (a song that could not be played, "Added to Up next"). One polite live region. -->
<script lang="ts">
	import { ui } from './state.svelte';
</script>

<div class="toast-wrap" role="status" aria-live="polite">
	{#if ui.toastText}
		<p class="toast">
			{ui.toastText}
			{#if ui.toastAction}
				<button
					type="button"
					class="toast-act"
					onclick={() => {
						// Taken before the message goes: the action goes with it.
						const run = ui.toastAction?.run;
						ui.toastText = null;
						ui.toastAction = null;
						run?.();
					}}>{ui.toastAction.label}</button
				>
			{/if}
		</p>
	{/if}
</div>

<style>
	.toast-act { margin-left: 12px; padding: 2px 4px; border-radius: 6px; color: var(--acc-text, inherit); font: 600 13px/1.4 var(--font-body); pointer-events: auto; }
	@media (hover: hover) and (pointer: fine) { .toast-act:hover { text-decoration: underline; } }
	.toast-act:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
