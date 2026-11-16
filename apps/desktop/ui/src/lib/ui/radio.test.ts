import { describe, expect, it } from 'vitest';
import type { Station } from '../core';
import { findingWords, nothingWords, stationLine } from './radio';

const station = (over: Partial<Station> = {}): Station => ({
	kind: 'radio',
	label: 'Teardrop radio',
	seed: { title: 'Teardrop', artist: 'Massive Attack' },
	via: 'lastfm',
	why: null,
	finding: false,
	added: 3,
	from_qid: 12,
	...over
});

describe('stationLine (the line above a station’s songs in Up next)', () => {
	it('none without a station', () => {
		expect(stationLine(null)).toBeNull();
		expect(stationLine(undefined)).toBeNull();
	});

	it('a radio Last.fm chose for credits Last.fm', () => {
		expect(stationLine(station())).toEqual({ lead: 'Similar to Teardrop · from', lastfm: true, tip: null, stopped: 'Radio stopped' });
		expect(stationLine(station({ seed: { artist: 'Massive Attack' }, label: 'Massive Attack radio' }))?.lead).toBe('Similar to Massive Attack · from');
	});

	it('a radio the library chose says why', () => {
		const line = stationLine(station({ via: 'library', why: 'Last.fm could not be reached' }));
		expect(line).toEqual({ lead: 'Songs like Teardrop from your library', lastfm: false, tip: 'Last.fm could not be reached', stopped: 'Radio stopped' });
	});

	it('autoplay, from Last.fm or from the library', () => {
		expect(stationLine(station({ kind: 'autoplay' }))?.lead).toBe('Autoplay · like Teardrop · from');
		const library = stationLine(station({ kind: 'autoplay', via: 'library', seed: null, label: '' }));
		expect(library).toEqual({ lead: 'Autoplay · from your library', lastfm: false, tip: null, stopped: 'Autoplay stopped' });
	});

	it('what it says while it finds its first songs', () => {
		expect(findingWords(station())).toBe('Finding songs like Teardrop…');
		expect(findingWords(station({ seed: { artist: 'Massive Attack' }, label: 'Massive Attack radio' }))).toBe('Finding songs for Massive Attack radio…');
	});

	it('what it says when it found nothing, and why', () => {
		expect(nothingWords(station({ via: 'library', why: 'Last.fm could not be reached', from_qid: null }))).toBe(
			'No songs like Teardrop were found (Last.fm could not be reached).'
		);
		expect(nothingWords(station({ seed: { artist: 'Massive Attack' }, from_qid: null }))).toBe('No songs like Massive Attack were found.');
	});
});
