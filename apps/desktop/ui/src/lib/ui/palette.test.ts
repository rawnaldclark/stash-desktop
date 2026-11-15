import { describe, expect, it } from 'vitest';
import { DEFAULT_PALETTE, WORDMARK, forScheme, inkFor, paletteFromPixels, paletteStyle, wordmarkInk } from './palette';

const solid = (r: number, g: number, b: number, n = 400) => {
	const d = new Uint8ClampedArray(n * 4);
	for (let i = 0; i < n; i++) d.set([r, g, b, 255], i * 4);
	return d;
};
const hue = (hex: string) => {
	const n = parseInt(hex.slice(1), 16);
	const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((x) => x / 255);
	const max = Math.max(r, g, b);
	const d = max - Math.min(r, g, b);
	if (!d) return 0;
	const h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
	return h * 60;
};

describe('paletteFromPixels', () => {
	it('keeps the cover hue: orange stays orange, teal stays teal', () => {
		const orange = paletteFromPixels(solid(226, 115, 58))!;
		expect(Math.abs(hue(orange.vib) - 21)).toBeLessThan(10);
		const teal = paletteFromPixels(solid(20, 163, 148))!;
		expect(Math.abs(hue(teal.vib) - 173)).toBeLessThan(10);
	});
	it('returns null for greyscale or transparent images (the default purple stays)', () => {
		expect(paletteFromPixels(solid(128, 128, 128))).toBeNull();
		expect(paletteFromPixels(new Uint8ClampedArray(400))).toBeNull();
	});
	it('the dominant dark is darker than the vibrant, which is darker than the text on dark', () => {
		const p = paletteFromPixels(solid(139, 92, 246))!;
		const lum = (h: string) => {
			const n = parseInt(h.slice(1), 16);
			return ((n >> 16) & 255) * 0.299 + ((n >> 8) & 255) * 0.587 + (n & 255) * 0.114;
		};
		expect(lum(p.dom)).toBeLessThan(lum(p.vib));
		expect(lum(p.vib)).toBeLessThan(lum(p.textDark));
		expect(lum(p.textLight)).toBeLessThan(lum(p.vib));
	});
});

describe('inkFor (monochrome rule: YIQ > 128 gets dark ink)', () => {
	it('matches the prototype palettes', () => {
		expect(inkFor('#e2733a')).toBe('#1a0b2e');
		expect(inkFor('#8b5cf6')).toBe('#ffffff');
		expect(inkFor('#14a394')).toBe('#ffffff');
		expect(inkFor('#e5486b')).toBe('#ffffff');
	});
});

describe('paletteStyle', () => {
	it('sets the registered --np-* colours, picking the text colour by scheme', () => {
		expect(paletteStyle(DEFAULT_PALETTE, false)).toContain('--np-text:#ae8df9');
		expect(paletteStyle(DEFAULT_PALETTE, true)).toContain('--np-text:#4f3390');
	});
});

