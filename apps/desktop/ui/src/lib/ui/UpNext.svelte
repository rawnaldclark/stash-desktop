<!-- Up next: where the queue plays from and how far along it is, the song playing, then what follows. Click a row to jump to it;
     "more", a right-click or a long press for the song menu (the same as every song row); drag the handle to reorder (not while shuffle is on: the order is the shuffle's); on a touch
     screen, swipe a row past half its width to remove it. "Save as playlist" keeps the whole queue. A station growing the
     queue (a radio, or autoplay) has one line above its songs still to come: what it is like, who chose them, and Stop.
     The core's queue can be the whole library, so the window holds the song playing and the next couple of hundred (`S.v.queue`,
     from place `S.v.queueStart`); every place handed to the player is a place in the whole queue. -->
<script lang="ts">
	import { tick } from 'svelte';
	import { library, player } from '../app';
	import Cover from './Cover.svelte';
	import Icon from './Icon.svelte';
	import { mmss, songs } from './format';
	import { findingWords, nothingWords, stationLine } from './radio';
	import { reorder } from './reorder';
	import { openSongMenu, songMenu } from './song-menu';
	import { S, ui } from './state.svelte';
	import { swipe } from './swipe';

	const s = $derived(S.v);
	const upcoming = $derived(s.queue.map((q, k) => ({ q, i: s.queueStart + k })).filter((x) => x.i > s.index));
	/** Songs that follow but are not listed here (the window holds only the next ones). */
	const beyond = $derived(Math.max(0, s.queueLength - (s.queueStart + s.queue.length)));
	/** The station's line, and the row it sits above (the first of its songs still to come). */
	const line = $derived(stationLine(s.station));
	const lineAt = $derived(s.station?.from_qid != null ? `q${s.station.from_qid}` : null);
	function stopStation() {
		const words = line?.stopped ?? 'Stopped';
		void player.stopRadio().then((stopped) => stopped && ui.toast(words));
	}
	let saving = $state(false);
	let busy = $state(false);
	let name = $state('');
	let nameEl = $state<HTMLInputElement>();

	const menuOf = (q: (typeof s.queue)[number], i: number) => ({ item: q, extra: { queueIndex: i, queueLength: s.queueLength } });
	/** A swiped row goes by its song's slot id, looked up now: the queue may have moved under the finger. */
	function swiped(row: HTMLElement) {
		const k = S.v.queue.findIndex((q) => q.qid === row.dataset.qid);
		if (k < 0) return;
		const i = S.v.queueStart + k;
		if (i > S.v.index) player.removeAt(i);
	}
	function openSave() {
		name = s.from ?? 'Up next';
		saving = true;
		void tick().then(() => nameEl?.select());
	}
	async function save(e: SubmitEvent) {
		e.preventDefault();
		if (busy) return;
		busy = true;
		try {
			const items = await player.wholeQueue();
			if (!items.length) return;
			const p = await library.createPlaylist(name);
			await library.addToPlaylist(p.id, items);
			saving = false;
			ui.toast(`Saved ${songs(items.length)} as ${p.name}`);
		} catch {
			/* said by the shell (a refusal's own words) */
		} finally {
			busy = false;
		}
	}
</script>

