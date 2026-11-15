<!-- Header: back/forward, the search box, Settings. -->
<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { measuring } from '../core';
	import Icon from './Icon.svelte';
	import { ui } from './state.svelte';

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
		<button type="button" class="hd-circ" aria-label="Back" title="Back" onclick={() => history.back()}><Icon name="chevron_left" /></button>
		<button type="button" class="hd-circ" aria-label="Forward" title="Forward" onclick={() => history.forward()}><Icon name="chevron_right" /></button>
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
	</form>
	<a class="hd-dev" href="/settings" aria-label="Settings" title="Settings"><Icon name="settings" /><span class="hd-dt">Settings</span></a>
</header>
