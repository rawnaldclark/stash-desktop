/**
 * Album colour: a palette (--np-dom / --np-vib / --np-mut / --np-text / --acc-ink) from a cover.
 * Extraction needs pixel access, so it runs only when the cover loads with CORS (`crossOrigin="anonymous"`) or is same-origin;
 * otherwise the Stash-purple default stays. Pure maths lives in `paletteFromPixels` (unit-tested).
 */
export type Palette = {
	key: string;
	dom: string;
	vib: string;
	mut: string;
	/** --np-text on dark and on light surfaces. */
	textDark: string;
	textLight: string;
	/** Ink on the accent (white, or near-black when the accent is bright). */
	ink: string;
};

export const DEFAULT_PALETTE: Palette = {
	key: 'default',
	dom: '#2e1b6e',
	vib: '#8b5cf6',
	mut: '#3b3358',
	textDark: '#ae8df9',
	textLight: '#4f3390',
	ink: '#ffffff'
};

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

function hslToHex(h: number, s: number, l: number): string {
	const k = (n: number) => (n + h / 30) % 12;
	const a = s * Math.min(l, 1 - l);
	const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
	const hex = (x: number) => Math.round(x * 255).toString(16).padStart(2, '0');
	return `#${hex(f(0))}${hex(f(8))}${hex(f(4))}`;
}

function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
	r /= 255;
	g /= 255;
	b /= 255;
	const max = Math.max(r, g, b);
	const min = Math.min(r, g, b);
	const l = (max + min) / 2;
	const d = max - min;
	if (d === 0) return [0, 0, l];
	const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
	let h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
	h *= 60;
	return [h, s, l];
}

/** monochrome's rule: YIQ brightness of the vibrant colour > 128 gets dark ink. */
export function inkFor(hex: string): string {
	const n = parseInt(hex.slice(1), 16);
	const r = (n >> 16) & 255;
	const g = (n >> 8) & 255;
	const b = n & 255;
	return (r * 299 + g * 587 + b * 114) / 1000 > 128 ? '#1a0b2e' : '#ffffff';
}

/** Derives a palette from RGBA pixels (e.g. a 32x32 canvas). Returns null for a (near-)greyscale or empty image. */
export function paletteFromPixels(data: ArrayLike<number>, key = 'img'): Palette | null {
	const BUCKETS = 12;
	const score = new Float64Array(BUCKETS);
	const sx = new Float64Array(BUCKETS);
	const sy = new Float64Array(BUCKETS);
	const sat = new Float64Array(BUCKETS);
	const lum = new Float64Array(BUCKETS);
	const cnt = new Float64Array(BUCKETS);
	let seen = 0;
	for (let i = 0; i + 3 < data.length; i += 4) {
		if (data[i + 3] < 128) continue;
		const [h, s, l] = rgbToHsl(data[i], data[i + 1], data[i + 2]);
		seen++;
		if (s < 0.18 || l < 0.08 || l > 0.94) continue;
		const b = Math.floor(h / (360 / BUCKETS)) % BUCKETS;
		// colourful, mid-light pixels vote harder
		const w = s * s * (1 - Math.abs(l - 0.5));
		score[b] += w;
		sx[b] += Math.cos((h * Math.PI) / 180) * w;
		sy[b] += Math.sin((h * Math.PI) / 180) * w;
		sat[b] += s;
		lum[b] += l;
		cnt[b]++;
	}
	if (!seen) return null;
	let best = -1;
	for (let b = 0; b < BUCKETS; b++) if (best < 0 || score[b] > score[best]) best = b;
	if (best < 0 || cnt[best] < Math.max(6, seen * 0.02)) return null;
	const hue = ((Math.atan2(sy[best], sx[best]) * 180) / Math.PI + 360) % 360;
	const s = sat[best] / cnt[best];
	const vib = hslToHex(hue, clamp(s, 0.55, 0.9), clamp(lum[best] / cnt[best], 0.45, 0.62));
	return {
		key,
		dom: hslToHex(hue, clamp(s, 0.4, 0.8), 0.24),
		vib,
		mut: hslToHex(hue, 0.18, 0.27),
		textDark: hslToHex(hue, clamp(s + 0.1, 0.55, 0.95), 0.74),
		textLight: hslToHex(hue, 0.55, 0.3),
		ink: inkFor(vib)
	};
}

const cache = new Map<string, Promise<Palette | null>>();

