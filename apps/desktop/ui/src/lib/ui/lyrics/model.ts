/**
 * Lyric motion maths (framework-free, no DOM). Constants are the prototype's / the app's (LyricsMotion.kt, LyricsSyllableRenderer.kt):
 * line alpha active 1 / sung .497 / upcoming .51 over 300 ms; distance blur in two static steps (near at distance 2, far at 3+) made of
 * pre-blurred copies cross-faded over 150 ms at s_k + 150 ms; the lyric column steps 400 ms after each line start; word lift
 * 1 -> 1.0505 -> 1 and glow on the app's curve; held words lit letter by letter.
 * Everything here returns Web Animations keyframes in SONG time (ms); the animator pins them to the engine clock.
 */
import type { LyricLine } from '../../sources/contract';

export type WordModel = { a: number; b: number; text: string; held: boolean; rtl?: boolean };
export type LineModel = { i: number; start: number; end: number; text: string; words: WordModel[]; timed: boolean; fit: number };

export const OP = { active: 1, sung: 0.497, upcoming: 0.51 } as const;
export const LINE_MS = 300;
export const FADE_MS = 150;
export const COLUMN_MS = 400;
export const HELD_MS = 1000;
export const EASE = 'cubic-bezier(.4,0,.2,1)';
export type Layer = 'ls' | 'nd' | 'fd' | 'nl' | 'fl';
export const LAYERS: Layer[] = ['ls', 'nd', 'fd', 'nl', 'fl'];

const COMBINING = /\p{M}/u;

/** True when a word's first letter is from a right-to-left script (Hebrew, Arabic, Syriac, Thaana, N'Ko, Arabic presentation forms). */
export function isRtlWord(text: string): boolean {
	for (const ch of text) {
		if (/[\u0590-\u08FF\uFB1D-\uFDFF\uFE70-\uFEFF]/.test(ch)) return true;
		if (/\p{L}/u.test(ch)) return false;
	}
	return false;
}

/** Normalises synced lyrics into lines with absolute word windows. Out-of-order or empty lines are dropped. */
export function buildLines(synced: LyricLine[]): LineModel[] {
	const src = synced.filter((l) => l.text.trim().length > 0 || (l.words?.length ?? 0) > 0).slice().sort((a, b) => a.startMs - b.startMs);
	return src.map((l, i) => {
		const next = src[i + 1];
		const start = l.startMs;
		let end = l.endMs ?? next?.startMs ?? start + 4000;
		if (next) end = Math.min(end, next.startMs);
		end = Math.max(end, start + 300);
		const text = (l.words?.length ? l.words.map((w) => w.text).join(' ') : l.text).replace(/\s+/g, ' ').trim();
		let words: WordModel[];
		let timed = false;
		if (l.words && l.words.length > 0) {
			timed = true;
			let heldCount = 0;
			words = l.words.map((w, wi) => {
				const a = w.startMs;
				const nw = l.words![wi + 1];
				let b = w.endMs ?? nw?.startMs ?? end;
				b = Math.max(b, a + 40);
				const t = w.text.trim();
				const rtl = isRtlWord(t);
				// a right-to-left word is never split into letters: that would break the joined shapes of Arabic. Nor is a word with
				// combining marks (Devanagari, Bengali, Thai vowel signs, a decomposed accent): a mark alone in its own box draws as a
				// dotted circle.
				const held = !rtl && !COMBINING.test(t) && b - a >= HELD_MS && [...t].length <= 12 && [...t].length >= 2 && heldCount < 2;
				if (held) heldCount++;
				return rtl ? { a, b, text: t, held, rtl } : { a, b, text: t, held };
			});
		} else {
			// Line-synced only: the whole line lights at its start (a quick sweep), then sits lit.
			words = [isRtlWord(text) ? { a: start, b: start + 250, text, held: false, rtl: true } : { a: start, b: start + 250, text, held: false }];
		}
		const chars = [...text].length;
		const fit = chars <= 25 ? 1 : Math.max(0.6, 25 / chars);
		return { i, start, end, text, words, timed, fit };
	});
}

/** Where to seek so that the DISPLAYED lyric clock (position minus the song's sync offset) lands on a tapped line: the app's `onLyricsLineSeek`. */
export const seekTarget = (line: Pick<LineModel, 'start'>, offsetMs: number): number => Math.max(0, line.start + offsetMs);

/** Index of the line that is current at `t`: the last one that has started (the first line during an intro). */
export function activeIndex(lines: LineModel[], t: number): number {
	let lo = 0;
	let hi = lines.length - 1;
	if (hi < 0) return -1;
	if (t < lines[0].start) return 0;
	while (lo < hi) {
		const mid = (lo + hi + 1) >> 1;
		if (lines[mid].start <= t) lo = mid;
		else hi = mid - 1;
	}
	return lo;
}

