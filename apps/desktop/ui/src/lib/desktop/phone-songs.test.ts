import { describe, expect, it } from 'vitest';
import { LIKED, artistOf, bytesOf, complete, detailOf, facet, formatOf, keysOf, narrowed, noFilters, qualityOf, shows, size, toTake, type Filters, type Offered } from './phone-songs';

const song = (o: Partial<Offered>): Offered => ({
	file: '1.flac',
	title: 'Song',
	artist: 'Band',
	album: 'Album',
	album_artist: null,
	format: 'flac',
	bits: 16,
	sample_rate: 44100,
	duration_ms: 200_000,
	art_url: 'https://cover',
	liked: false,
	playlists: [],
	bytes: 30e6,
	here: false,
	...o
});

describe('a phone song’s format and quality', () => {
	it('are what the phone says, else what its file’s extension says', () => {
		expect(formatOf(song({ format: 'flac' }))).toBe('FLAC');
		expect(formatOf(song({ format: 'aac', file: '2.m4a' }))).toBe('AAC');
		expect(formatOf(song({ format: null, file: '3.m4a' }))).toBe('AAC');
		expect(formatOf(song({ format: null, file: '4.opus' }))).toBe('Opus');
		expect(formatOf(song({ format: 'alac', file: '5.m4a' }))).toBe('ALAC');
		expect(formatOf(song({ format: '', file: 'noext' }))).toBe('Other');
		expect(formatOf(song({ format: 'dsd' }))).toBe('DSD');
	});

	it('count lossless past 16-bit or 48 kHz as hi-res, as Statistics does', () => {
		expect(qualityOf(song({ bits: 24, sample_rate: 96000 }))).toBe('hires');
		expect(qualityOf(song({ bits: 16, sample_rate: 96000 }))).toBe('hires');
		expect(qualityOf(song({ bits: 16, sample_rate: 44100 }))).toBe('cd');
		expect(qualityOf(song({ bits: null, sample_rate: null }))).toBe('cd');
		expect(qualityOf(song({ format: 'opus', bits: 24, sample_rate: 48000 }))).toBe('lossy');
		expect(detailOf(song({ bits: 24, sample_rate: 44100 }))).toBe('FLAC · 24/44.1');
		expect(detailOf(song({ format: 'aac', bits: 16, sample_rate: 44100 }))).toBe('AAC');
		expect(detailOf(song({ bits: null }))).toBe('FLAC');
	});

	it('are complete with a title, an artist, an album and a cover', () => {
		expect(complete(song({}))).toBe(true);
		expect(complete(song({ album: null }))).toBe(false);
		expect(complete(song({ album: '  ' }))).toBe(false);
		expect(complete(song({ art_url: null }))).toBe(false);
		expect(complete(song({ artist: '' }))).toBe(false);
	});

	it('are by their album’s artist when the phone says', () => {
		expect(artistOf(song({ artist: 'Band feat. Guest', album_artist: 'Band' }))).toBe('Band');
		expect(artistOf(song({ artist: 'Band', album_artist: ' ' }))).toBe('Band');
		expect(artistOf(song({ artist: '', album_artist: null }))).toBe('Unknown artist');
	});
});

