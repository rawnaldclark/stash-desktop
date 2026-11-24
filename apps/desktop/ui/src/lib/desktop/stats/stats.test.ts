import { describe, expect, it } from 'vitest';
import { hours, monthNames, niceTop, percent, rowHref, tick } from './stats';

describe('a long time', () => {
	it('is said in whole hours', () => {
		expect([hours(0), hours(40 * 60_000), hours(3_600_000), hours(27.4 * 3_600_000), hours(1234 * 3_600_000)]).toEqual(['under an hour', '1 hour', '1 hour', '27 hours', '1,234 hours']);
	});
});

describe('an axis', () => {
	it('tops out at a clean number at least as high as the data', () => {
		expect([niceTop(0), niceTop(1), niceTop(3), niceTop(7), niceTop(12), niceTop(312), niceTop(1000), niceTop(1001)]).toEqual([1, 1, 5, 10, 20, 500, 1000, 2000]);
	});
	it('says its ticks in full under ten thousand', () => {
		expect([tick(0), tick(250), tick(2000), tick(15000)]).toEqual(['0', '250', '2,000', '15K']);
	});
});

describe('a share', () => {
	it('is a whole percent that never claims all or nothing wrongly', () => {
		expect([percent(0, 0), percent(0, 10), percent(1, 400), percent(399, 400), percent(400, 400), percent(148, 402)]).toEqual(['0%', '0%', '<1%', '>99%', '100%', '37%']);
	});
});

describe('a month', () => {
	it('has a short and a full name', () => {
		const m = monthNames('2026-11');
		expect(m.full).toContain('2026');
		expect(m.short.length).toBeLessThan(m.full.length);
		expect(monthNames('nonsense')).toEqual({ short: 'nonsense', full: 'nonsense' });
	});
});

describe('a row', () => {
	it('opens what it counts', () => {
		expect(rowHref('genres', { label: 'Jazz', value: 3, key: 'Jazz' })).toBe('/library?tab=songs&genre=Jazz');
		expect(rowHref('genres', { label: 'No genre', value: 3, key: '' })).toBe('/library?tab=songs&genre=');
		expect(rowHref('genres', { label: 'Other genres', value: 3 })).toBeUndefined();
		expect(rowHref('decades', { label: '1970s', value: 3, key: '1970' })).toBe('/library?tab=songs&decade=1970');
		expect(rowHref('artists', { label: 'Cher', value: 3, key: '41' })).toBe('/artist/library/41');
	});
});
