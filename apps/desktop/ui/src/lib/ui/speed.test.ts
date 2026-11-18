import { describe, expect, it } from 'vitest';
import { readSpeed, readTuning, speedLabel, speedMark, tuningHeading, tuningMark } from './speed';

describe('playback speed and tuning in words', () => {
	it('names speeds as Android does', () => {
		expect(speedLabel(1)).toBe('Normal');
		expect(speedLabel(1.25)).toBe('1.25x');
		expect(speedMark(1)).toBeNull();
		expect(speedMark(0.5)).toBe('0.5x');
	});
	it('names tunings', () => {
		expect(tuningMark(440)).toBeNull();
		expect(tuningMark(432)).toBe('A432');
		expect(tuningHeading(440)).toBe('Tuning · Standard');
		expect(tuningHeading(431.5)).toBe('Tuning · A = 431.5 Hz');
	});
	it('reads a typed speed within 0.1x to 4x, two places at most, a comma as a point', () => {
		expect(readSpeed('')).toEqual({ value: null, wrong: false });
		expect(readSpeed('1,1')).toEqual({ value: 1.1, wrong: false });
		expect(readSpeed('4')).toEqual({ value: 4, wrong: false });
		expect(readSpeed('9').wrong).toBe(true);
		expect(readSpeed('0.05').wrong).toBe(true);
		expect(readSpeed('1.234').wrong).toBe(true);
		expect(readSpeed('fast').wrong).toBe(true);
	});
	it('reads a typed tuning within 392 to 494 Hz, one place at most', () => {
		expect(readTuning('431.5')).toEqual({ value: 431.5, wrong: false });
		expect(readTuning('500').wrong).toBe(true);
		expect(readTuning('431.55').wrong).toBe(true);
	});
});
