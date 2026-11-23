import { describe, expect, it } from 'vitest';
import type { TagsNow } from '../core';
import { changedFields, editSaid } from './edit-tags';

const same = (value: string | null) => ({ value });
const now: TagsNow = {
	files: 3, others: 0,
	title: same(null), artist: same('Miles Davis'), album: same('Kind of Blue'), album_artist: same(''), genre: same(null), year: same('1959'), track_no: same(null), disc_no: same('1'), composer: same('')
};

describe('the fields an edit writes', () => {
	it('are only the ones typed in', () => {
		expect(changedFields(now, {})).toEqual({});
		expect(changedFields(now, { genre: ' Jazz ' })).toEqual({ genre: 'Jazz' });
	});

	it('leave out a field typed back to what every song says', () => {
		expect(changedFields(now, { artist: 'Miles Davis', year: ' 1959 ', album: 'Kind Of Blue' })).toEqual({ album: 'Kind Of Blue' });
	});

	it('clear a field that is emptied, and a differing one too', () => {
		expect(changedFields(now, { album: '', genre: '' })).toEqual({ album: '', genre: '' });
	});

	it('do not write an empty field that was empty in every song', () => {
		expect(changedFields(now, { composer: '  ', album_artist: '' })).toEqual({});
	});
});

describe('what an edit did', () => {
	it('is said in one line', () => {
		expect(editSaid({ written: 1, skipped: 0, failed: [] })).toBe('Saved to 1 file');
		expect(editSaid({ written: 148, skipped: 2, failed: [] })).toBe('Saved to 148 files');
		expect(editSaid({ written: 2, skipped: 0, failed: [[7, 'the file is in use']] })).toBe('Saved to 2 files · 1 file could not be written (the file is in use)');
		expect(editSaid({ written: 0, skipped: 0, failed: [[7, 'read-only'], [8, 'read-only']] })).toBe('Nothing was written · 2 files could not be written (read-only)');
	});
});
