/** What Fill in missing tags says (FillTags.svelte), apart from the screen so it is tested. */
import type { FillNow } from '../core';

const files = (n: number) => `${n.toLocaleString()} ${n === 1 ? 'file' : 'files'}`;

/** Where the lookup is, in one line. */
export function fillLine(now: FillNow): string {
	if (now.running) return `Looking up ${files(now.total)}: ${now.done.toLocaleString()} done, ${now.found.length.toLocaleString()} found so far.`;
	if (now.found.length) {
		const missed = now.missed ? ` The catalogue had nothing new for ${now.missed.toLocaleString()}.` : '';
		return `Found tags for ${files(now.found.length)}. Untick any that look wrong, then write the rest.${missed}`;
	}
	if (!now.blanks) return 'Every file on this computer has its album, year, track number and genre.';
	if (now.total) return `The catalogue had nothing new for the ${files(now.blanks)} still missing tags. Edit tags fills those by hand.`;
	return `${files(now.blanks)} on this computer ${now.blanks === 1 ? 'is' : 'are'} missing an album, year, track number or genre.`;
}

/** What would be written for one song: "In Rainbows · 2007 · track 3 · Alternative". */
export function offerLine(set: Record<string, string>): string {
	const parts: string[] = [];
	if (set.album) parts.push(set.album);
	if (set.year) parts.push(set.year);
	if (set.track_no) parts.push(`track ${set.track_no}${set.disc_no ? `, disc ${set.disc_no}` : ''}`);
	if (set.genre) parts.push(set.genre);
	return parts.join(' · ');
}
