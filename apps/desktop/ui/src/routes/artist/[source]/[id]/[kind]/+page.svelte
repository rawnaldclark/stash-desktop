<!-- All of an artist's albums, or all their singles and EPs: what "Show all" on their page opens. -->
<script lang="ts">
	import { page } from '$app/state';
	import { browse } from '../../../../../lib/app';
	import type { Discography } from '../../../../../lib/sources/contract';
	import { Refused } from '../../../../../lib/core';
	import { albumHref, artistHref } from '../../../../../lib/identity';
	import BrowseMsg from '../../../../../lib/ui/BrowseMsg.svelte';
	import Card from '../../../../../lib/ui/Card.svelte';

	const sid = $derived(page.params.source ?? '');
	const id = $derived(page.params.id ?? '');
	const kind = $derived<'albums' | 'singles'>(page.params.kind === 'singles' ? 'singles' : 'albums');
	let list = $state.raw<Discography | null>(null);
	let status = $state<'loading' | 'ok' | 'missing' | 'failed'>('loading');
	let seq = 0;

	function load() {
		const mine = ++seq;
		status = 'loading';
		list = null;
		browse.discography(sid, id, kind).then(
			(d) => {
				if (mine !== seq) return;
				list = d;
				status = 'ok';
			},
			(e) => {
				if (mine !== seq) return;
				status = e instanceof Refused && e.status === 404 ? 'missing' : 'failed';
			}
		);
	}
	$effect(() => {
		sid;
		id;
		kind;
		load();
	});
	const what = $derived(kind === 'singles' ? 'Singles and EPs' : 'Albums');
</script>

<div class="view view-in view-artist">
	{#if status !== 'ok' || !list}
		<BrowseMsg kind="artist" state={status === 'ok' ? 'loading' : status} retry={load} />
	{:else}
		<p class="caps"><a class="lnk" href={artistHref({ sourceId: sid, id })}>{list.name}</a></p>
		<h1 class="alb-title disco-title">{what}</h1>
		{#if !list.whole}
			<p class="s-hint">YouTube Music gave only part of the list: {list.albums.length === 1 ? 'this one' : `these ${list.albums.length}`}.</p>
		{/if}
		{#if list.albums.length}
			<div class="cards">
				{#each list.albums as al (al.id)}
					<Card title={al.title} sub={al.year ? String(al.year) : ''} artwork={al.artwork} href={albumHref({ sourceId: sid, id: al.id })} />
				{/each}
			</div>
		{:else}
			<p class="s-hint">None.</p>
		{/if}
	{/if}
</div>

<style>
	.disco-title { margin: 4px 0 20px; }
</style>
