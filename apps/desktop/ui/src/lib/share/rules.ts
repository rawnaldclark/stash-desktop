/**
 * The share service's data rules, in one dependency-free file that both the player Worker's `/api/share` proxy and the browser use.
 * Mirrors stashfm.app's share Worker (MP3APK `infra/share-worker/src/validate.js`) and the app's `ShareLinks.kt` / `SharedTrack.kt`:
 * the same field names (`t a al d isrc sp yt art`), the same limits, the same cover hosts. Keep them in step with those files.
 */

/** Where every link the player makes points: stashfm.app, so the Android app's App Links keep opening them in the app. */
export const SHARE_ORIGIN = 'https://stashfm.app';

/** Album-art CDNs a shared song or mix may point at (share Worker COVER_HOSTS). Anything else could log the recipient's IP. */
export const COVER_HOSTS = [
	'i.scdn.co',
	'mosaic.scdn.co',
	'i.ytimg.com',
	'lh3.googleusercontent.com',
	'yt3.googleusercontent.com',
	'yt3.ggpht.com',
	'lastfm.freetls.fastly.net',
	'lastfm-img.freetls.fastly.net',
	'static.qobuz.com',
	'c.saavncdn.com',
	'cdn-images.dzcdn.net',
	'e-cdns-images.dzcdn.net'
] as const;

export const MAX_MIX_TRACKS = 2000;
export const MAX_MIX_COVERS = 4;
export const MAX_NAME = 100;
export const MAX_SHARED_BY = 40;
const TEXT_MAX = 500;
const ISRC_MAX = 20;

/** A song as share links, short links and shared mixes carry it. Only `t` and `a` are required. */
export type WireTrack = { t: string; a: string; al?: string; d?: number; isrc?: string; sp?: string; yt?: string; art?: string };

/** A shared mix document (share Worker spec section 3). The server sets `id`, `version` and `updatedAt`. */
export type MixDoc = {
	v: number;
	id?: string;
	version?: number;
	updatedAt?: number;
	name: string;
	sharedBy?: string;
	covers?: string[];
	tracks: WireTrack[];
};

const SHARE_ID = /^[A-Za-z0-9]{8}$/;
/** Mix ids and short song link ids: 8 letters or digits. */
export const isShareId = (v: unknown): v is string => typeof v === 'string' && SHARE_ID.test(v);
/** A mix's edit key: base64url of 32 random bytes. It never appears in a link. */
export const isEditKey = (v: unknown): v is string => typeof v === 'string' && /^[A-Za-z0-9_-]{43}$/.test(v);
export const isYouTubeId = (v: unknown): v is string => typeof v === 'string' && /^[A-Za-z0-9_-]{11}$/.test(v);
export const isSpotifyId = (v: unknown): v is string => typeof v === 'string' && /^[A-Za-z0-9]{22}$/.test(v);

// Control characters and bidi overrides never travel in a shared field.
const BAD = /[\p{Cc}‪-‮⁦-⁩]/gu;

/** `take` that never ends on half an emoji (the app's `cut`). */
export function cut(s: string, n: number): string {
	const t = s.slice(0, n);
	return t.length === n && /[\uD800-\uDBFF]$/.test(t) ? t.slice(0, -1) : t;
}

function req(v: unknown, max: number): string | null {
	if (typeof v !== 'string') return null;
	const s = v.replace(BAD, '').trim();
	return s && s.length <= max ? s : null;
}
function opt(v: unknown, max: number): string | undefined {
	if (typeof v !== 'string') return undefined;
	const s = v.replace(BAD, '').trim();
	return s && s.length <= max ? s : undefined;
}

/**
 * A cover URL in its parsed form when it is https on a COVER_HOSTS host (exactly, or a subdomain), else null. Credentials and ports are
 * refused, and the parsed `href` is what is kept, so every parser downstream reads the same host.
 */
export function cleanCover(url: unknown): string | null {
	if (typeof url !== 'string' || url.length > 1000) return null;
	let u: URL;
	try {
		u = new URL(url);
	} catch {
		return null;
	}
	if (u.protocol !== 'https:' || u.username || u.password || u.port) return null;
	const h = u.hostname.toLowerCase();
	return COVER_HOSTS.some((c) => h === c || h.endsWith(`.${c}`)) ? u.href : null;
}

/**
 * One song, cleaned: null when the title or artist is missing or over 500 characters (the server refuses those); optional fields that
 * are malformed are dropped, as the server's own cleaner drops them. `art` only with `withArt`, and only from a cover host.
 */
export function cleanWireTrack(v: unknown, withArt = false): WireTrack | null {
	if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
	const o = v as Record<string, unknown>;
	const t = req(o.t, TEXT_MAX);
	const a = req(o.a, TEXT_MAX);
	if (!t || !a) return null;
	const out: WireTrack = { t, a };
	const al = opt(o.al, TEXT_MAX);
	if (al) out.al = al;
	if (typeof o.d === 'number' && Number.isSafeInteger(o.d) && o.d > 0) out.d = o.d;
	const isrc = opt(o.isrc, ISRC_MAX);
	if (isrc) out.isrc = isrc;
	if (isSpotifyId(o.sp)) out.sp = o.sp;
	if (isYouTubeId(o.yt)) out.yt = o.yt;
	if (withArt) {
		const art = cleanCover(o.art);
		if (art) out.art = art;
	}
	return out;
}

/**
 * A mix document, cleaned, or null when it is not one. `fromServer` also keeps the server's `id`, `version` and `updatedAt` (and accepts
 * any `v`, so the caller can refuse a newer format itself); for an upload `v` must be 1. Any song that is not a song rejects the document.
 */
export function cleanMixDoc(v: unknown, fromServer = false): MixDoc | null {
	if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
	const o = v as Record<string, unknown>;
	if (fromServer ? !(Number.isSafeInteger(o.v) && (o.v as number) >= 1) : o.v !== 1) return null;
	const name = req(o.name, MAX_NAME);
	if (!name) return null;
	if (!Array.isArray(o.tracks) || o.tracks.length < 1 || o.tracks.length > MAX_MIX_TRACKS) return null;
	const tracks: WireTrack[] = [];
	for (const t of o.tracks) {
		const c = cleanWireTrack(t);
		if (!c) return null;
		tracks.push(c);
	}
	const doc: MixDoc = { v: o.v as number, name, tracks };
	if (o.sharedBy !== undefined && o.sharedBy !== null) {
		if (typeof o.sharedBy !== 'string' || o.sharedBy.length > MAX_SHARED_BY) return null;
		const by = opt(o.sharedBy, MAX_SHARED_BY);
		if (by) doc.sharedBy = by;
	}
	if (o.covers !== undefined) {
		if (!Array.isArray(o.covers) || o.covers.length > MAX_MIX_COVERS) return null;
		const covers = o.covers.map(cleanCover).filter((c): c is string => !!c);
		if (covers.length) doc.covers = covers;
	}
	if (fromServer) {
		if (!isShareId(o.id) || !Number.isSafeInteger(o.version) || (o.version as number) < 1) return null;
		doc.id = o.id;
		doc.version = o.version as number;
		if (Number.isSafeInteger(o.updatedAt) && (o.updatedAt as number) > 0) doc.updatedAt = o.updatedAt as number;
	}
	return doc;
}
