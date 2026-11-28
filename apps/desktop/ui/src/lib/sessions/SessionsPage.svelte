<!-- /sessions: Community's live listening (the web player's page; keep them in step). Live now (the stations this computer follows first, then everyone, highest voted first)
     and Upcoming (stations on a schedule). Tune in opens the room in the side panel (the mockups' B) and the player follows the DJ.
     Tuning in needs Community joined (nothing is sent before); "coming soon" while Sessions aren't on in this player. -->
<script lang="ts">
	import { onMount } from 'svelte';
	import { community } from '../community/key';
	import JoinCommunity from '../community/JoinCommunity.svelte';
	import GoLive from './GoLive.svelte';
	import { ui } from '../ui/state.svelte';
	import { client, directory, MAX_ROOM_NAME, session } from './session.svelte';
	import { sessionsMessage } from './client';
	import { personColor } from './words';
	import type { Live, StationSummary, Window } from './rules';
	import './sessions.css';

	let tab = $state<'live' | 'up'>('live');
	let going = $state(false);
	function openGoLive() {
		if ($joined === false) return void ui.toast('Join Community first: it’s how your station is yours.');
		going = true;
	}
	const joined = community.joined;
	const followedLive = $derived((directory.following ?? []).filter((s) => s.live));
	const followedIds = $derived(new Set(followedLive.map((s) => s.id)));
	const everyone = $derived((directory.live ?? []).filter((s) => !followedIds.has(s.station.id)));
	const liveCount = $derived((directory.live ?? []).length);

	onMount(() => {
		void directory.load();
		const back = () => document.visibilityState === 'visible' && void directory.load({ ifStale: true });
		document.addEventListener('visibilitychange', back);
		return () => document.removeEventListener('visibilitychange', back);
	});

	// Join reloads, so the followed stations and this computer's votes show.
	let wasJoined: boolean | null = null;
	$effect(() => {
		const j = $joined;
		if (wasJoined === false && j) void directory.load();
		wasJoined = j;
	});

	/** The station waiting on the one-time "what should the room call you?" before its first tune-in. */
	let naming = $state<string | null>(null);
	let nameDraft = $state('');

	async function tuneIn(id: string) {
		if (session.name === null && !(session.inSession && session.station?.id === id)) {
			naming = id;
			return;
		}
		naming = null;
		const r = await session.tuneIn(id);
		if (r === 'join') ui.toast('Join Community first: it’s how a session knows you.');
		if (r === 'hosting') ui.toast('You’re live. End your session in the booth to tune in somewhere else.');
		if (r === 'ok') ui.setTab('room');
	}

	const here = (n: number) => (n === 1 ? '1 here' : `${n} here`);
	const nowPlaying = (l: Live) => (l.paused ? 'Paused' : l.track ? `${l.track.t} · ${l.track.a}` : 'Starting');
	const cover = (s: StationSummary, l?: Live | null) => ({ art: l?.track?.art, c1: personColor(s.id), c2: personColor(s.id.split('').reverse().join('')) });
	function when(w: Window): string {
		const start = new Date(w.start);
		const now = Date.now();
		const t = start.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
		if (w.start <= now) return 'On the air soon';
		const days = Math.floor((new Date(start).setHours(0, 0, 0, 0) - new Date(now).setHours(0, 0, 0, 0)) / 86_400_000);
		if (days === 0) return `Today at ${t}`;
		if (days === 1) return `Tomorrow at ${t}`;
		return `${start.toLocaleDateString([], { weekday: 'long' })} at ${t}`;
	}
	async function vote(s: StationSummary, value: -1 | 1) {
		const key = await community.key();
		if (!key) return void ui.toast('Join Community to vote.');
		try {
			const r = await client.vote(key, s.id, s.myVote === value ? 0 : value);
			s.up = r.up;
			s.down = r.down;
			s.myVote = r.myVote;
		} catch (e) {
			ui.toast(sessionsMessage(e));
		}
	}
	async function follow(s: StationSummary) {
		const key = await community.key();
		if (!key) return void ui.toast('Join Community to follow stations.');
		try {
			const r = await client.follow(key, s.id, !s.following);
			s.following = r.following;
			s.followers = r.followers;
		} catch (e) {
			ui.toast(sessionsMessage(e));
		}
	}
</script>

{#snippet art(s: StationSummary, l?: Live | null)}
	{@const c = cover(s, l)}
	{#if c.art}<img class="ss-cv" src={c.art} alt="" loading="lazy" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" style="--c1: {c.c1}; --c2: {c.c2}" aria-hidden="true"></span>{/if}
{/snippet}

{#snippet voteCtl(s: StationSummary)}
	<span class="ss-vote">
		<button type="button" aria-pressed={s.myVote === 1} aria-label="Vote {s.name} up" onclick={() => vote(s, 1)}>▲</button>
		<b>{s.up - s.down}</b>
		<button type="button" aria-pressed={s.myVote === -1} aria-label="Vote {s.name} down" onclick={() => vote(s, -1)}>▼</button>
	</span>
{/snippet}

<div class="view view-in view-sessions">
	<div class="ss-head">
		<div style="min-width: 0">
			<p class="ss-eye">Community</p>
			<h1 class="ss-title">Sessions</h1>
			<p class="ss-sub">Tune in to someone’s station and hear what they play, as they play it. Request a song, react, and vote up the stations you like.</p>
		</div>
		{#if directory.on !== false}
			{#if session.role === 'host' && session.inSession}
				<button type="button" class="ss-go" onclick={() => ui.setTab('room')}><span class="ss-live">LIVE</span>You’re live · open the booth</button>
			{:else}
				<button type="button" class="ss-go" onclick={openGoLive}>Go live</button>
			{/if}
		{/if}
	</div>
	{#if going}<GoLive onclose={() => ((going = false), void directory.load())} />{/if}

	{#if directory.on === false}
		<p class="ss-empty">{directory.offline ? 'Stash is offline: Sessions need the internet.' : 'Sessions are coming soon.'}</p>
	{:else}
		{#if $joined === false}
			<div style="margin-top: 20px"><JoinCommunity lead="Join Community to tune in: it’s how a session knows you’re there, and lets you request songs, react and vote." /></div>
		{/if}

		{#if naming}
			<form class="ss-name" onsubmit={(e) => (e.preventDefault(), session.setName(nameDraft), tuneIn(naming!))}>
				<label for="ss-name-in"><b>What should the room call you?</b><span class="ss-fine">The DJ and everyone tuned in see it. It’s kept on this computer.</span></label>
				<div class="ss-name-row">
					<!-- svelte-ignore a11y_autofocus -->
					<input id="ss-name-in" type="text" bind:value={nameDraft} maxlength={MAX_ROOM_NAME} autocomplete="nickname" placeholder="Your name" autofocus />
					<button type="submit" class="btn">Tune in</button>
					<button type="button" class="btn ghost" onclick={() => (session.setName(''), tuneIn(naming!))}>Skip</button>
				</div>
			</form>
		{/if}

		<div class="seg ss-tabs" role="tablist" aria-label="Sessions">
			<button type="button" role="tab" class="seg-i" aria-selected={tab === 'live'} onclick={() => (tab = 'live')}>Live{liveCount ? ` · ${liveCount}` : ''}</button>
			<button type="button" role="tab" class="seg-i" aria-selected={tab === 'up'} onclick={() => (tab = 'up')}>Upcoming{directory.upcoming?.length ? ` · ${directory.upcoming.length}` : ''}</button>
		</div>

		{#if directory.failed && !directory.live}
			<div class="ss-empty"><p>{directory.failed}</p><button type="button" class="btn ghost" style="margin-top: 12px" onclick={() => directory.load()}>Try again</button></div>
		{:else if directory.live === null}
			<p class="ss-empty" aria-busy="true">Finding who’s live…</p>
		{:else if tab === 'live'}
			{#if followedLive.length}
				<div class="ss-sec"><h2>Stations you follow</h2><p>{followedLive.length} live now</p></div>
				<div class="ss-fol">
					{#each followedLive as s (s.id)}
						<div class="ss-card">
							{@render art(s, s.live)}
							<div class="ss-cb">
								<span class="ss-live">LIVE</span>
								<button type="button" class="ss-cn" onclick={() => tuneIn(s.id)}>{s.name}</button>
								{#if s.vibe}<p class="ss-vibe">“{s.vibe}”</p>{/if}
								{#if s.live}<p class="ss-np"><span class="ss-eq" aria-hidden="true"><i></i><i></i><i></i></span><span>{nowPlaying(s.live)}</span></p>{/if}
								<div class="ss-cf">
									<span class="ss-here">{s.hostName ? `${s.hostName} · ` : ''}{here(s.live?.listeners ?? 0)}</span>
									{@render voteCtl(s)}
									{#if s.live?.full}<span class="ss-tag full">Full</span>{:else}<button type="button" class="btn" onclick={() => tuneIn(s.id)}>{session.station?.id === s.id && session.inSession ? (session.role === 'host' ? 'Your station' : 'Listening') : 'Tune in'}</button>{/if}
								</div>
							</div>
						</div>
					{/each}
				</div>
			{/if}

			<div class="ss-sec"><h2>{followedLive.length ? 'Everyone live' : 'Live now'}</h2><p>Highest voted first. A session holds 50 listeners.</p></div>
			{#if everyone.length}
				<div class="ss-list">
					{#each everyone as l, i (l.station.id)}
						<div class="ss-row">
							<span class="ss-rk">{i + 1}</span>
							{@render art(l.station, l)}
							<button type="button" class="ss-n" onclick={() => tuneIn(l.station.id)}><b>{l.station.name}</b>{#if l.station.vibe}<i>“{l.station.vibe}”</i>{/if}</button>
							<span class="ss-np"><span class="ss-eq" aria-hidden="true"><i></i><i></i><i></i></span><span>{nowPlaying(l)}</span></span>
							<span class="ss-count">{here(l.listeners)}</span>
							{@render voteCtl(l.station)}
							{#if l.full}<span class="ss-tag full">Full</span>{:else}<button type="button" class="btn" onclick={() => tuneIn(l.station.id)}>{session.station?.id === l.station.id && session.inSession ? (session.role === 'host' ? 'Your station' : 'Listening') : 'Tune in'}</button>{/if}
						</div>
					{/each}
				</div>
			{:else if !followedLive.length}
				<p class="ss-empty">Nobody’s live right now. Stations on a schedule are under Upcoming.</p>
			{/if}
		{:else}
			<div class="ss-sec"><h2>Upcoming</h2><p>Stations on a schedule go live when their DJ plays during these hours.</p></div>
			{#if directory.upcoming?.length}
				<div class="ss-list">
					{#each directory.upcoming as s (s.id)}
						<div class="ss-row up">
							{@render art(s)}
							<span class="ss-n"><b>{s.name}</b>{#if s.vibe}<i>“{s.vibe}”</i>{/if}</span>
							<span class="ss-np"><span>{when(s.next)}</span></span>
							<button type="button" class="ss-mini" aria-pressed={!!s.following} onclick={() => follow(s)}>{s.following ? 'Following' : 'Follow'}</button>
						</div>
					{/each}
				</div>
			{:else}
				<p class="ss-empty">No stations on a schedule yet.</p>
			{/if}
		{/if}
	{/if}
</div>
