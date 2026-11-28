<!-- The room, in the side panel (the mockups' B): the station and its DJ, the song with the room's faces rising over its cover,
     whose pick it is and how the room feels about it, who's here, what they say, this listener's requests, and React / Leave.
     Song rows elsewhere say Request while tuned in, so a listener keeps browsing. -->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { goto } from '$app/navigation';
	import { itemFromWire } from '../share/links';
	import { L, ui } from '../ui/state.svelte';
	import Icon from '../ui/Icon.svelte';
	import BoothPanel from './BoothPanel.svelte';
	import FaceTray from './FaceTray.svelte';
	import Floats from './Floats.svelte';
	import { session } from './session.svelte';
	import { initialOf, personColor, type PhraseId } from './words';

	let tray = $state(false);
	let posMs = $state(0);
	const tick = setInterval(() => (posMs = session.position() ?? 0), 500);
	onDestroy(() => clearInterval(tick));

	const s = $derived(session.station);
	const track = $derived(session.track);
	const item = $derived(track ? itemFromWire(track) : null);
	const liked = $derived(item ? L.isLiked(item) : false);
	const durMs = $derived(track?.d ?? 0);
	const pct = $derived(durMs > 0 ? Math.min(100, (posMs / durMs) * 100) : 0);
	const listeners = $derived(session.members.filter((m) => m.id !== session.hostId && !session.info?.cohosts.includes(m.id)));
	const cohosts = $derived(session.members.filter((m) => session.info?.cohosts.includes(m.id)));
	const crowdLine = $derived.by(() => {
		const names = listeners.map((m) => (m.id === session.me ? 'You' : m.name || 'Someone'));
		names.sort((a, b) => (a === 'You' ? -1 : b === 'You' ? 1 : 0));
		if (!names.length) return 'Just the DJ so far';
		if (names.length <= 3) return names.join(', ').replace(/, ([^,]*)$/, ' and $1');
		return `${names.slice(0, 3).join(', ')} and ${names.length - 3} more`;
	});
	const statsLine = $derived.by(() => {
		const st = session.stats;
		if (!st || (!st.reactions && !st.saves)) return '';
		const r = st.reactions === 1 ? '1 reaction' : `${st.reactions} reactions`;
		return `${r} · ${st.saves} saved`;
	});

	function face(id: string) {
		if (!session.react(id)) ui.toast('Back in a moment: the room is reconnecting.');
	}
	function say(id: PhraseId) {
		if (session.say(id)) tray = false;
	}
	async function heart() {
		if (!item) return;
		const was = liked;
		await L.toggleLike(item);
		if (!was) session.saved();
	}
	function leave() {
		session.leave();
		tray = false;
	}
</script>

{#if session.role === 'host'}
	<BoothPanel />
{:else}
<div class="ss-room">
	{#if session.phase === 'ended'}
		<div class="ss-ended">
			<span class="ss-eye">Session ended</span>
			<p>{session.endedLine}</p>
			<button type="button" class="btn" onclick={() => (session.dismiss(), goto('/sessions'))}>Find another session</button>
		</div>
	{:else if s}
		<div class="ss-rhead">
			<span class="ss-live">LIVE</span>
			<span class="ss-fine">· {session.info?.listeners ?? listeners.length} here</span>
			{#if session.phase === 'retrying'}<span class="ss-fine">· reconnecting…</span>{/if}
			<button type="button" class="ss-mini" style="margin-left: auto" aria-pressed={!!s.following} onclick={() => session.follow(!s.following)}>{s.following ? 'Following' : 'Follow'}</button>
			<a class="ss-mini" href="/now" aria-label="Full screen" title="Full screen" style="padding: 0 8px"><Icon name="open_in_full" /></a>
		</div>
		<div style="display: grid; gap: 6px">
			<a class="ss-rname" href="/sessions">{s.name}</a>
			{#if s.vibe}<p class="ss-vibe">“{s.vibe}”</p>{/if}
			<span class="ss-booth" class:away={session.info?.presence === 'away'}><b style="color: {session.hostId ? personColor(session.hostId) : 'inherit'}">{session.hostName}</b>&nbsp;{session.info?.presence === 'away' ? 'stepped away' : 'is in the booth'}</span>
		</div>

		<div class="ss-art">
			{#if track?.art}<img class="ss-cv" src={track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" style="--c1: {personColor(s.id)}" aria-hidden="true"></span>{/if}
			<Floats floats={session.floats} />
		</div>

		{#if track}
			<div class="ss-song">
				<div><b dir="auto">{track.t}</b><i dir="auto">{track.a}</i></div>
				<button type="button" class="ss-heart" aria-pressed={liked} aria-label={liked ? 'Saved to Liked songs' : 'Save to Liked songs'} onclick={heart}><Icon name={liked ? 'favorite' : 'favorite_border'} /></button>
			</div>
			<div class="ss-pickline">
				{#if track.by && track.by !== session.hostId}<span class="ss-pick"><span class="ss-pp" style="background: {personColor(track.by)}">{initialOf(session.nameOf(track.by))}</span>{track.by === session.me ? 'Your pick' : `${session.nameOf(track.by)}’s pick`}</span>{/if}
				{#if session.stats?.favourite}<span class="ss-fav">Crowd favourite</span>{/if}
				{#if statsLine}<span>{statsLine}</span>{/if}
				{#if session.info?.lastSong}<span>Last song</span>{/if}
			</div>
			<div class="ss-prog" role="progressbar" aria-label="Song progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}><i style="width: {pct}%"></i></div>
		{:else}
			<p class="ss-fine">{session.phase === 'joining' ? 'Tuning in…' : `${session.hostName} is picking a song.`}</p>
		{/if}

		<div class="ss-crowd">
			<span class="ss-faces">
				{#if session.hostId}<span class="ss-pp host" style="background: {personColor(session.hostId)}" title="{session.hostName} (DJ)">{initialOf(session.hostName)}</span>{/if}
				{#each cohosts as m (m.id)}<span class="ss-pp co" style="background: {personColor(m.id)}; margin-left: 4px" title="{m.name} (co-host)">{initialOf(m.name)}</span>{/each}
			</span>
			<span class="ss-faces">
				{#each listeners.slice(0, 5) as m (m.id)}<span class="ss-pp sm" style="background: {personColor(m.id)}" title={m.id === session.me ? 'You' : m.name}>{initialOf(m.id === session.me ? 'You' : m.name)}</span>{/each}
				{#if listeners.length > 5}<span class="ss-pp sm more">+{listeners.length - 5}</span>{/if}
			</span>
			<span class="ss-fine">{crowdLine}</span>
		</div>

		{#if session.feed.length}
			<div class="ss-feed" aria-live="polite">
				{#each session.feed as f, i (f.key)}
					<p style="opacity: {Math.max(0.35, 1 - (session.feed.length - 1 - i) * 0.2)}">{#if f.phrase}<span class="who" style="color: {f.color}">{session.nameOf(f.who)}</span>{#if f.count > 1}&nbsp;and {f.count - 1} {f.count === 2 ? 'other' : 'others'}{/if} · {f.text}{:else}<span class="who" style="color: {f.color}">{f.text}</span>{/if}</p>
				{/each}
			</div>
		{/if}

		<div>
			<p class="ss-eye" style="color: var(--wp-text-2); margin-bottom: 4px">Your requests · {session.waiting.length} of 3</p>
			{#if session.mine.length}
				<div class="ss-mine">
					{#each session.mine as r, i (r.id ?? `q${i}`)}
						<div class="ss-req">
							{#if r.track.art}<img class="ss-cv" src={r.track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
							<span><b dir="auto">{r.track.t}</b><i dir="auto">{r.track.a}</i></span>
							<span>
								<span class="ss-tag" class:ok={r.state === 'queued'} class:wait={r.state === 'waiting'}>{r.state === 'queued' ? 'Up next' : 'Waiting'}</span>
								{#if r.id}<button type="button" class="ss-mini" style="height: 26px; padding: 0 9px" onclick={() => session.cancelRequest(r.id!)}>Take back</button>{/if}
							</span>
						</div>
					{/each}
				</div>
			{/if}
			<p class="ss-fine" style="margin-top: 4px">Search for a song and tap Request. {session.hostName} sees it in the booth.</p>
		</div>

		{#if tray}<FaceTray onface={face} onphrase={say} onclose={() => (tray = false)} />{/if}
		<div class="ss-acts">
			<button type="button" class="ss-obtn grow" aria-expanded={tray} onclick={() => (tray = !tray)}>React</button>
			<button type="button" class="ss-obtn" onclick={leave}>Leave</button>
		</div>
	{/if}
</div>
{/if}
