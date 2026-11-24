/** The seek bar in a song's own shape (ProgressBar `wave`): its waveform from the core, drawn as a mask. */

/** How tall a slice is drawn at the least, out of 100, so a silent stretch still shows where the bar runs. */
const FLOOR = 6;

/**
 * The waveform as an SVG mask image: one bar a slice, mirrored about the middle, on a 100-high box stretched to the
 * track. Peaks are 0 to 255.
 */
export function waveMask(peaks: number[]): string {
	const n = peaks.length;
	if (!n) return 'none';
	let bars = '';
	for (let i = 0; i < n; i++) {
		const h = Math.max(FLOOR, Math.min(100, (peaks[i] / 255) * 100));
		bars += `<rect x="${i + 0.15}" y="${((100 - h) / 2).toFixed(1)}" width="0.7" height="${h.toFixed(1)}"/>`;
	}
	const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} 100" preserveAspectRatio="none">${bars}</svg>`;
	return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}
