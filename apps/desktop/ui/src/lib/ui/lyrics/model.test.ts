import { describe, expect, it } from 'vitest';
import { OP, activeIndex, isRtlWord, seekTarget, alphaAt, buildLines, columnTrack, layerOpacity, layerTrack, layerVisible, letterTrack, spanTrack, sweepFrames, liftFrames, LAYERS } from './model';

const synced = [
	{ startMs: 1000, endMs: 4000, text: 'Lamplight on the glass', words: [{ startMs: 1000, endMs: 1800, text: 'Lamplight' }, { startMs: 1800, endMs: 2200, text: 'on' }, { startMs: 2200, endMs: 2600, text: 'the' }, { startMs: 2600, endMs: 3900, text: 'glass' }] },
	{ startMs: 4000, text: 'No signal on the bridge' },
	{ startMs: 7600, text: 'Every song we saved' },
	{ startMs: 11200, text: 'Right where we left it' },
	{ startMs: 14800, text: 'Hum it to the end' },
	{ startMs: 18400, text: 'Light under the door' },
	{ startMs: 22000, text: 'Hold the last word' }
];
const lines = buildLines(synced);

describe('buildLines', () => {
	it('keeps word timing, derives ends, marks long short words as held', () => {
		expect(lines).toHaveLength(7);
		expect(lines[0].timed).toBe(true);
		expect(lines[0].words[3]).toMatchObject({ a: 2600, b: 3900, held: true });
		expect(lines[0].words[1].held).toBe(false);
	});
	it('line-synced lines become one quick sweep at the line start', () => {
		expect(lines[1].timed).toBe(false);
		expect(lines[1].words).toEqual([{ a: 4000, b: 4250, text: 'No signal on the bridge', held: false }]);
	});
	it('clamps a line end to the next start and fits long lines', () => {
		expect(lines[1].end).toBe(7600);
		const long = buildLines([{ startMs: 0, text: 'x'.repeat(60) }]);
		expect(long[0].fit).toBeLessThan(1);
		expect(long[0].fit).toBeGreaterThanOrEqual(0.6);
	});
	it('drops empty lines and sorts', () => {
		const l = buildLines([{ startMs: 5, text: 'b' }, { startMs: 1, text: 'a' }, { startMs: 3, text: '  ' }]);
		expect(l.map((x) => x.text)).toEqual(['a', 'b']);
	});
});

describe('activeIndex', () => {
	it('is 0 during the intro and the last started line afterwards', () => {
		expect(activeIndex(lines, 0)).toBe(0);
		expect(activeIndex(lines, 3999)).toBe(0);
		expect(activeIndex(lines, 4000)).toBe(1);
		expect(activeIndex(lines, 12000)).toBe(3);
		expect(activeIndex(lines, 1e9)).toBe(6);
		expect(activeIndex([], 5)).toBe(-1);
	});
});

describe('line alpha (the app: active 1, sung .497, upcoming .51 over 300 ms)', () => {
	it('upcoming, ramp, active, ramp, sung', () => {
		expect(alphaAt(lines, 1, 0)).toBe(OP.upcoming);
		expect(alphaAt(lines, 1, 4150)).toBeCloseTo((OP.upcoming + OP.active) / 2, 3);
		expect(alphaAt(lines, 1, 5000)).toBe(OP.active);
		expect(alphaAt(lines, 1, 7600 + 150)).toBeCloseTo((OP.active + OP.sung) / 2, 3);
		expect(alphaAt(lines, 1, 20000)).toBe(OP.sung);
	});
	it('the last line stays active; reduced motion steps', () => {
		expect(alphaAt(lines, 6, 99999)).toBe(OP.active);
		expect(alphaAt(lines, 1, 3999, true)).toBe(OP.upcoming);
		expect(alphaAt(lines, 1, 4000, true)).toBe(OP.active);
		expect(alphaAt(lines, 1, 7600, true)).toBe(OP.sung);
	});
});

describe('distance blur layers', () => {
	it('shows the sharp line within one, near copies at two, far copies at three or more; dim before, lit after', () => {
		expect(layerVisible('ls', 3, 3)).toBe(1);
		expect(layerVisible('ls', 4, 3)).toBe(1);
		expect(layerVisible('ls', 5, 3)).toBe(0);
		expect(layerVisible('nd', 5, 3)).toBe(1);
		expect(layerVisible('nl', 1, 3)).toBe(1);
		expect(layerVisible('nl', 5, 3)).toBe(0);
		expect(layerVisible('fd', 6, 3)).toBe(1);
		expect(layerVisible('fl', 0, 3)).toBe(1);
		expect(layerVisible('fl', 6, 3)).toBe(0);
	});
	it('exactly one layer carries a line at any instant (cross-fades sum to one)', () => {
		for (const j of [0, 2, 4, 6]) {
			for (const t of [0, 1000, 4100, 4200, 7650, 7700, 12000, 15000, 22100]) {
				const total = LAYERS.reduce((a, l) => a + layerOpacity(lines, l, j, t), 0);
				expect(total).toBeCloseTo(alphaAt(lines, j, t), 3);
			}
		}
	});
	it('layer tracks are well-formed keyframes in song time', () => {
		for (const instant of [false, true]) {
			const tr = layerTrack(lines, 'ls', 2, instant);
			expect(tr.duration).toBeGreaterThan(0);
			expect(tr.frames[0].offset).toBe(0);
			expect(tr.frames[tr.frames.length - 1].offset).toBe(1);
			const offs = tr.frames.map((f) => f.offset as number);
			expect([...offs].sort((a, b) => a - b)).toEqual(offs);
		}
	});
});

