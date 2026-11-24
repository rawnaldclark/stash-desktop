/**
 * The pairing QR, drawn as the web player draws its own (stash-player `src/lib/sync/qr.ts`): `qrcode-generator` (MIT, no
 * dependencies), level M, one SVG path of a rectangle per run of dark modules.
 */
import qrcode from 'qrcode-generator';

/** Modules of white around the code (the QR standard's quiet zone). */
export const QUIET = 4;

/** The code as an SVG path in module units (`size` × `size`, quiet zone included). */
export function qrPath(text: string): { size: number; path: string } {
	const q = qrcode(0, 'M');
	q.addData(text, 'Byte');
	q.make();
	const n = q.getModuleCount();
	let path = '';
	for (let r = 0; r < n; r++) {
		for (let c = 0; c < n; ) {
			if (!q.isDark(r, c)) {
				c++;
				continue;
			}
			let run = 1;
			while (c + run < n && q.isDark(r, c + run)) run++;
			path += `M${c + QUIET} ${r + QUIET}h${run}v1h-${run}z`;
			c += run;
		}
	}
	return { size: n + 2 * QUIET, path };
}
