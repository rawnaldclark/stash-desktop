// The words for a station growing the queue (a radio, or autoplay): the line Up next draws above its songs still to
// come, and what it says while it finds its first ones.
import type { Station } from '../core';

/** The line above a station's songs: `lead`, then (when Last.fm chose them) a "Last.fm" link, then Stop. `tip` says why
 * the library chose them, when it did and the core said why. `stopped` is said once Stop was pressed. */
export type StationLine = { lead: string; lastfm: boolean; tip: string | null; stopped: string };

/** What the station is like: the seed song's title, or the artist's name. */
const like = (s: Station) => s.seed?.title || s.seed?.artist || s.label;

export function stationLine(s: Station | null | undefined): StationLine | null {
	if (!s) return null;
	const tip = s.via === 'library' ? s.why : null;
	if (s.kind === 'radio') {
		const stopped = 'Radio stopped';
		return s.via === 'lastfm'
			? { lead: `Similar to ${like(s)} · from`, lastfm: true, tip, stopped }
			: { lead: `Songs like ${like(s)} from your library`, lastfm: false, tip, stopped };
	}
	const stopped = 'Autoplay stopped';
	return s.via === 'lastfm'
		? { lead: `Autoplay · like ${like(s)} · from`, lastfm: true, tip, stopped }
		: { lead: 'Autoplay · from your library', lastfm: false, tip, stopped };
}

/** What Up next says when a station has nothing after the song playing and finds no more: what it looked for, and
 * why, when the core said ("No songs like Teardrop were found (Last.fm could not be reached)."). */
export const nothingWords = (s: Station) => `No songs like ${like(s)} were found${s.why ? ` (${s.why})` : ''}.`;

/** What Up next says while a station finds its first songs and nothing follows yet. */
export const findingWords = (s: Station) => (s.seed?.title ? `Finding songs like ${s.seed.title}…` : `Finding songs for ${s.label}…`);
