import { describe, expect, it } from 'vitest';
import { waveMask } from './wave';

const svgOf = (mask: string) => decodeURIComponent(mask.slice('url("data:image/svg+xml,'.length, -2));

describe("a song's waveform as a mask", () => {
	it('draws one bar a slice, mirrored about the middle, on a box as wide as the slices', () => {
		const svg = svgOf(waveMask([255, 0, 128]));
		expect(svg).toContain('viewBox="0 0 3 100"');
		expect(svg).toContain('preserveAspectRatio="none"');
		const rects = [...svg.matchAll(/<rect x="([\d.]+)" y="([\d.]+)" width="0.7" height="([\d.]+)"\/>/g)].map((m) => m.slice(1).map(Number));
		expect(rects).toEqual([
			[0.15, 0, 100],
			[1.15, 47, 6],
			[2.15, 24.9, 50.2]
		]);
	});

	it('is nothing to draw with no slices', () => {
		expect(waveMask([])).toBe('none');
	});
});
