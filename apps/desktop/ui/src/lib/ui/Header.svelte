<!-- Header: back/forward, the search box (Settings is in the navigation, as on Android). -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { measuring } from '../core';
	import Icon from './Icon.svelte';
	import { ui } from './state.svelte';
	import { goOffline } from '../app';
	import { K, withKeys } from './keys';

	let input = $state<HTMLInputElement>();
	let timer: ReturnType<typeof setTimeout> | undefined;

	function go(q: string, now = false) {
		// While the window measures itself the box filters the list in place (Library › Songs reads `ui.q`): no page change.
		if (measuring) return;
		clearTimeout(timer);
		const run = () => {
			const url = q.trim() ? `/search?q=${encodeURIComponent(q.trim())}` : '/search';
			void goto(url, { reset: false, replace: page.url.pathname === '/search' });
		};
		if (now) run();
		else timer = setTimeout(run, 280);
	}

	export function focusSearch() {
		input?.focus();
		input?.select();
	}
</script>

<header class="head">
	<span class="hd-nav">
		<button type="button" class="hd-circ" aria-label="Back" title={withKeys('Back', K.pageBack)} onclick={() => history.back()}><Icon name="chevron_left" /></button>
		<button type="button" class="hd-circ" aria-label="Forward" title={withKeys('Forward', K.pageForward)} onclick={() => history.forward()}><Icon name="chevron_right" /></button>
	</span>
	<form class="hd-search" role="search" onsubmit={(e) => { e.preventDefault(); go(ui.q, true); }}>
		<Icon name="search" />
		<input
			bind:this={input}
			type="search"
			id="q"
			name="q"
			placeholder="Search songs, albums and artists"
			aria-label="Search"
			autocomplete="off"
			spellcheck="false"
			bind:value={ui.q}
			oninput={() => go(ui.q)}
			onfocus={() => { if (page.url.pathname !== '/search') go(ui.q, true); }}
			onkeydown={(e) => { if (e.key === 'Escape') { ui.q = ''; go('', true); input?.blur(); } }}
		/>
		{#if !ui.q}<kbd class="kbd hd-key" aria-hidden="true" title="Press / to search">/</kbd>{/if}
	</form>
	{#if ui.local}<a class="hd-dev hd-off" href="/settings?c=general" aria-label="Stash plays only your own music files. Change it in Settings" title="Only your own music files: Stash stays off the internet. Click to change"><Icon name="folder" /><span class="hd-dt">My files</span></a>
	{:else if ui.offline}<button type="button" class="hd-dev hd-off" aria-label="Offline: only what is on this computer plays. Go back online" title="Offline: only what is on this computer plays. Click to go back online" onclick={() => void goOffline(false)}><Icon name="download" /><span class="hd-dt">Offline</span></button>{/if}
</header>
