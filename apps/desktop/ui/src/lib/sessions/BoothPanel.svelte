<!-- The booth, in the side panel while this window hosts (the mockups' B, host): how long it's been live and who's here, End (after
     this song, or now), what's on air with the room's faces rising over it, the requests waiting (Next, Queue, Pass) and
     Auto-accept, the crowd, and what they say. The music is the player's own: play, skip and queue as always. -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import Floats from './Floats.svelte';
	import { session } from './session.svelte';
	import { initialOf, personColor } from './words';

	let now = $state(Date.now());
	const tick = setInterval(() => (now = Date.now()), 15_000);
	onDestroy(() => clearInterval(tick));
	let ending = $state(false);

	const s = $derived(session.station);
	const track = $derived(session.track);
	const listeners = $derived(session.members.filter((m) => m.id !== session.hostId));
	const live = $derived.by(() => {
		const min = Math.max(0, Math.floor((now - session.startedAt) / 60_000));
		return min < 60 ? `${min} min` : `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, '0')}`;
	});
	const statsLine = $derived.by(() => {
		const st = session.stats;
		if (!st || (!st.reactions && !st.saves)) return '';
		return `${st.reactions === 1 ? '1 reaction' : `${st.reactions} reactions`} · ${st.saves} saved`;
	});
</script>

<div class="ss-room">
	{#if session.phase === 'ended'}
		<div class="ss-ended">
			<span class="ss-eye">Session ended</span>
			<p>{session.endedLine}</p>
			<button type="button" class="btn" onclick={() => (session.dismiss(), goto('/sessions'))}>Back to Sessions</button>
		</div>
	{:else if s}
		<div class="ss-rhead">
			<span class="ss-live">LIVE</span>
			<span class="ss-fine">· {listeners.length} here · {live}</span>
			{#if session.phase === 'retrying'}<span class="ss-fine">· reconnecting…</span>{/if}
			<span style="margin-left: auto; position: relative">
				{#if session.info?.lastSong}
					<button type="button" class="ss-mini" onclick={() => session.lastSong(false)}>Keep going</button>
				{:else}
					<button type="button" class="ss-mini ss-warn" aria-expanded={ending} onclick={() => (ending = !ending)}>End</button>
				{/if}
				{#if ending && !session.info?.lastSong}
					<div class="ss-endmenu" role="menu">
						<button type="button" role="menuitem" onclick={() => ((ending = false), session.lastSong(true))}><b>End after this song</b><i>{track ? `${track.t} finishes, then it ends` : 'The next song ends it'}</i></button>
						<button type="button" role="menuitem" onclick={() => ((ending = false), session.endNow())}><b class="ss-warn">End now</b><i>Everyone’s session ends straight away</i></button>
					</div>
				{/if}
			</span>
		</div>
		<div style="display: grid; gap: 4px">
			<p class="ss-rname">{s.name}</p>
			{#if s.vibe}<p class="ss-vibe">“{s.vibe}”</p>{/if}
			{#if session.info?.lastSong}<span class="ss-tag wait" style="justify-self: start">Ending after this song</span>{/if}
		</div>

		<div class="ss-onair">
			{#if track?.art}<img class="ss-cv" src={track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
			<span class="ss-onair-t">
				<span class="ss-eye" style="color: var(--wp-text-2)">On air</span>
				{#if track}<b dir="auto">{track.t}</b><i dir="auto">{track.a}{statsLine ? ` · ${statsLine}` : ''}</i>{:else}<b>Play something</b><i>Your station plays what your player plays.</i>{/if}
			</span>
			<Floats floats={session.floats} />
		</div>
		{#if session.stats?.favourite}<p class="ss-fav">Crowd favourite</p>{/if}

		<div>
			<div class="ss-boxh">
				<p class="ss-eye" style="color: var(--wp-text-2)">Requests · {session.requests.length} waiting</p>
				<label class="ss-switch"><span>Auto-accept</span><input type="checkbox" role="switch" checked={!!session.info?.autoAccept} onchange={(e) => session.autoAccept(e.currentTarget.checked)} /></label>
			</div>
			{#if session.info?.autoAccept}<p class="ss-fine">New requests go straight into your queue.</p>{/if}
			{#each session.requests as r (r.id)}
				<div class="ss-req ss-req-h">
					{#if r.track.art}<img class="ss-cv" src={r.track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
					<span><b dir="auto">{r.track.t}</b><i dir="auto"><span style="color: {personColor(r.from)}; font-weight: 600">{session.nameOf(r.from)}</span> · {r.track.a}</i></span>
					<span>
						<button type="button" class="ss-mini" aria-label="Play {r.track.t} next" onclick={() => session.answer(r.id, 'next')}>Next</button>
						<button type="button" class="ss-mini" aria-label="Add {r.track.t} to the queue" onclick={() => session.answer(r.id, 'queue')}>Queue</button>
						<button type="button" class="ss-mini" aria-label="Pass on {r.track.t}" onclick={() => session.answer(r.id, 'pass')}>Pass</button>
					</span>
				</div>
			{:else}
				<p class="ss-fine">No requests waiting.</p>
			{/each}
		</div>

		<div>
			<p class="ss-eye" style="color: var(--wp-text-2); margin-bottom: 8px">The crowd · {listeners.length}</p>
			{#if listeners.length}
				<div class="ss-crowd" style="flex-wrap: wrap">
					{#each listeners.slice(0, 24) as m (m.id)}<span class="ss-pp" style="background: {personColor(m.id)}" title="{m.name || 'Someone'}{m.regular ? ' · a regular' : ''}">{initialOf(m.name)}</span>{/each}
					{#if listeners.length > 24}<span class="ss-pp more">+{listeners.length - 24}</span>{/if}
				</div>
			{:else}
				<p class="ss-fine">Nobody yet. Your station shows in Live now while you play.</p>
			{/if}
		</div>

		{#if session.feed.length}
			<div class="ss-feed" aria-live="polite">
				{#each session.feed as f, i (f.key)}
					<p style="opacity: {Math.max(0.35, 1 - (session.feed.length - 1 - i) * 0.2)}">{#if f.phrase}<span class="who" style="color: {f.color}">{session.nameOf(f.who)}</span>{#if f.count > 1}&nbsp;and {f.count - 1} {f.count === 2 ? 'other' : 'others'}{/if} · {f.text}{:else}<span class="who" style="color: {f.color}">{f.text}</span>{/if}</p>
				{/each}
			</div>
		{/if}
	{/if}
</div>