/** Loads `url` with CORS and extracts a palette; resolves null if the image cannot be read (no CORS, error, tainted). */
export function loadPalette(url: string | undefined): Promise<Palette | null> {
	if (!url || typeof document === 'undefined') return Promise.resolve(null);
	let p = cache.get(url);
	if (!p) {
		p = new Promise<Palette | null>((resolve) => {
			const img = new Image();
			img.crossOrigin = 'anonymous';
			img.decoding = 'async';
			img.referrerPolicy = 'no-referrer';
			img.onload = () => {
				try {
					const c = document.createElement('canvas');
					c.width = c.height = 32;
					const ctx = c.getContext('2d', { willReadFrequently: true });
					if (!ctx) return resolve(null);
					ctx.drawImage(img, 0, 0, 32, 32);
					resolve(paletteFromPixels(ctx.getImageData(0, 0, 32, 32).data, url));
				} catch {
					resolve(null); // tainted canvas
				}
			};
			img.onerror = () => resolve(null);
			img.src = url;
		});
		cache.set(url, p);
		if (cache.size > 60) cache.delete(cache.keys().next().value as string);
	}
	return p;
}

/**
 * The palette as painted on a light surface. The extracted dark tones (dom at L 0.24, mut at L 0.27) are fine behind white text
 * on dark, but washed at 14 to 22 % over white they turn into a brown-grey haze. For light, dom and mut are lifted to mid
 * lightness with the cover's own hue (so the wash is a clean tint of it, as the prototype's lavender), and the accent and
 * ink stay. The default Stash purple is the prototype's own light palette and is left alone.
 */
const relLum = (hex: string) => {
	const n = parseInt(hex.slice(1), 16);
	const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
	return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
};

export function forScheme(p: Palette, light: boolean): Palette {
	if (!light || p.key === 'default') return p;
	const n = parseInt(p.vib.slice(1), 16);
	const [h, s] = rgbToHsl((n >> 16) & 255, (n >> 8) & 255, n & 255);
	const dom = hslToHex(h, clamp(s, 0.5, 0.85), 0.5);
	// The accent text sits on the wash (dom at up to 22 % over white): darken it until it holds 5:1 there (yellows need it).
	const mix = (c: number) => Math.round(c * 0.22 + 255 * 0.78);
	const d = parseInt(dom.slice(1), 16);
	const wash = `#${[(d >> 16) & 255, (d >> 8) & 255, d & 255].map((c) => mix(c).toString(16).padStart(2, '0')).join('')}`;
	let l = 0.3;
	let text = hslToHex(h, 0.55, l);
	while (l > 0.14 && (relLum(wash) + 0.05) / (relLum(text) + 0.05) < 5) text = hslToHex(h, 0.55, (l -= 0.02));
	return { ...p, key: `${p.key}#light`, dom, mut: hslToHex(h, 0.4, 0.58), textLight: text };
}

/** The inline style that sets the registered --np-* colours (they crossfade over 800 ms in tokens.css). */
export function paletteStyle(p: Palette, light: boolean): string {
	p = forScheme(p, light);
	return `--np-dom:${p.dom};--np-vib:${p.vib};--np-mut:${p.mut};--np-text:${light ? p.textLight : p.textDark};--acc-ink:${p.ink}`;
}

const contrast = (a: string, b: string) => {
	const [hi, lo] = [relLum(a), relLum(b)].sort((x, y) => y - x) as [number, number];
	return (hi + 0.05) / (lo + 0.05);
};
const mixHex = (a: string, b: string, t: number) => {
	const x = parseInt(a.slice(1), 16);
	const y = parseInt(b.slice(1), 16);
	const ch = (s: number) => Math.round(((x >> s) & 255) * (1 - t) + ((y >> s) & 255) * t);
	return `#${[16, 8, 0].map((s) => ch(s).toString(16).padStart(2, '0')).join('')}`;
};
/** The wordmark's plain ink (white on dark, the light wordmark's base on light) and the surfaces it sits on (the sidebar). */
export const WORDMARK = {
	dark: { plain: '#ffffff', on: ['#06060c', '#0d0d18', '#000000'] },
	light: { plain: '#1e0742', on: ['#f6f3ff', '#ffffff'] }
} as const;

/**
 * The wordmark's colour: its plain ink, or while a song plays a tint of it toward the song's mood colour
 * (--np-text for the scheme). Contrast-checked against every surface it sits on: the tint steps back toward the plain ink until it
 * holds 4.5:1 on all of them.
 */
export function wordmarkInk(p: Palette | null, light: boolean, amount = 0.6): string {
	const w = light ? WORDMARK.light : WORDMARK.dark;
	if (!p) return w.plain;
	const mood = light ? forScheme(p, true).textLight : p.textDark;
	for (let t = amount; t > 0.001; t -= 0.05) {
		const c = mixHex(w.plain, mood, t);
		if (w.on.every((bg) => contrast(c, bg) >= 4.5)) return c;
	}
	return w.plain;
}
