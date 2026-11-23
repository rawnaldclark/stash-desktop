import { describe, expect, it } from 'vitest';
import { diskSize, longTime, statTiles } from './local-home';

describe('how long the music is', () => {
	it('is said in the two biggest units', () => {
		expect(longTime(20_000)).toBe('under a minute');
		expect(longTime(42 * 60_000)).toBe('42 minutes');
		expect(longTime(60 * 60_000)).toBe('1 hour');
		expect(longTime((5 * 60 + 12) * 60_000)).toBe('5 hours 12 minutes');
		expect(longTime((3 * 1440 + 4 * 60 + 30) * 60_000)).toBe('3 days 4 hours');
		expect(longTime(1440 * 60_000)).toBe('1 day');
	});
});

describe('a size on disk', () => {
	it('is in gigabytes once there is one', () => {
		expect(diskSize(720 * 1_048_576)).toBe('720 MB');
		expect(diskSize(38.5 * 1_073_741_824)).toBe('38.5 GB');
		expect(diskSize(250 * 1_073_741_824)).toBe('250 GB');
		expect(diskSize(10)).toBe('1 MB');
	});
});

describe("Home's numbers", () => {
	it('each say what they count, and the counts open their list', () => {
		const tiles = statTiles({ songs: 402, albums: 1, artists: 190, genres: 12, duration_ms: 26 * 3_600_000, bytes: 14 * 1_073_741_824 });
		expect(tiles.map((t) => `${t.value} ${t.label}`)).toEqual(['402 songs', '1 album', '190 artists', '12 genres', '1 day 2 hours of music', '14.0 GB on disk']);
		expect(tiles.filter((t) => t.href).map((t) => t.href)).toEqual(['/library?tab=songs', '/library?tab=albums', '/library?tab=artists', '/library?tab=genres']);
	});
});
