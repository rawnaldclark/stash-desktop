import { describe, expect, it } from 'vitest';
import type { HistoryEntry, LibraryItem } from '../api';
import { JUMP, jumpBackIn } from './jump';

const DAY = 24 * 3600_000;
const NOW = Date.UTC(2026, 9, 7, 12);
const song = (key: string, album: string, artist = 'A'): LibraryItem => ({ key, title: key, artist, album, refs: {}, addedAt: 0 }) as LibraryItem;
const plays = (item: LibraryItem, ...daysAgo: number[]): HistoryEntry[] => daysAgo.map((d) => ({ item, playedAt: NOW - d * DAY }));

describe('jumpBackIn', () => {
	const a1 = song('a1', 'Alpha'), a2 = song('a2', 'Alpha');
	const b1 = song('b1', 'Beta');
	const c1 = song('c1', 'Gamma');
	const d1 = song('d1', 'Delta');
	const base = [...plays(a1, 3, 5), ...plays(a2, 6), ...plays(b1, 4, 9, 10), ...plays(c1, 2, 8), ...plays(d1, 7, 12)];

	it('returns old favourites, most played first, each with its most played songs first', () => {
		const out = jumpBackIn(base, new Set(), NOW);
		// Beta and Alpha both have 3 plays: the one played longest ago comes first; then Delta and Gamma (2 plays each), same rule.
		expect(out.map((g) => g[0].album)).toEqual(['Beta', 'Alpha', 'Delta', 'Gamma']);
		expect(out[1].map((s) => s.key)).toEqual(['a1', 'a2']);
	});

	it('never repeats what Recently played shows, and skips albums played in the last day', () => {
		const e1 = song('e1', 'Epsilon');
		const recent = new Set(['b1']);
		const fresh = [...base, ...plays(e1, 11, 13), ...plays(c1, 0.2)];
		expect(jumpBackIn(fresh, recent, NOW).map((g) => g[0].album)).toEqual(['Alpha', 'Epsilon', 'Delta']);
		expect(JUMP.minAlbums).toBe(2);
	});

	it('needs repeat listening inside the window, and enough albums to be worth a shelf', () => {
		const once = [...plays(a1, 3), ...plays(b1, 4), ...plays(c1, 5), ...plays(d1, 6)];
		expect(jumpBackIn(once, new Set(), NOW)).toEqual([]);
		const old = [...plays(a1, 40, 41), ...plays(b1, 42, 43), ...plays(c1, 44, 45)];
		expect(jumpBackIn(old, new Set(), NOW)).toEqual([]);
		const one = [...plays(a1, 3, 4)];
		expect(jumpBackIn(one, new Set(), NOW)).toEqual([]);
	});

	it('ignores songs without an album and matches albums case-insensitively', () => {
		const loose = song('x', '');
		const mixed = [...plays(song('m1', 'Same', 'Band'), 3), ...plays(song('m2', 'SAME', 'band'), 4), ...plays(loose, 3, 4), ...plays(b1, 5, 6), ...plays(c1, 7, 8)];
		expect(jumpBackIn(mixed, new Set(), NOW).map((g) => g.length)).toEqual([1, 1, 2]);
	});
});
