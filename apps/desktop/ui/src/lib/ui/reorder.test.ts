import { describe, expect, it } from 'vitest';
import { edgeSpeed, EDGE, MAX_SPEED, targetIndex } from './reorder';

// Rows 60 px high starting at 0: midpoints 30, 90, 150, 210, 270.
const mids = [30, 90, 150, 210, 270];

describe('targetIndex', () => {
	it('stays put until the centre passes a neighbour midpoint', () => {
		expect(targetIndex(mids, 1, 90)).toBe(1);
		expect(targetIndex(mids, 1, 95)).toBe(1);
		expect(targetIndex(mids, 1, 151)).toBe(2);
		expect(targetIndex(mids, 1, 29)).toBe(0);
	});
	it('moves several rows at once and clamps at the ends', () => {
		expect(targetIndex(mids, 0, 215)).toBe(3);
		expect(targetIndex(mids, 0, 5000)).toBe(4);
		expect(targetIndex(mids, 4, -5000)).toBe(0);
		expect(targetIndex(mids, 3, 91)).toBe(2);
	});
	it('a one-row list never moves', () => {
		expect(targetIndex([30], 0, 500)).toBe(0);
	});
});

describe('edgeSpeed', () => {
	it('is zero in the middle and ramps up toward each edge', () => {
		expect(edgeSpeed(300, 0, 600)).toBe(0);
		expect(edgeSpeed(EDGE, 0, 600)).toBe(0);
		expect(edgeSpeed(EDGE / 2, 0, 600)).toBeCloseTo(-MAX_SPEED / 2);
		expect(edgeSpeed(0, 0, 600)).toBe(-MAX_SPEED);
		expect(edgeSpeed(-50, 0, 600)).toBe(-MAX_SPEED);
		expect(edgeSpeed(600 - EDGE / 2, 0, 600)).toBeCloseTo(MAX_SPEED / 2);
		expect(edgeSpeed(700, 0, 600)).toBe(MAX_SPEED);
	});
	it('works for a scroller that is not at the top of the page', () => {
		expect(edgeSpeed(100, 100, 500)).toBe(-MAX_SPEED);
		expect(edgeSpeed(300, 100, 500)).toBe(0);
	});
});
