<!-- The room, full screen (the mockups' C): /now while tuned in. The cover's colours wash the page; the room's faces rise over the
     big cover; the song, whose pick it is, how the room feels, what they say and who's here beside it. Below: what's up next
     (yours, when it is), Request (a search sheet: each song found asks the DJ), React, and the volume. Esc or the arrow goes back. -->
<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { app } from '../app';
	import type { LibraryItem } from '../api';
	import { itemFromWire } from '../share/links';
	import Icon from '../ui/Icon.svelte';
	import Volume from '../ui/Volume.svelte';
	import { mmss } from '../ui/format';
	import { L, ui } from '../ui/state.svelte';
	import FaceTray from './FaceTray.svelte';
	import Floats from './Floats.svelte';
	import { session } from './session.svelte';
	import { initialOf, personColor, type PhraseId } from './words';
	import './sessions.css';

	let tray = $state(false);
	let sheet = $state(false);
	let posMs = $state(0);
	const tick = setInterval(() => (posMs = session.position() ?? 0), 500);
	onDestroy(() => clearInterval(tick));

	const s = $derived(session.station);
	const track = $derived(session.track);
	const item = $derived(track ? itemFromWire(track) : null);
	const liked = $derived(item ? L.isLiked(item) : false);
	const durMs = $derived(track?.d ?? 0);
	const pct = $derived(durMs > 0 ? Math.min(100, (posMs / durMs) * 100) : 0);
	const next = $derived(session.queue[0] ?? null);
	const listeners = $derived(session.members.filter((m) => m.id !== session.hostId && !session.info?.cohosts.includes(m.id)));
	const cohosts = $derived(session.members.filter((m) => session.info?.cohosts.includes(m.id)));
	const statsLine = $derived.by(() => {
		const st = session.stats;
		if (!st || (!st.reactions && !st.saves)) return '';
		return `${st.reactions === 1 ? '1 reaction' : `${st.reactions} reactions`} · ${st.saves} saved`;
	});

	function back() {
		if (history.length > 1) history.back();
		else void goto('/sessions');
	}
	function leave() {
		session.leave();
		void goto('/sessions');
	}
	async function heart() {
		if (!item) return;
		const was = liked;
		await L.toggleLike(item);
		if (!was) session.saved();
	}
	function key(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		if (sheet) sheet = false;
		else if (tray) tray = false;
		else back();
	}
	onMount(() => {
		window.addEventListener('keydown', key);
		return () => window.removeEventListener('keydown', key);
	});

	// ---- the Request sheet: a search whose songs ask the DJ
	let q = $state('');
	let found = $state<LibraryItem[]>([]);
	let searching = $state(false);
	let seq = 0;
	let wait: ReturnType<typeof setTimeout> | undefined;
	function find() {
		clearTimeout(wait);
		const term = q.trim();
		if (term.length < 2) return void (found = []);
		wait = setTimeout(async () => {
			const n = ++seq;
			searching = true;
			try {
				const r = await app.search.search(term);
				if (n === seq) found = r.top.slice(0, 20);
			} catch {
				if (n === seq) found = [];
			} finally {
				if (n === seq) searching = false;
			}
		}, 350);
	}
	const asked = (it: LibraryItem) => session.mine.some((r) => r.track.t === it.title && r.track.a === it.artist);
</script>

{#if s}
	<div class="ssf" style="--art: url('{track?.art ?? ''}'); --c1: {personColor(s.id)}">
		<div class="ssf-bg" aria-hidden="true"></div>
		<div class="ssf-top">
			<button type="button" class="ssf-ib" aria-label="Back" title="Back" onclick={back}><Icon name="expand_more" /></button>
			<div class="ssf-ht">
				<p class="ssf-name">{s.name}</p>
				<p class="ssf-meta"><span class="ss-live">LIVE</span><span>{session.info?.listeners ?? listeners.length} here</span>{#if s.vibe}<span aria-hidden="true">·</span><span>“{s.vibe}”</span>{/if}<span class="ss-booth" class:away={session.info?.presence === 'away'}><b>{session.hostName}</b>&nbsp;{session.info?.presence === 'away' ? 'stepped away' : 'is in the booth'}</span></p>
			</div>
			<button type="button" class="ss-mini" aria-pressed={!!s.following} onclick={() => session.follow(!s.following)}>{s.following ? 'Following' : 'Follow'}</button>
			<span class="ss-vote">
				<button type="button" aria-pressed={s.myVote === 1} aria-label="Vote {s.name} up" onclick={() => session.vote(s.myVote === 1 ? 0 : 1)}>▲</button>
				<b>{s.up - s.down}</b>
				<button type="button" aria-pressed={s.myVote === -1} aria-label="Vote {s.name} down" onclick={() => session.vote(s.myVote === -1 ? 0 : -1)}>▼</button>
			</span>
			<button type="button" class="ss-mini" onclick={leave}>Leave</button>
		</div>

		{#if session.phase === 'ended'}
			<div class="ssf-ended">
				<span class="ss-eye">Session ended</span>
				<p>{session.endedLine}</p>
				<button type="button" class="btn" onclick={() => (session.dismiss(), goto('/sessions'))}>Find another session</button>
			</div>
		{:else}
			<div class="ssf-g">
				<div class="ssf-art">
					{#if track?.art}<img class="ss-cv" src={track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
					<Floats floats={session.floats} />
				</div>
				<div class="ssf-side">
					{#if track}
						<div class="ss-song">
							<div><b class="ssf-t" dir="auto">{track.t}</b><i class="ssf-a" dir="auto">{track.a}</i></div>
							<button type="button" class="ss-heart" aria-pressed={liked} aria-label={liked ? 'Saved to Liked songs' : 'Save to Liked songs'} onclick={heart}><Icon name={liked ? 'favorite' : 'favorite_border'} /></button>
						</div>
						<div class="ss-pickline">
							{#if track.by && track.by !== session.hostId}<span class="ss-pick"><span class="ss-pp" style="background: {personColor(track.by)}">{initialOf(session.nameOf(track.by))}</span>{track.by === session.me ? 'Your pick' : `${session.nameOf(track.by)}’s pick`}</span>{/if}
							{#if session.stats?.favourite}<span class="ss-fav">Crowd favourite</span>{/if}
							{#if statsLine}<span>{statsLine}</span>{/if}
							{#if session.info?.lastSong}<span>Last song</span>{/if}
						</div>
						<div>
							<div class="ss-prog" role="progressbar" aria-label="Song progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)}><i style="width: {pct}%"></i></div>
							<div class="ssf-times"><span>{mmss(posMs)}</span><span>-{mmss(Math.max(0, durMs - posMs))}</span></div>
						</div>
					{:else}
						<p class="ss-fine">{session.phase === 'joining' ? 'Tuning in…' : `${session.hostName} is picking a song.`}</p>
					{/if}
					{#if session.feed.length}
						<div class="ss-feed ssf-feed" aria-live="polite">
							{#each session.feed as f, i (f.key)}
								<p style="opacity: {Math.max(0.35, 1 - (session.feed.length - 1 - i) * 0.2)}">{#if f.phrase}<span class="who" style="color: {f.color}">{session.nameOf(f.who)}</span>{#if f.count > 1}&nbsp;and {f.count - 1} {f.count === 2 ? 'other' : 'others'}{/if} · {f.text}{:else}<span class="who" style="color: {f.color}">{f.text}</span>{/if}</p>
							{/each}
						</div>
					{/if}
					<div class="ss-crowd">
						<span class="ss-faces">
							{#if session.hostId}<span class="ss-pp host" style="background: {personColor(session.hostId)}" title="{session.hostName} (DJ)">{initialOf(session.hostName)}</span>{/if}
							{#each cohosts as m (m.id)}<span class="ss-pp co" style="background: {personColor(m.id)}; margin-left: 4px" title="{m.name} (co-host)">{initialOf(m.name)}</span>{/each}
						</span>
						<span class="ss-faces">
							{#each listeners.slice(0, 6) as m (m.id)}<span class="ss-pp" style="background: {personColor(m.id)}" title={m.id === session.me ? 'You' : m.name}>{initialOf(m.id === session.me ? 'You' : m.name)}</span>{/each}
							{#if listeners.length > 6}<span class="ss-pp more">+{listeners.length - 6}</span>{/if}
						</span>
					</div>
				</div>
			</div>

			<div class="ssf-foot">
				<div class="ssf-next">
					{#if next}
						{#if next.art}<img class="ss-cv" src={next.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
						<span class="ssf-nt"><span class="ss-eye" style="color: var(--wp-text-2)">Up next</span><b dir="auto">{next.t}</b></span>
						{#if next.by === session.me}<span class="ss-tag ok">Your request</span>{/if}
					{/if}
				</div>
				<div class="ssf-acts">
					{#if tray}<div class="ssf-tray"><FaceTray onface={(f) => session.react(f) || ui.toast('Back in a moment: the room is reconnecting.')} onphrase={(p: PhraseId) => session.say(p) && (tray = false)} onclose={() => (tray = false)} /></div>{/if}
					<button type="button" class="btn ssf-req" aria-expanded={sheet} onclick={() => ((sheet = true), (tray = false))}>Request</button>
					<button type="button" class="ss-obtn" aria-expanded={tray} onclick={() => (tray = !tray)}>React</button>
				</div>
				<div class="ssf-vol"><Volume /></div>
			</div>
		{/if}

		{#if sheet}
			<div class="ssf-sheet" role="dialog" aria-modal="true" aria-labelledby="ssf-rq">
				<div class="ssf-sh"><h2 id="ssf-rq">Request a song</h2><button type="button" class="ssf-ib" aria-label="Close" onclick={() => (sheet = false)}><Icon name="close" /></button></div>
				<!-- svelte-ignore a11y_autofocus -->
				<input class="ssf-find" type="search" bind:value={q} oninput={find} placeholder="Search for a song" aria-label="Search for a song" autofocus />
				<p class="ss-eye" style="color: var(--wp-text-2); margin: 14px 0 6px">Your requests · {session.waiting.length} of 3</p>
				{#each session.mine as r, i (r.id ?? `q${i}`)}
					<div class="ss-req">
						{#if r.track.art}<img class="ss-cv" src={r.track.art} alt="" referrerpolicy="no-referrer" />{:else}<span class="ss-cv" aria-hidden="true"></span>{/if}
						<span><b dir="auto">{r.track.t}</b><i dir="auto">{r.track.a}</i></span>
						<span><span class="ss-tag" class:ok={r.state === 'queued'} class:wait={r.state === 'waiting'}>{r.state === 'queued' ? 'Up next' : 'Waiting'}</span>{#if r.id}<button type="button" class="ss-mini" style="height: 26px; padding: 0 9px" onclick={() => session.cancelRequest(r.id!)}>Take back</button>{/if}</span>
					</div>
				{/each}
				{#if found.length}
					<p class="ss-eye" style="color: var(--wp-text-2); margin: 14px 0 6px">Songs</p>
					{#each found as it (it.key)}
						<div class="ss-req">
							<span class="ss-cv" aria-hidden="true"></span>
							<span><b dir="auto">{it.title}</b><i dir="auto">{it.artist}</i></span>
							<span>{#if asked(it)}<span class="ss-tag">Requested</span>{:else}<button type="button" class="ss-mini" onclick={() => session.request(it)}>Request</button>{/if}</span>
						</div>
					{/each}
				{:else if q.trim().length >= 2 && !searching}
					<p class="ss-fine" style="margin-top: 12px">No songs found.</p>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<style>
	.ssf { position: fixed; inset: 0; z-index: 40; display: grid; grid-template-rows: auto 1fr auto; gap: 16px; padding: max(16px, env(safe-area-inset-top)) max(24px, env(safe-area-inset-right)) max(16px, env(safe-area-inset-bottom)) max(24px, env(safe-area-inset-left)); background: var(--wp-main); color: var(--wp-text); overflow: hidden; }
	.ssf-bg { position: absolute; inset: -10%; background: var(--art) center / cover, linear-gradient(165deg, var(--c1), #0b0b12); filter: blur(70px) saturate(1.5); opacity: .85; }
	.ssf-bg::after { content: ''; position: absolute; inset: 0; background: linear-gradient(180deg, rgb(0 0 0 / .15), rgb(0 0 0 / .55)); }
	.ssf > :not(.ssf-bg) { position: relative; }
	.ssf-top { display: flex; align-items: center; gap: 10px; }
	.ssf-ht { flex: 1; min-width: 0; text-align: center; }
	.ssf-name { font: 500 18px/1.2 var(--font-display); }
	.ssf-meta { display: flex; flex-wrap: wrap; justify-content: center; align-items: center; gap: 4px 8px; margin-top: 4px; color: var(--wp-text-2); font-size: 13px; }
	.ssf-ib { display: grid; place-items: center; width: 40px; height: 40px; border-radius: 50%; color: var(--wp-text); }
	.ssf-ib:hover { background: var(--wp-hover); }
	.ssf-g { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); align-items: center; gap: clamp(24px, 4vw, 56px); min-height: 0; }
	.ssf-art { position: relative; justify-self: end; width: min(100%, 440px); }
	.ssf-art .ss-cv { width: 100%; height: auto; border-radius: 16px; box-shadow: 0 30px 80px -30px rgb(0 0 0 / .8); }
	.ssf-side { display: grid; gap: 16px; max-width: 560px; min-width: 0; }
	.ssf-t { font: 500 clamp(28px, 3.4vw, 46px)/1.08 var(--font-display) !important; letter-spacing: -.02em; }
	.ssf-a { font-size: clamp(15px, 1.4vw, 19px) !important; }
	.ssf-times { display: flex; justify-content: space-between; margin-top: 6px; color: var(--wp-text-2); font-size: 12px; font-variant-numeric: tabular-nums; }
	.ssf-feed { font-size: 15px; }
	.ssf-foot { display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: 16px; }
	.ssf-next { display: flex; align-items: center; gap: 10px; min-width: 0; max-width: 380px; }
	.ssf-next .ss-cv { width: 44px; height: 44px; border-radius: 8px; }
	.ssf-nt { display: grid; gap: 3px; min-width: 0; }
	.ssf-nt b { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.ssf-acts { position: relative; display: flex; gap: 10px; }
	.ssf-req { height: 44px; min-width: 180px; border-radius: 999px; font-size: 14px; }
	.ssf-tray { position: absolute; bottom: calc(100% + 10px); left: 50%; width: min(420px, calc(100vw - 32px)); transform: translateX(-50%); max-height: min(70vh, 560px); overflow-y: auto; border-radius: 18px; }
	.ssf-vol { justify-self: end; }
	.ssf-ended { display: grid; place-content: center; justify-items: center; gap: 12px; text-align: center; }
	.ssf-sheet { position: absolute; top: 72px; right: max(24px, env(safe-area-inset-right)); bottom: 16px; z-index: 2; width: min(420px, calc(100vw - 32px)); padding: 16px; border-radius: 18px; background: var(--wp-chrome); border: 1px solid var(--wp-line-2); overflow-y: auto; box-shadow: 0 30px 80px -20px rgb(0 0 0 / .7); }
	.ssf-sh { display: flex; align-items: center; justify-content: space-between; margin-bottom: 10px; }
	.ssf-sh h2 { font: 500 18px/1.2 var(--font-display); }
	.ssf-find { width: 100%; height: 40px; padding: 0 14px; border-radius: 12px; background: var(--wp-field); border: 1px solid var(--wp-line-2); color: var(--wp-text); font: 500 14px/1 var(--font-body); }
	@media (max-width: 820px) {
		.ssf { overflow-y: auto; grid-template-rows: auto auto auto; }
		.ssf-g { grid-template-columns: 1fr; }
		.ssf-art { justify-self: center; width: min(100%, 360px); }
		.ssf-foot { grid-template-columns: 1fr; justify-items: center; }
		.ssf-vol { display: none; }
		.ssf-top .ss-vote { display: none; }
	}
</style>
