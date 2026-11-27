<!-- React: the 17 text faces, each over its name ("/shrug"), the five big ones in their own row (they play over everyone's
     screen), the quick phrases, and a line to type a face's command ("/shrug", or the start of one: "/tab"). Esc closes it. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { FACES, faceFromCommand, faceName, faceText, isBigFace } from './faces';
	import { PHRASES, phraseText, type PhraseId } from './words';

	let { onface, onphrase, onclose }: { onface: (id: string) => void; onphrase: (id: PhraseId) => void; onclose: () => void } = $props();

	const small = FACES.filter((f) => !isBigFace(f));
	const big = FACES.filter(isBigFace);
	let typed = $state('');
	const match = $derived(typed.trim() ? faceFromCommand(typed) : null);
	let box = $state<HTMLDivElement>();

	onMount(() => {
		box?.querySelector<HTMLButtonElement>('button')?.focus({ preventScroll: true });
		box?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
	});

	function send(e: SubmitEvent) {
		e.preventDefault();
		if (!match) return;
		onface(match);
		typed = '';
	}
	function key(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.stopPropagation();
			onclose();
		}
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="ss-tray" role="dialog" aria-label="React" tabindex="-1" bind:this={box} onkeydown={key}>
	<p class="ss-tray-l">Faces</p>
	<div class="ss-tray-row">
		{#each small as f (f)}
			<button type="button" class="ss-face" aria-label={faceName(f).slice(1)} onclick={() => onface(f)}><b aria-hidden="true">{faceText(f)}</b><small aria-hidden="true">{faceName(f)}</small></button>
		{/each}
	</div>
	<p class="ss-tray-l">Big ones · play over everyone’s screen</p>
	<div class="ss-tray-row">
		{#each big as f (f)}
			<button type="button" class="ss-face" aria-label={faceName(f).slice(1)} onclick={() => onface(f)}><b aria-hidden="true">{faceText(f)}</b><small aria-hidden="true">{faceName(f)}</small></button>
		{/each}
	</div>
	<p class="ss-tray-l">Say</p>
	<div class="ss-phr">
		{#each PHRASES as p (p)}
			<button type="button" onclick={() => onphrase(p)}>{phraseText(p)}</button>
		{/each}
	</div>
	<form class="ss-cmd" onsubmit={send}>
		<input type="text" bind:value={typed} placeholder="Type a face: /shrug" aria-label="Type a face's name" autocomplete="off" spellcheck="false" maxlength="20" />
		<output aria-live="polite">{match ? faceText(match) : typed.trim() ? 'No such face' : ''}</output>
	</form>
</div>