{#snippet more(a: Parameters<typeof openSongMenu>[0], title: string)}
	<button type="button" class="more" aria-label="More for {title}" aria-haspopup="menu" onclick={(e) => openSongMenu(a, e.currentTarget)}><Icon name="more_horiz" /></button>
{/snippet}

<div class="q-blk">
	{#if s.current}
		<div class="q-head">
			<p class="q-from">
				{#if s.from}<span class="q-src" title="Playing from {s.from}">Playing from <b dir="auto">{s.from}</b></span>{/if}
				<span class="q-n">{(s.index + 1).toLocaleString()} of {s.queueLength.toLocaleString()}</span>
			</p>
			<button type="button" class="q-save" onclick={() => (saving ? (saving = false) : openSave())} aria-expanded={saving}><Icon name="playlist_add" /><span>Save as playlist</span></button>
		</div>
		{#if saving}
			<form class="mnew q-saveform" onsubmit={save}>
				<input aria-label="Playlist name" placeholder="Playlist name" bind:value={name} bind:this={nameEl} maxlength="80" onkeydown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); saving = false; } }} />
				<button type="submit" disabled={!name.trim() || busy}>Save</button>
			</form>
		{/if}
		<p class="caps">Now playing</p>
		<div class="q-row q-now" use:songMenu={{ item: s.current }}>
			<Cover artwork={s.current.artwork} seed={s.current.title} min={88} class="cv" />
			<span class="tr-t"><b dir="auto">{s.current.title}</b><i dir="auto">{s.current.artist}</i></span>
			<i class="eq" class:still={!s.playing} aria-hidden="true"><b></b><b></b><b></b></i>
			{@render more({ item: s.current }, s.current.title)}
		</div>
		<p class="caps">Next up{#if s.shuffle}<span class="q-hint"> · shuffled</span>{/if}</p>
		<div
			class="q-list"
			use:reorder={{ row: '.q-next', handle: '.rd-h', disabled: s.shuffle, onmove: (a, b) => player.move(s.index + 1 + a, s.index + 1 + b) }}
			use:swipe={{ row: '.q-next', ignore: '.rd-h, .q-x, .more', onremove: swiped }}
		>
		{#each upcoming as { q, i } (q.qid)}
			{@const away = ui.offline && q.here === false}
			{#if line && q.qid === lineAt}
				<p class="caps q-station" title={line.tip ?? undefined}>{line.lead}{#if line.lastfm}{' '}<button type="button" class="q-lnk" onclick={() => void player.openLastfm()} title="Open on Last.fm">Last.fm</button>{/if}{' · '}<button type="button" class="q-lnk" onclick={stopStation}>Stop</button></p>
			{/if}
			<div class="q-row q-next" class:has-rd={!s.shuffle} class:away data-qid={q.qid} use:songMenu={menuOf(q, i)} role="presentation" title={away ? 'Not on this computer: Stash is offline, and passes it over' : undefined}>
				{#if !s.shuffle}<button type="button" class="rd-h" aria-label="Reorder {q.title}" title="Drag to reorder, or press Arrow Up or Down" aria-keyshortcuts="ArrowUp ArrowDown"><Icon name="drag_indicator" /></button>{/if}
				<button type="button" class="q-main" onclick={() => (away ? ui.toast('Not on this computer, and Stash is offline') : player.jumpTo(i))} aria-label="Play {q.title} by {q.artist}{away ? ' (not on this computer)' : ''}">
					<Cover artwork={q.artwork} seed={q.title} min={88} class="cv" />
					<span class="tr-t"><b dir="auto">{q.title}</b><i dir="auto">{q.artist}</i></span>
					<span class="tr-d">{q.durationMs ? mmss(q.durationMs) : ''}</span>
				</button>
				{@render more(menuOf(q, i), q.title)}
				<button type="button" class="q-x" aria-label="Remove {q.title} from Up next" title="Remove" onclick={() => player.removeAt(i)}><Icon name="close" /></button>
			</div>
		{:else}
			{#if s.station?.finding}
				<p class="q-empty" role="status">{findingWords(s.station)}</p>
			{:else if s.station && !s.station.added}
				<p class="q-empty" role="status">{nothingWords(s.station)}</p>
			{:else}
				<p class="q-empty">Nothing after this one. Add songs from any list with the “more” menu.</p>
			{/if}
		{/each}
		</div>
		{#if beyond > 0 && upcoming.length}<p class="q-empty">And {songs(beyond)} more after these.</p>{/if}
	{:else}
		<p class="q-empty">Nothing is playing yet. Play a song and what follows shows up here.</p>
	{/if}
</div>