describe('choosing a phone’s songs', () => {
	const songs = [
		song({ file: '1.flac', title: 'Hi', bits: 24, sample_rate: 96000, playlists: ['Gym'], liked: true }),
		song({ file: '2.m4a', title: 'Again', format: 'aac', here: true }),
		song({ file: '3.opus', title: 'Café', artist: 'Other', format: 'opus', album: null, playlists: ['Gym', 'Chill'], bytes: 5e6 }),
		song({ file: '4.mp3', title: 'Old', format: 'mp3', art_url: null })
	];
	const files = (list: Offered[]) => list.map((s) => s.file);
	const only = (f: Partial<Filters>) => files(songs.filter((s) => shows(s, { ...noFilters(), showHere: true, ...f })));

	it('hides songs here already, until asked to show them', () => {
		expect(files(songs.filter((s) => shows(s, noFilters())))).toEqual(['1.flac', '3.opus', '4.mp3']);
		expect(only({})).toHaveLength(4);
	});

	it('shows any of what is ticked in a filter, and what every filter at once allows', () => {
		expect(only({ picked: { format: ['FLAC', 'AAC'] } })).toEqual(['1.flac', '2.m4a']);
		expect(only({ picked: { quality: ['Lossy'] } })).toEqual(['2.m4a', '3.opus', '4.mp3']);
		expect(only({ picked: { quality: ['Hi-res'] } })).toEqual(['1.flac']);
		expect(only({ picked: { artist: ['Other'] } })).toEqual(['3.opus']);
		expect(only({ picked: { album: ['Album · Band'] } })).toEqual(['1.flac', '2.m4a', '4.mp3']);
		expect(only({ picked: { playlist: ['Chill'] } })).toEqual(['3.opus']);
		expect(only({ picked: { playlist: ['Gym'], quality: ['Lossy'] } })).toEqual(['3.opus']);
		expect(only({ picked: { tags: ['Complete tags'] } })).toEqual(['1.flac', '2.m4a']);
		expect(only({ picked: { playlist: [LIKED] } })).toEqual(['1.flac']);
		expect(only({ picked: { format: [] } })).toHaveLength(4);
	});

	it('finds words anywhere in the title, artist or album, accents or not', () => {
		expect(only({ text: 'cafe' })).toEqual(['3.opus']);
		expect(only({ text: 'other café' })).toEqual(['3.opus']);
		expect(only({ text: '  ' })).toHaveLength(4);
		expect(only({ text: 'nothing' })).toEqual([]);
	});

	it('knows when the songs shown are narrowed', () => {
		expect(narrowed(noFilters())).toBe(false);
		expect(narrowed({ ...noFilters(), showHere: true, picked: { format: [] } })).toBe(false);
		expect(narrowed({ ...noFilters(), text: 'x' })).toBe(true);
		expect(narrowed({ ...noFilters(), picked: { artist: ['Band'] } })).toBe(true);
	});

	it('takes only the songs chosen, never one here already, whatever is shown', () => {
		expect(toTake(songs, new Set())).toEqual([]);
		expect(files(toTake(songs, new Set(['1.flac', '2.m4a', '4.mp3', 'gone.flac'])))).toEqual(['1.flac', '4.mp3']);
		expect(bytesOf(toTake(songs, new Set(['1.flac', '3.opus'])))).toBe(35e6);
	});

	it('offers each filter’s choices with their counts, the most first, the liked songs with the playlists', () => {
		const fresh = songs.filter((s) => !s.here);
		const counts = (id: Parameters<typeof facet>[1]) => facet(fresh, id).map((x) => [x.value, x.count]);
		expect(counts('format')).toEqual([['FLAC', 1], ['MP3', 1], ['Opus', 1]]);
		expect(counts('quality')).toEqual([['Lossy', 2], ['Hi-res', 1]]);
		expect(counts('artist')).toEqual([['Band', 2], ['Other', 1]]);
		expect(counts('album')).toEqual([['Album · Band', 2]]);
		expect(counts('playlist')).toEqual([['Gym', 2], ['Chill', 1], [LIKED, 1]]);
		expect(counts('tags')).toEqual([['Missing tags', 2], ['Complete tags', 1]]);
		expect(keysOf(song({ album: ' Live ', album_artist: 'Band ' }), 'album')).toEqual(['Live · Band']);
	});

	it('says sizes as people do', () => {
		expect(size(4.14e9)).toBe('4.1 GB');
		expect(size(380e6)).toBe('380 MB');
		expect(size(2e5)).toBe('1 MB');
		expect(size(0)).toBe('0 MB');
	});
});
