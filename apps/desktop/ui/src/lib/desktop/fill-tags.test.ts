import { describe, expect, it } from 'vitest';
import type { FillNow } from '../core';
import { fillLine, offerLine } from './fill-tags';

const now = (over: Partial<FillNow>): FillNow => ({ blanks: 254, running: false, total: 0, done: 0, missed: 0, problem: null, found: [], ...over });
const offer = (id: number) => ({ id, title: 'Nude', artist: 'Radiohead', set: { genre: 'Alternative' } });

describe('where the lookup is', () => {
	it('says how many files have blanks before it starts', () => {
		expect(fillLine(now({}))).toBe('254 files on this computer are missing an album, year, track number or genre.');
		expect(fillLine(now({ blanks: 1 }))).toBe('1 file on this computer is missing an album, year, track number or genre.');
		expect(fillLine(now({ blanks: 0 }))).toBe('Every file on this computer has its album, year, track number and genre.');
	});

	it('counts as it goes, and sums up at the end', () => {
		expect(fillLine(now({ running: true, total: 254, done: 12, found: [offer(1), offer(2)] }))).toBe('Looking up 254 files: 12 done, about 13 minutes left. You can write what is found as it comes, and close this while it works.');
		expect(fillLine(now({ total: 254, done: 254, missed: 30, found: [offer(1)] }))).toBe('Found tags for 1 file. Untick any that look wrong, then write the rest. The catalogue had nothing new for 30.');
		expect(fillLine(now({ blanks: 30, total: 30, done: 30, missed: 30 }))).toBe('The catalogue had nothing new for the 30 files still missing tags. Edit tags fills those by hand.');
	});
});

describe('what would be written for a song', () => {
	it('is one line of only what is new', () => {
		expect(offerLine({ album: 'In Rainbows', year: '2007', track_no: '3', genre: 'Alternative' })).toBe('In Rainbows \u00b7 2007 \u00b7 track 3 \u00b7 Alternative');
		expect(offerLine({ genre: 'Jazz' })).toBe('Jazz');
		expect(offerLine({ album: 'The Wall', track_no: '4', disc_no: '2' })).toBe('The Wall \u00b7 track 4, disc 2');
	});
});