describe('forScheme (light mode lift)', () => {
	const rgb = (hex: string) => {
		const n = parseInt(hex.slice(1), 16);
		return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
	};
	const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
	const lum = (hex: string) => {
		const [r, g, b] = rgb(hex).map(lin);
		return 0.2126 * r + 0.7152 * g + 0.0722 * b;
	};
	const ratio = (a: number, b: number) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
	/** `fg` at alpha over `bg` hex. */
	const over = (fg: string, a: number, bg: string) => '#' + rgb(fg).map((c, i) => Math.round(c * a + rgb(bg)[i] * (1 - a)).toString(16).padStart(2, '0')).join('');
	const swatch = (hueDeg: number) => {
		const c = (n: number) => Math.round(255 * (0.5 - 0.35 * Math.max(-1, Math.min(((n + hueDeg / 30) % 12) - 3, Math.min(9 - ((n + hueDeg / 30) % 12), 1)))));
		return solid(c(0), c(8), c(4));
	};

	it('leaves the default palette and dark mode alone', () => {
		expect(forScheme(DEFAULT_PALETTE, true)).toBe(DEFAULT_PALETTE);
		const p = paletteFromPixels(solid(226, 115, 58))!;
		expect(forScheme(p, false)).toBe(p);
	});

	it('lifts dom and mut so the top wash over white is a clean pastel (not a dark haze), for every hue', () => {
		for (let h = 0; h < 360; h += 15) {
			const p = paletteFromPixels(swatch(h));
			if (!p) continue;
			const l = forScheme(p, true);
			expect(l.key).not.toBe(p.key);
			expect(lum(l.dom), `dom lifted at hue ${h}`).toBeGreaterThan(lum(p.dom) * 2);
			// the strongest wash layer is dom at 22 % over white: it must stay light (>= 0.62 relative luminance)
			expect(lum(over(l.dom, 0.22, '#ffffff')), `wash at hue ${h}`).toBeGreaterThan(0.62);
			expect(lum(over(l.mut, 0.14, '#ffffff')), `mut wash at hue ${h}`).toBeGreaterThan(0.62);
		}
	});

	it('keeps text readable on light: --np-text >= 4.5:1 on white and on the lifted wash, ink readable on the accent', () => {
		for (let h = 0; h < 360; h += 15) {
			const p = paletteFromPixels(swatch(h));
			if (!p) continue;
			const l = forScheme(p, true);
			const wash = over(l.dom, 0.22, '#ffffff');
			expect(ratio(lum(l.textLight), lum('#ffffff')), `text/white at ${h}`).toBeGreaterThanOrEqual(4.5);
			expect(ratio(lum(l.textLight), lum(wash)), `text/wash at ${h}`).toBeGreaterThanOrEqual(4.5);
			expect(ratio(lum(l.ink), lum(l.vib)), `ink/accent at ${h}`).toBeGreaterThanOrEqual(3);
		}
	});

	it('paletteStyle paints the lifted tones in light and the extracted ones in dark', () => {
		const p = paletteFromPixels(solid(226, 115, 58))!;
		expect(paletteStyle(p, false)).toContain(`--np-dom:${p.dom}`);
		expect(paletteStyle(p, true)).toContain(`--np-dom:${forScheme(p, true).dom}`);
		expect(paletteStyle(p, true)).not.toContain(`--np-dom:${p.dom}`);
	});
});

describe('the wordmark ink', () => {
	const lum = (hex: string) => {
		const n = parseInt(hex.slice(1), 16);
		const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
		return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
	};
	const ratio = (a: string, b: string) => {
		const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p) as [number, number];
		return (x + 0.05) / (y + 0.05);
	};
	const orange = paletteFromPixels(solid(226, 115, 58), 'orange')!;
	const teal = paletteFromPixels(solid(40, 160, 150), 'teal')!;

	it('is the plain ink when nothing plays: white on dark, the base ink on light', () => {
		expect(wordmarkInk(null, false)).toBe('#ffffff');
		expect(wordmarkInk(null, true)).toBe('#1e0742');
	});
	it('takes a tint of the mood colour while a song plays, readable (4.5:1) on every surface it sits on, both themes', () => {
		for (const p of [orange, teal, DEFAULT_PALETTE]) {
			const dark = wordmarkInk(p, false);
			const light = wordmarkInk(p, true);
			expect(dark).not.toBe('#ffffff');
			expect(light).not.toBe('#1e0742');
			for (const bg of WORDMARK.dark.on) expect(ratio(dark, bg)).toBeGreaterThanOrEqual(4.5);
			for (const bg of WORDMARK.light.on) expect(ratio(light, bg)).toBeGreaterThanOrEqual(4.5);
		}
		// it follows the song: two moods, two inks
		expect(wordmarkInk(orange, false)).not.toBe(wordmarkInk(teal, false));
	});
	it('falls back toward the plain ink when the mood colour would be too dim', () => {
		const dim = { ...DEFAULT_PALETTE, key: 'dim', textDark: '#15151f', textLight: '#f0f0ff' };
		const d = wordmarkInk(dim, false);
		const l = wordmarkInk(dim, true);
		for (const bg of WORDMARK.dark.on) expect(ratio(d, bg)).toBeGreaterThanOrEqual(4.5);
		for (const bg of WORDMARK.light.on) expect(ratio(l, bg)).toBeGreaterThanOrEqual(4.5);
		expect(lum(d)).toBeGreaterThan(0.18); // stepped back toward white, not the dim mood
	});
});
