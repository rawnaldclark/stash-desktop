<!-- A song's artists. Each one the credit names links to their own page when the library knows them by themselves
     ("Drake & Future" is two links); a name it does not know links to a search for it (`search`), or stays text. One
     artist, or none known: the whole credit, linked to its own page as before. -->
<script lang="ts">
	import { creditOf } from '../desktop/credits.svelte';
	import { artistHref, LIBRARY_SOURCE, type BrowseLink } from '../identity';

	let {
		credit,
		to = null,
		search = false,
		cls
	}: {
		/** The credit as written. */
		credit: string;
		/** The whole credit's own page, when the song has one (a song of the library). */
		to?: BrowseLink | null;
		/** Names with no page here link to a search for them. */
		search?: boolean;
		cls?: string;
	} = $props();

	const parts = $derived(creditOf(credit));
	const split = $derived(parts.some((p) => p.id !== undefined));
	const page = (id: number) => artistHref({ sourceId: LIBRARY_SOURCE, id: String(id) });
	const find = (name: string) => `/search?q=${encodeURIComponent(name)}`;
</script>

{#if split}{#each parts as p, i (i)}{p.before}{#if p.id !== undefined}<a class={cls} href={page(p.id)} title="Go to {p.name}">{p.name}</a>{:else if search}<a class={cls} href={find(p.name)} title="Search for {p.name}">{p.name}</a>{:else}{p.name}{/if}{p.after ?? ''}{/each}{:else if to}<a class={cls} href={artistHref(to)} title={search ? `Go to ${credit}` : undefined}>{credit}</a>{:else if search}<a class={cls} href={find(credit)} title="Search for {credit}">{credit}</a>{:else}{credit}{/if}
