/**
 * Songs as a session (and the share service) carries them: built from library items, and read back. The formats are the Android
 * app's (`ShareLinks.kt`) and the web player's (stash-player `src/lib/share/links.ts`), trimmed to what Sessions uses here.
 */
import { descriptorKey, type LibraryItem } from '../identity';
import { cleanCover, cut, isYouTubeId, type WireTrack } from './rules';

/** The source id under which a song's YouTube video id (`yt`) is a ref: the YouTube source names itself `youtube`. */
export const YT_SOURCE = 'youtube';

/** The web player's caps for a song's words (sources/contract.ts LIMITS). */
const LIMITS = { title: 300, artist: 300, album: 300, maxDurationMs: 24 * 60 * 60 * 1000 };
/** Control characters, and the bidi controls that could reorder a line. */
const UNSAFE = /[\p{Cc}\u202A-\u202E\u2066-\u2069\u200E\u200F]/gu;

/** One line of someone else's text: unsafe characters out, spaces folded, cut to `max` (the web's `cleanText`). */
export function cleanText(s: string, max: number): string {
	return cut(s.replace(UNSAFE, ' ').replace(/\s+/g, ' ').trim(), max);
}

/**
 * The cover a song may carry: the first of its covers that sits on a known cover host. The core's own cover addresses
 * (`/api/art/remote?u=<CDN URL>`) are unwrapped to the CDN address; a cover from a file on this computer has none.
 */
export function shareableArt(item: Pick<LibraryItem, 'artwork'>): string | undefined {
	for (const a of item.artwork ?? []) {
		let url = a.url;
		try {
			const u = new URL(url);
			const inner = u.searchParams.get('u');
			if (inner && (u.pathname === '/art' || u.pathname.endsWith('/art/remote'))) url = inner;
		} catch {
			continue;
		}
		const c = cleanCover(url);
		if (c) return c;
	}
	return undefined;
}

/** A library item as a shared song: the app's fields and limits. `withArt` adds the cover. */
export function wireTrackOf(item: LibraryItem, withArt = false): WireTrack {
	const t: WireTrack = { t: cut(item.title.trim(), 500) || 'Unknown title', a: cut(item.artist.trim(), 500) || 'Unknown artist' };
	if (item.album?.trim()) t.al = cut(item.album.trim(), 500);
	if (item.durationMs && item.durationMs > 0) t.d = Math.round(item.durationMs);
	if (item.isrc && item.isrc.length <= 20) t.isrc = item.isrc;
	const yt = item.refs?.[YT_SOURCE];
	if (isYouTubeId(yt)) t.yt = yt;
	if (withArt) {
		const art = shareableArt(item);
		if (art) t.art = art;
	}
	return t;
}

/** An ISRC in its 12-character form (hyphens and case forgiven), else undefined. */
export function isrcCode(v: string | undefined): string | undefined {
	const c = (v ?? '').replace(/-/g, '').toUpperCase();
	return /^[A-Z0-9]{12}$/.test(c) ? c : undefined;
}

/** A shared song as a library item: no stream, no source ids except its YouTube video id. */
export function itemFromWire(t: WireTrack, now = Date.now()): LibraryItem {
	const title = cleanText(t.t, LIMITS.title) || 'Unknown title';
	const artist = cleanText(t.a, LIMITS.artist) || 'Unknown artist';
	const item: LibraryItem = { title, artist, key: '', refs: {}, addedAt: now };
	const album = t.al ? cleanText(t.al, LIMITS.album) : '';
	if (album) item.album = album;
	if (t.d && t.d <= LIMITS.maxDurationMs) item.durationMs = t.d;
	const isrc = isrcCode(t.isrc);
	if (isrc) item.isrc = isrc;
	if (isYouTubeId(t.yt)) item.refs[YT_SOURCE] = t.yt;
	const art = cleanCover(t.art);
	if (art) item.artwork = [{ url: art }];
	item.key = descriptorKey(item);
	return item;
}