const lerp = (t: number, t0: number, t1: number, a: number, b: number) => (t <= t0 ? a : t >= t1 ? b : a + ((b - a) * (t - t0)) / (t1 - t0));

/** Line alpha of line j at time t (instant = reduced motion: a step at each start). */
export function alphaAt(lines: LineModel[], j: number, t: number, instant = false): number {
	const s = lines[j].start;
	const n = lines[j + 1]?.start;
	if (instant) return t < s ? OP.upcoming : n !== undefined && t >= n ? OP.sung : OP.active;
	if (t <= s) return OP.upcoming;
	if (t <= s + LINE_MS) return lerp(t, s, s + LINE_MS, OP.upcoming, OP.active);
	if (n === undefined || t <= n) return OP.active;
	return lerp(t, n, n + LINE_MS, OP.active, OP.sung);
}

/** The blur clock: the active line k whose level is reached at s_k + 150 ms (0 until the second line's level). */
function level(lines: LineModel[], t: number): number {
	if (lines.length < 2 || t - FADE_MS < lines[1].start) return 0;
	return activeIndex(lines, t - FADE_MS);
}

/** 1 when `layer` of line j shows while line k is active. near = distance 2, far = 3+; dim before, lit after. */
export function layerVisible(layer: Layer, j: number, k: number): number {
	const d = Math.abs(j - k);
	if (layer === 'ls') return d <= 1 ? 1 : 0;
	const dim = k < j;
	if (layer === 'nd' || layer === 'nl') return d === 2 && (layer === 'nd') === dim ? 1 : 0;
	return d >= 3 && (layer === 'fd') === dim ? 1 : 0;
}

/** Opacity of a layer of line j at time t: line alpha x the cross-fade of the blur step. */
export function layerOpacity(lines: LineModel[], layer: Layer, j: number, t: number, instant = false): number {
	const m = level(lines, t);
	let vis: number;
	if (m === 0) vis = layerVisible(layer, j, 0);
	else {
		const f = instant ? 1 : Math.min(1, (t - (lines[m].start + FADE_MS)) / FADE_MS);
		vis = (1 - f) * layerVisible(layer, j, m - 1) + f * layerVisible(layer, j, m);
	}
	return alphaAt(lines, j, t, instant) * vis;
}

export type Track = { t0: number; duration: number; frames: Keyframe[] };

const r3 = (n: number) => Math.round(n * 1000) / 1000;

function toTrack(stops: number[], value: (t: number) => Keyframe, easing: string): Track {
	const ts = [...new Set(stops.map((t) => Math.round(t)))].sort((a, b) => a - b);
	const t0 = ts[0];
	const duration = Math.max(1, ts[ts.length - 1] - t0);
	const frames = ts.map((t) => ({ ...value(t), offset: r3((t - t0) / duration), easing }));
	return { t0, duration, frames };
}

/** Keyframes for one layer of line j, valid for the whole time the line can be on screen (about 5 lines either side). */
export function layerTrack(lines: LineModel[], layer: Layer, j: number, instant = false): Track {
	const n = lines.length;
	const a = lines[Math.max(0, j - 5)].start;
	const b = lines[Math.min(n - 1, j + 5)].start + 2 * LINE_MS;
	const stops = [a, b, lines[j].start, lines[j].start + LINE_MS];
	if (lines[j + 1]) stops.push(lines[j + 1].start, lines[j + 1].start + LINE_MS);
	for (let k = Math.max(1, j - 4); k <= Math.min(n - 1, j + 4); k++) stops.push(lines[k].start + FADE_MS, lines[k].start + 2 * FADE_MS);
	if (instant) {
		// jumps land exactly on the stops: steps(1,end) holds the previous value until the next stop
		const only = stops.filter((t) => t >= a && t <= b);
		const tr = toTrack(only, (t) => ({ opacity: r3(layerOpacity(lines, layer, j, t, true)) }), 'steps(1,end)');
		return tr;
	}
	return toTrack(stops.filter((t) => t >= a && t <= b), (t) => ({ opacity: r3(layerOpacity(lines, layer, j, t)) }), EASE);
}

