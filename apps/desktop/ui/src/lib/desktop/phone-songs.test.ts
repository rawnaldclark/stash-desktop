import { describe, expect, it } from 'vitest';
import { NO_FILTERS, artistOf, bytesOf, chosen, complete, detailOf, facets, formatOf, qualityOf, shows, size, type Offered } from './phone-songs';

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

	it('hides songs here already, until asked to show them', () => {
		expect(files(songs.filter((s) => shows(s, NO_FILTERS)))).toEqual(['1.flac', '3.opus', '4.mp3']);
		expect(songs.filter((s) => shows(s, { ...NO_FILTERS, newOnly: false }))).toHaveLength(4);
	});

	it('takes any of the formats, qualities, artists, albums or playlists chosen, and all the filters at once', () => {
		const all = { ...NO_FILTERS, newOnly: false };
		const only = (f: Partial<typeof all>) => files(songs.filter((s) => shows(s, { ...all, ...f })));
		expect(only({ formats: ['FLAC', 'AAC'] })).toEqual(['1.flac', '2.m4a']);
		expect(only({ qualities: ['lossy'] })).toEqual(['2.m4a', '3.opus', '4.mp3']);
		expect(only({ qualities: ['hires'] })).toEqual(['1.flac']);
		expect(only({ artists: ['Other'] })).toEqual(['3.opus']);
		expect(only({ albums: ['Album'] })).toEqual(['1.flac', '2.m4a', '4.mp3']);
		expect(only({ playlists: ['Chill'] })).toEqual(['3.opus']);
		expect(only({ playlists: ['Gym'], qualities: ['lossy'] })).toEqual(['3.opus']);
		expect(only({ completeOnly: true })).toEqual(['1.flac', '2.m4a']);
		expect(only({ likedOnly: true })).toEqual(['1.flac']);
	});

	it('finds words anywhere in the title, artist or album, accents or not', () => {
		const find = (text: string) => files(songs.filter((s) => shows(s, { ...NO_FILTERS, newOnly: false, text })));
		expect(find('cafe')).toEqual(['3.opus']);
		expect(find('other café')).toEqual(['3.opus']);
		expect(find('  ')).toHaveLength(4);
		expect(find('nothing')).toEqual([]);
	});

	it('takes what is shown and not unticked, and keeps an untick while the filters change', () => {
		const unticked = new Set(['3.opus']);
		expect(files(chosen(songs, NO_FILTERS, unticked))).toEqual(['1.flac', '4.mp3']);
		expect(files(chosen(songs, { ...NO_FILTERS, formats: ['Opus', 'MP3'] }, unticked))).toEqual(['4.mp3']);
		expect(bytesOf(chosen(songs, NO_FILTERS, new Set()))).toBe(65e6);
	});

	it('offers each filter’s choices with their counts, the most first', () => {
		const f = facets(songs);
		expect(f.formats.map((x) => [x.value, x.count])).toEqual([['AAC', 1], ['FLAC', 1], ['MP3', 1], ['Opus', 1]]);
		expect(f.qualities.map((x) => [x.value, x.count])).toEqual([['hires', 1], ['lossy', 3]]);
		expect(f.artists.map((x) => [x.value, x.count])).toEqual([['Band', 3], ['Other', 1]]);
		expect(f.albums.map((x) => [x.value, x.count])).toEqual([['Album', 3]]);
		expect(f.playlists.map((x) => [x.value, x.count])).toEqual([['Gym', 2], ['Chill', 1]]);
	});

	it('says sizes as people do', () => {
		expect(size(4.14e9)).toBe('4.1 GB');
		expect(size(380e6)).toBe('380 MB');
		expect(size(2e5)).toBe('1 MB');
		expect(size(0)).toBe('0 MB');
	});
});
