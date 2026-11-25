import { describe, expect, it } from 'vitest';
import { BROWSE, COLLECTIONS, isBrowse, isCollection, keysFor, sectionForKey } from './library-sections';
import { KEY_GROUPS } from '../ui/keys';

describe('library sections', () => {
	it('Ctrl+1 to Ctrl+6 open the chips in their order, and no other key opens one', () => {
		expect(BROWSE.map((b, i) => sectionForKey(String(i + 1)))).toEqual(BROWSE.map((b) => b.id));
		expect(sectionForKey('7')).toBeNull();
		expect(sectionForKey('0')).toBeNull();
		expect(sectionForKey('k')).toBeNull();
		expect(sectionForKey('12')).toBeNull();
		expect(keysFor('songs')).toBe('Ctrl+1');
		expect(keysFor('folders')).toBe('Ctrl+6');
	});

	it('a tab is a section or a collection, never both', () => {
		for (const b of BROWSE) expect(isCollection(b.id)).toBe(false);
		for (const c of Object.keys(COLLECTIONS)) expect(isBrowse(c)).toBe(false);
		expect(isBrowse('albums')).toBe(true);
		expect(isCollection('likes')).toBe(true);
		expect(isCollection('toString')).toBe(false);
		expect(isBrowse(null)).toBe(false);
	});

	it('the keys sheet names every section with the keys that open it', () => {
		const going = KEY_GROUPS.find((g) => g.title === 'Going')!;
		for (const [i, b] of BROWSE.entries()) {
			expect(going.rows).toContainEqual({ keys: [['Ctrl', String(i + 1)]], does: `Library › ${b.label}` });
		}
	});
});
