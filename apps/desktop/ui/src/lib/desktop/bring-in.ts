// The first time Stash opens: bringing in the music on the listener's phone (BringIn.svelte). What the phone sent is
// the send its Send to computer made (`/api/phone/songs`) and, from a phone new enough, its whole library with it
// (`/api/phone/library`): likes, plays and every playlist, the ones synced from a streaming service marked as such.
// This works out what comes in from what is ticked, and the line that says so.
import type { Offer } from './phone-songs';

/** A playlist the phone sent: `from` names the streaming service it is synced from (none: the listener's own). */
export type SentList = {
	id: string;
	name: string;
	from: 'spotify' | 'youtube' | null;
	songs: number;
	/** Its songs the phone has downloaded (or this computer has as files already). */
	downloaded: number;
	/** Up to four covers of its first songs (addresses on the web). */
	covers: string[];
};

/** The phone's library, summed up for the screen (`GET /api/phone/library`). */
export type PhoneLibrary = {
	sent_at_ms: number;
	phone: string;
	downloads: { songs: number; bytes: number; here: number };
	likes: { songs: number; downloaded: number };
	plays: number;
	playlists: SentList[];
	albums: { title: string; artist: string; art_url: string | null; songs: number }[];
};

/** What is ticked. `onlyHere`: only songs downloaded on the phone come in, and Stash then stays offline. */
export type Picks = { downloads: boolean; likes: boolean; plays: boolean; lists: string[]; onlyHere: boolean };

export const ownLists = (lib: PhoneLibrary | null) => (lib?.playlists ?? []).filter((p) => !p.from);
export const syncedLists = (lib: PhoneLibrary | null) => (lib?.playlists ?? []).filter((p) => !!p.from);

/** As the screen opens: everything of the listener's own is coming (downloads, likes, plays, own playlists); a synced playlist is added by hand. */
export function firstPicks(offer: Offer, lib: PhoneLibrary | null): Picks {
	return {
		downloads: offer.songs.some((s) => !s.here),
		likes: !!lib && lib.likes.songs > 0,
		plays: !!lib && lib.plays > 0,
		lists: ownLists(lib).map((p) => p.id),
		onlyHere: false
	};
}

/** Ticks `key` (downloads, likes, plays, or a playlist's id) on or off. */
export function flip(picks: Picks, key: string): Picks {
	if (key === 'downloads' || key === 'likes' || key === 'plays') return { ...picks, [key]: !picks[key] };
	const lists = picks.lists.includes(key) ? picks.lists.filter((id) => id !== key) : [...picks.lists, key];
	return { ...picks, lists };
}

/**
 * The phone's files to take: every song this computer doesn't have with All your downloads ticked; without it, the
 * ones liked (with Liked songs) or in a playlist ticked, so those still play offline.
 */
export function filesToTake(offer: Offer, lib: PhoneLibrary | null, picks: Picks): string[] {
	const fresh = offer.songs.filter((s) => !s.here);
	if (picks.downloads) return fresh.map((s) => s.file);
	const names = new Set((lib?.playlists ?? []).filter((p) => picks.lists.includes(p.id)).map((p) => p.name));
	return fresh.filter((s) => (picks.likes && s.liked) || s.playlists.some((n) => names.has(n))).map((s) => s.file);
}

/** The body of `POST /api/phone/take` for what is ticked. A phone too old to send its library sends songs only. */
export function takeBody(offer: Offer, lib: PhoneLibrary | null, picks: Picks): Record<string, unknown> {
	const files = filesToTake(offer, lib, picks);
	const songs = picks.downloads ? { all: true } : files.length ? { files } : { files: [], none: true };
	if (!lib) return { sent_at_ms: offer.sent_at_ms, ...songs };
	return {
		sent_at_ms: offer.sent_at_ms,
		...songs,
		library: { likes: picks.likes, plays: picks.plays, playlists: picks.lists, only_here: picks.onlyHere }
	};
}

export type Tally = { files: number; bytes: number; away: number; lists: number; nothing: boolean };

/**
 * How much comes in: songs as files (and their size), and at most how many more stream (a song in two lists ticked
 * counts in both, so it is "up to").
 */
export function tally(offer: Offer, lib: PhoneLibrary | null, picks: Picks): Tally {
	const chosen = new Set(filesToTake(offer, lib, picks));
	const taken = offer.songs.filter((s) => chosen.has(s.file));
	let away = 0;
	if (lib && !picks.onlyHere) {
		if (picks.likes) away += Math.max(0, lib.likes.songs - lib.likes.downloaded);
		for (const p of lib.playlists) if (picks.lists.includes(p.id)) away += Math.max(0, p.songs - p.downloaded);
	}
	const nothing = !taken.length && !(lib && (picks.likes || picks.plays || picks.lists.length));
	return { files: taken.length, bytes: taken.reduce((sum, s) => sum + s.bytes, 0), away, lists: picks.lists.length, nothing };
}

const whole = new Intl.NumberFormat('en-US');
export const n = (x: number) => whole.format(x);
export const gb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);

/** The bar's two lines: what comes in, and what of it plays offline or streams. */
export function lines(t: Tally, picks: Picks): { top: string; sub: string } {
	const parts: string[] = [];
	if (t.files) parts.push(t.files === 1 ? '1 downloaded song' : `${n(t.files)} downloaded songs`);
	if (picks.likes) parts.push('Liked songs');
	if (t.lists) parts.push(t.lists === 1 ? '1 playlist' : `${n(t.lists)} playlists`);
	if (picks.plays) parts.push('your plays');
	if (t.nothing || !parts.length) return { top: 'Nothing picked yet', sub: 'Pick something to bring it in.' };
	const offline = t.files ? `${n(t.files)} play offline (${gb(t.bytes)})` : '';
	let sub: string;
	if (picks.onlyHere) sub = t.files ? `${offline}. Nothing streams.` : 'Only songs you downloaded come in. Nothing streams.';
	else if (t.away) sub = `${offline ? `${offline} · ` : ''}up to ${n(t.away)} more stream when you’re online`;
	else sub = offline || 'Nothing to copy.';
	return { top: parts.join(' · '), sub };
}

/** A playlist's line under its cover. */
export function listLine(p: SentList, onlyHere: boolean): string {
	const songs = (x: number) => (x === 1 ? '1 song' : `${n(x)} songs`);
	if (!p.downloaded) return onlyHere ? 'None downloaded' : `${songs(p.songs)} · none downloaded`;
	if (onlyHere || p.downloaded >= p.songs) return `${songs(p.downloaded)} · all downloaded`;
	return `${songs(p.songs)} · ${n(p.downloaded)} downloaded`;
}