/** The column: line k sits at the anchor; it travels there over 400 ms after s_k. Positions in px (anchor and pitch measured by the caller). */
export function columnTrack(lines: LineModel[], anchorPx: number, pitchPx: number, instant = false): Track {
	const y = (k: number) => `translateY(${r3(anchorPx - k * pitchPx)}px)`;
	const pts: { t: number; v: string; easing: string }[] = [{ t: 0, v: y(0), easing: 'linear' }];
	for (let k = 1; k < lines.length; k++) {
		const s = Math.max(lines[k].start, pts[pts.length - 1].t + 1);
		const e = Math.max(s + 1, instant ? s + 1 : Math.min(s + COLUMN_MS, lines[k + 1]?.start ?? Infinity));
		pts.push({ t: s, v: y(k - 1), easing: instant ? 'steps(1,end)' : EASE });
		pts.push({ t: e, v: y(k), easing: 'linear' });
	}
	const tail = pts[pts.length - 1].t + 2000;
	pts.push({ t: tail, v: y(Math.max(0, lines.length - 1)), easing: 'linear' });
	const duration = Math.max(1, tail);
	return { t0: 0, duration, frames: pts.map((p) => ({ transform: p.v, offset: r3(p.t / duration), easing: p.easing })) };
}

/** Word sweep: the window slides in while the lit text inside slides out, so the text never moves (two copies, compositor only). */
export const sweepFrames = (side: -1 | 1): Keyframe[] => [{ transform: `translateX(${side * 100}%)` }, { transform: 'translateX(0)' }];

/** WordScale 1 -> 1.0505 (at 70%) -> 1 (the app starts at .95; a shrunk word would widen the gaps around it) and lift +.01 -> -.0108 em. */
export const liftFrames = (): Keyframe[] => [
	{ transform: 'translateY(.01em) scale(1)', offset: 0 },
	{ transform: 'translateY(-.0108em) scale(1.0505)', offset: 0.7 },
	{ transform: 'none', offset: 1 }
];

/** The glow behind a held word: opacity 0 -> 1 (15%) -> 1 (60%) -> 0. */
export const glowFrames = (): Keyframe[] => [
	{ opacity: 0, offset: 0 },
	{ opacity: 1, offset: 0.15 },
	{ opacity: 1, offset: 0.6 },
	{ opacity: 0, offset: 1 }
];

/** Letter k of a held word: lit over its slice of [a, b - 250 ms], ripple scale 1 -> 1.175 -> 1 (it starts at 1, not the app's .95: a shrunk word would widen the gaps around it). Returns the window [t0, t1] and frames. */
export function letterTrack(w: WordModel, k: number, n: number, ripple: boolean): { t0: number; duration: number; frames: Keyframe[] } {
	const span = Math.max(n, w.b - w.a - 250);
	const slice = span / n;
	const t0 = w.a + k * slice;
	const t1 = w.b;
	const duration = Math.max(1, t1 - t0);
	const lit = Math.min(1, slice / duration);
	const f = (op: number, sc: number, off: number): Keyframe => ({ opacity: op, ...(ripple ? { transform: `scale(${sc})` } : {}), offset: off });
	const frames: Keyframe[] = [f(0.412, 1, 0), f(0.6, 1.175, r3(lit * 0.7)), f(1, 1, r3(lit)), f(1, 1, 1)];
	return { t0, duration, frames };
}

/**
 * Stretches a short track (one word's sweep, lift, glow or ripple) into a long one over [from, to] in song time: the value is held before
 * and after the original window. WHY: an animation that starts or ends while others run makes Chrome run a full main-thread frame at the
 * display rate (measured: 60 staggered 300 ms transform animations = 500 main frames in 4 s; the same motion as plateaus inside long
 * animations = 0). The prototype avoids it the same way (one long keyframe track per element). `easing` is applied per keyframe segment.
 */
export function spanTrack(tr: Track, from: number, to: number, easing = 'linear'): Track {
	const t0 = Math.min(from, tr.t0);
	const t1 = Math.max(to, tr.t0 + tr.duration);
	const duration = Math.max(1, t1 - t0);
	const n = tr.frames.length;
	const abs = tr.frames.map((f, i) => ({ f, t: tr.t0 + (f.offset ?? (n > 1 ? i / (n - 1) : 0)) * tr.duration }));
	const frames: Keyframe[] = abs.map(({ f, t }) => ({ ...f, offset: r3((t - t0) / duration), easing: f.easing ?? easing }));
	const first = abs[0];
	if (first.t > t0) frames.unshift({ ...tr.frames[0], offset: 0, easing: 'linear' });
	const last = abs[n - 1];
	if (last.t < t1) frames.push({ ...tr.frames[n - 1], offset: 1, easing: 'linear' });
	// offsets rounded to 3 digits can collide on a very long track: keep them non-decreasing
	for (let i = 1; i < frames.length; i++) if ((frames[i].offset as number) < (frames[i - 1].offset as number)) frames[i].offset = frames[i - 1].offset;
	return { t0, duration, frames };
}
