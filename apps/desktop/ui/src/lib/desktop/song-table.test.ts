import { describe, expect, it } from 'vitest';
import { COLUMNS, DEFAULT_COLUMNS, formatOf, gridOf, readColumns } from './song-table';

describe('the table’s columns', () => {
	it('start as the default, with the title always there', () => {
		expect(readColumns(null)).toEqual(DEFAULT_COLUMNS);
		expect(readColumns('')).toEqual(DEFAULT_COLUMNS);
		expect(readColumns('nonsense')).toEqual(DEFAULT_COLUMNS);
		expect(readColumns('genre')).toEqual(['title', 'genre']);
	});

	it('keep the table’s own order, whatever order they were chosen in', () => {
		expect(readColumns('plays,year,title,track,cover')).toEqual(['cover', 'track', 'title', 'year', 'plays']);
	});

	it('make one grid track a column', () => {
		const cols = COLUMNS.filter((c) => ['title', 'year'].includes(c.id));
		expect(gridOf(cols)).toBe('minmax(140px, 3fr) 60px');
	});
});

describe('a song’s format', () => {
	it('reads as a listener writes it', () => {
		expect(formatOf({ codec: 'flac', bit_depth: 24, sample_rate: 96000, bitrate: 2800 })).toBe('FLAC 24/96');
		expect(formatOf({ codec: 'flac', bit_depth: 16, sample_rate: 44100, bitrate: 900 })).toBe('FLAC 16/44.1');
		expect(formatOf({ codec: 'mp3', bit_depth: null, sample_rate: 44100, bitrate: 320 })).toBe('MP3 320');
		expect(formatOf({ codec: 'aac', bit_depth: null, sample_rate: null, bitrate: null })).toBe('AAC');
		expect(formatOf({ codec: 'pcm', bit_depth: 16, sample_rate: 48000, bitrate: null })).toBe('WAV 16/48');
		expect(formatOf({ codec: null, bit_depth: null, sample_rate: null, bitrate: null })).toBe('');
	});
});
