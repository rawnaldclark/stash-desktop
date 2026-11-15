<!--
  The lyric viewport: ONE column used by the docked panel and the big lyrics.
  Lines are laid out in a column that steps to the active line; the animator (Web Animations pinned to player.now())
  does the word sweep, lift, glow, line alpha and distance blur. Reduced motion: no sweep or scale, instant line changes.
-->
<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { lyrics as lyricsApi, player } from '../../app';
	import type { Lyrics, QueueItem } from '../../api';
	import { S, ui } from '../state.svelte';
	import { activeIndex, buildLines, seekTarget, type LineModel } from './model';
	import { LyricAnimator } from './animator';
	import './lyrics.css';

	let { item }: { item: QueueItem } = $props();

	let status = $state<'loading' | 'ready' | 'plain' | 'none'>('loading');
	let lines = $state.raw<LineModel[]>([]);
	let plain = $state('');
	let act = $state(0);
	let vp = $state<HTMLElement>();
	let col = $state<HTMLElement>();
	let colY = $state(0);
	let animator: LyricAnimator | null = null;

	const qid = $derived(item.qid);
	const from = $derived(Math.max(0, act - 3));
	const to = $derived(Math.min(lines.length - 1, act + 12));
	const visible = $derived(Array.from({ length: Math.max(0, to - from + 1) }, (_, k) => from + k));

	$effect(() => {
		// (Re)load whenever the song changes (keyed by id: the store re-emits the same song 4x a second).
		qid;
		const it = untrack(() => item);
		let cancelled = false;
		status = 'loading';
		lines = [];
		lyricsApi.forTrack(it).then(
			(ly: Lyrics | null) => {
				if (cancelled) return;
				if (ly?.synced?.length) {
					lines = buildLines(ly.synced);
					status = lines.length ? 'ready' : 'none';
					act = lines.length ? activeIndex(lines, player.now()) : 0;
				} else if (ly?.plain?.trim()) {
					plain = ly.plain.trim();
					status = 'plain';
				} else status = 'none';
			},
			() => {
				if (!cancelled) status = 'none';
			}
		);
		return () => {
			cancelled = true;
		};
	});

	/** Everything that touches the DOM or builds animations (the line window, `act`, words) happens here, and only in the frame where the
	 *  clock text changes (once a second, see the effect below): a line change on its own would cost a main-thread frame of its own.
	 *  The animations are built to cover the next second and a half, and the pinned animations show every line change on the compositor. */
	function check() {
		if (!animator || document.hidden) return;
		const a = activeIndex(lines, player.now());
		if (a !== act) act = a;
		animator.update(S.v.playing);
	}

	// Build the animator once the column is in the DOM.
	$effect(() => {
		if (status !== 'ready' || !vp || !col || !lines.length) return;
		const an = new LyricAnimator({
			column: col,
			viewport: vp,
			lines,
			now: () => player.now(),
			reduced: ui.reduced,
			glow: !ui.reduced && window.innerWidth >= 768
		});
		animator = an;
		untrack(() => {
			colY = 0;
			an.update(S.v.playing);
		});
		const ro = new ResizeObserver(() => {
			an.measure();
			an.update(untrack(() => S.v.playing));
			an.repin();
		});
		ro.observe(vp);
		const vis = () => {
			if (!document.hidden) {
				an.reset(untrack(() => S.v.playing));
			}
		};
		document.addEventListener('visibilitychange', vis);
		return () => {
			document.removeEventListener('visibilitychange', vis);
			ro.disconnect();
			an.destroy();
			animator = null;
		};
	});

	// Events: play, pause and (while paused) any published position re-pin. While playing the published position (4 Hz)
	// only checks the pin for a seek (one read) and, when the whole second changes, runs the maintenance: setting startTime on ~100
	// animations is what costs main-thread time, so nothing re-pins unless the song jumped.
	let lastPlaying: boolean | null = null;
	let lastSec = -1;
	$effect(() => {
		const playing = S.v.playing;
		const pos = S.v.positionMs;
		untrack(() => {
			if (!animator) return;
			const changed = playing !== lastPlaying;
			lastPlaying = playing;
			if (playing && !changed) {
				const sec = Math.floor(pos / 1000);
				if (sec !== lastSec) {
					lastSec = sec;
					check();
				} else if (animator.drift() > 30) {
					check();
				}
				return;
			}
			check();
			animator.update(playing);
			animator.repin();
		});
	});

	/** Tap (or Enter / Space on) a line: seek to its start, like the app (`onLyricsLineSeek`: the line's time plus the song's sync offset, so the
	 *  DISPLAYED clock lands on the line). The window and the animations are rebuilt around the new time once the DOM has followed. */
	function seekTo(j: number) {
		const ln = lines[j];
		if (!ln) return;
		player.seek(seekTarget(ln, 0));
		act = j;
		void tick().then(() => animator?.reset(untrack(() => S.v.playing)));
	}
	function onKey(e: KeyboardEvent, j: number) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			seekTo(j);
		} else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			const next = col?.querySelector<HTMLElement>(`.ln[data-i="${j + (e.key === 'ArrowDown' ? 1 : -1)}"]`);
			if (next) {
				e.preventDefault();
				next.focus();
			}
		}
	}

	// After the DOM for a new window is in place, build the animations for it.
	$effect(() => {
		from;
		to;
		untrack(() => animator?.update(S.v.playing));
	});
</script>

<div class="lyr" bind:this={vp} class:rm={ui.reduced} data-status={status}>
	{#if status === 'ready'}
		<div class="lc" bind:this={col} role="group" aria-label="Lyrics, {item.title}. Press a line to jump to it." style:transform="translateY({colY}px)">
			{#each visible as j (j)}
				{@const ln = lines[j]}
				<div class="ln" dir="auto" role="button" tabindex={j === act ? 0 : -1} aria-label={ln.text} class:act={j === act} aria-current={j === act ? 'true' : undefined} onclick={() => seekTo(j)} onkeydown={(e) => onKey(e, j)} data-i={j} data-s={j < act - 2 ? 'behind' : 'live'} style="--i:{j};--fit:{ln.fit}">
					<span class="ls" data-layer="ls">{#each ln.words as w, wi (wi)}{#if wi > 0}{' '}{/if}{#if w.held}<span class="w h"><span class="wg">{w.text}</span>{#each [...w.text] as ch, ci (ci)}<i class="lt">{ch}</i>{/each}</span>{:else}<span class="w" class:r={w.rtl}><span class="wd">{w.text}</span><span class="wl"><span class="wlt">{w.text}</span></span></span>{/if}{/each}</span>
					<span class="lb nd" data-layer="nd">{ln.text}</span><span class="lb fd" data-layer="fd">{ln.text}</span><span class="lb nl" data-layer="nl">{ln.text}</span><span class="lb fl" data-layer="fl">{ln.text}</span>
				</div>
			{/each}
		</div>
		<!-- The whole song for screen readers: the column above only holds a window of about 16 lines (the seekable buttons). This copy is
		     static (built once per song, never animated or re-rendered while it plays) and visually hidden. -->
		<section class="vh lyr-all" aria-label="Full lyrics, {item.title}">
			<ol>{#each lines as ln, k (k)}<li dir="auto">{ln.text}</li>{/each}</ol>
		</section>
	{:else if status === 'plain'}
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div class="lyr-plain" tabindex="0" role="region" aria-label="Lyrics, {item.title}">{plain}</div>
	{:else if status === 'loading'}
		<p class="lyr-msg lyr-quiet">Looking for lyrics</p>
	{:else}
		<p class="lyr-msg">No lyrics for this song.</p>
	{/if}
</div>