describe('column track', () => {
	it('line k sits at the anchor after its 400 ms step', () => {
		const tr = columnTrack(lines, 100, 40);
		expect(tr.t0).toBe(0);
		expect(tr.frames[0].transform).toBe('translateY(100px)');
		expect(tr.frames[tr.frames.length - 1].transform).toBe(`translateY(${100 - 6 * 40}px)`);
		const offs = tr.frames.map((f) => f.offset as number);
		expect([...offs].sort((a, b) => a - b)).toEqual(offs);
	});
	it('reduced motion jumps (steps)', () => {
		const tr = columnTrack(lines, 100, 40, true);
		expect(tr.frames.some((f) => f.easing === 'steps(1,end)')).toBe(true);
	});
});

describe('held letters', () => {
	it('light over the slices of [a, b - 250 ms] and end at b', () => {
		const w = lines[0].words[3];
		const first = letterTrack(w, 0, 5, true);
		const last = letterTrack(w, 4, 5, true);
		expect(first.t0).toBe(w.a);
		expect(last.t0).toBeGreaterThan(first.t0);
		expect(last.t0 + last.duration).toBe(w.b);
		expect(letterTrack(w, 0, 5, false).frames[0].transform).toBeUndefined();
	});
});

describe('spanTrack (plateaus inside one long animation, so nothing starts or ends mid-song)', () => {
	const win = { t0: 5000, duration: 400, frames: sweepFrames(-1) };
	it('holds the first value before the window and the last one after it, over [from, to]', () => {
		const tr = spanTrack(win, 2000, 12000);
		expect(tr.t0).toBe(2000);
		expect(tr.duration).toBe(10000);
		expect(tr.frames.map((f) => f.offset)).toEqual([0, 0.3, 0.34, 1]);
		expect(tr.frames[0].transform).toBe('translateX(-100%)');
		expect(tr.frames[1].transform).toBe('translateX(-100%)');
		expect(tr.frames[2].transform).toBe('translateX(0)');
		expect(tr.frames[3].transform).toBe('translateX(0)');
	});
	it('keeps the motion on the same song times and puts the effect easing on each segment', () => {
		const tr = spanTrack({ t0: 1000, duration: 1000, frames: liftFrames() }, 0, 4000, 'ease-out');
		const at = (i: number) => tr.t0 + (tr.frames[i].offset as number) * tr.duration;
		expect([at(1), at(2), at(3)]).toEqual([1000, 1700, 2000]);
		expect(tr.frames[2].easing).toBe('ease-out');
		expect(tr.frames[0].easing).toBe('linear');
	});
	it('never shrinks the window and keeps offsets non-decreasing', () => {
		const tr = spanTrack(win, 5100, 5200);
		expect(tr.t0).toBe(5000);
		expect(tr.duration).toBe(400);
		const o = tr.frames.map((f) => f.offset as number);
		expect(o).toEqual([...o].sort((a, b) => a - b));
	});
});

describe('right-to-left lyrics', () => {
	const heb = buildLines([{ startMs: 0, text: 'שלום עולם', words: [{ startMs: 0, endMs: 1800, text: 'שלום' }, { startMs: 1800, endMs: 2200, text: 'hello' }, { startMs: 2200, endMs: 4000, text: 'עולם' }] }]);
	const ara = buildLines([{ startMs: 0, text: 'مرحبا بالعالم' }]);
	it('detects a right-to-left word by its first letter, ignoring digits and punctuation', () => {
		expect(isRtlWord('שלום')).toBe(true);
		expect(isRtlWord('مرحبا')).toBe(true);
		expect(isRtlWord('"שלום')).toBe(true);
		expect(isRtlWord('hello')).toBe(false);
		expect(isRtlWord('123')).toBe(false);
	});
	it('flags each word and never splits a right-to-left word into letters (it would break the joined shapes)', () => {
		expect(heb[0].words.map((w) => !!w.rtl)).toEqual([true, false, true]);
		expect(heb[0].words[0].held).toBe(false);
		expect(ara[0].words[0].rtl).toBe(true);
	});
	it('never splits a word with combining marks (Devanagari vowel signs, a decomposed accent) into letters', () => {
		const long = (text: string) => buildLines([{ startMs: 0, text, words: [{ startMs: 0, endMs: 3000, text }] }])[0].words[0];
		expect(long('हूँ').held).toBe(false);
		expect(long('café').held).toBe(false);
		expect(long('love').held).toBe(true);
	});
});

describe('seekTarget', () => {
	it('lands the displayed clock on the line: the line start plus the song offset, never negative', () => {
		expect(seekTarget({ start: 4000 }, 0)).toBe(4000);
		expect(seekTarget({ start: 4000 }, 1500)).toBe(5500);
		expect(seekTarget({ start: 1000 }, -3000)).toBe(0);
	});
});
