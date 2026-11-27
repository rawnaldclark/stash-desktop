// The first time Stash opens: bringing in the music on the listener's phone (BringIn.svelte). What the phone sent is
// the send its Send to computer made (`/api/phone/songs`) and, from a phone new enough, its whole library with it
// (`/api/phone/library`): likes, plays and every playlist, the ones synced from a streaming service marked as such.
// This works out what comes in from what is ticked, and the line that says so.
import type { Offer } from './phone-songs';

/** A playlist the phone sent: `from` names the streaming service it is synced from (none: the listener's own). */
export type SentList = {
	id: string;
	name: string;
	from: 'spotify' | 'youtube' | 'soundcloud' | null;
	/** What it is on the phone (none from an older phone). */
	kind?: 'playlist' | 'album' | 'mix' | 'radio' | 'stash_mix' | null;
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
	/** `by`: per service the songs are liked on (Stash's own hearts, or a service's liked list); none from an older phone. */
	likes: { songs: number; downloaded: number; by?: LikesOn[] };
	plays: number;
	playlists: SentList[];
	albums: { title: string; artist: string; art_url: string | null; songs: number }[];
};

/** Songs liked on one service (`all`: an older phone's likes, which say nowhere). */
export type LikesOn = { service: string; songs: number; downloaded: number };

/**
 * What is ticked. `likes`: the services whose likes come (`all` for an older phone's). `onlyHere`: only songs
 * downloaded on the phone come in, and Stash then stays offline.
 */
export type Picks = { downloads: boolean; likes: string[]; plays: boolean; lists: string[]; onlyHere: boolean };

export const LIKE_NAME: Record<string, string> = {
	stash: 'Liked in Stash',
	spotify: 'Spotify Liked Songs',
	youtube: 'YouTube Liked Music',
	soundcloud: 'SoundCloud likes',
	all: 'Liked songs'
};
const SERVICE: Record<string, string> = { stash: 'Stash', spotify: 'Spotify', youtube: 'YouTube Music', soundcloud: 'SoundCloud' };

/** The phone's likes, one card per service it can tell apart; an older phone's, all as one. */
export function likeParts(lib: PhoneLibrary | null): LikesOn[] {
	if (!lib || !lib.likes.songs) return [];
	const by = (lib.likes.by ?? []).filter((l) => l.songs > 0);
	return by.length ? by : [{ service: 'all', songs: lib.likes.songs, downloaded: lib.likes.downloaded }];
}

export const ownLists = (lib: PhoneLibrary | null) => (lib?.playlists ?? []).filter((p) => !p.from);
export const syncedLists = (lib: PhoneLibrary | null) => (lib?.playlists ?? []).filter((p) => !!p.from);

/** As the screen opens: everything on the phone is coming (owner: "a live transfer of everything i have synced"). */
export function firstPicks(offer: Offer, lib: PhoneLibrary | null): Picks {
	return {
		downloads: offer.songs.some((s) => !s.here),
		likes: likeParts(lib).map((l) => l.service),
		plays: !!lib && lib.plays > 0,
		lists: (lib?.playlists ?? []).map((p) => p.id),
		onlyHere: false
	};
}

/** Ticks `key` (downloads, plays, `likes:<service>`, or a playlist's id) on or off. */
export function flip(picks: Picks, key: string): Picks {
	if (key === 'downloads' || key === 'plays') return { ...picks, [key]: !picks[key] };
	if (key.startsWith('likes:')) {
		const on = key.slice(6);
		return { ...picks, likes: picks.likes.includes(on) ? picks.likes.filter((s) => s !== on) : [...picks.likes, on] };
	}
	const lists = picks.lists.includes(key) ? picks.lists.filter((id) => id !== key) : [...picks.lists, key];
	return { ...picks, lists };
}

/** Whether `key` (as [flip] takes it) is ticked. */
export function isOn(picks: Picks, key: string): boolean {
	if (key === 'downloads' || key === 'plays') return picks[key];
	if (key.startsWith('likes:')) return picks.likes.includes(key.slice(6));
	return picks.lists.includes(key);
}

/** Ticks every one of `keys` (as [flip] takes them), or leaves every one out: a drag across covers, Select all, Select none. */
export function setKeys(picks: Picks, keys: string[], on: boolean): Picks {
	const want = new Set(keys);
	const likes = keys.filter((k) => k.startsWith('likes:')).map((k) => k.slice(6));
	const lists = keys.filter((k) => k !== 'downloads' && k !== 'plays' && !k.startsWith('likes:'));
	const add = (have: string[], more: string[]) => [...have, ...more.filter((x) => !have.includes(x))];
	return {
		...picks,
		downloads: want.has('downloads') ? on : picks.downloads,
		plays: want.has('plays') ? on : picks.plays,
		likes: on ? add(picks.likes, likes) : picks.likes.filter((s) => !likes.includes(s)),
		lists: on ? add(picks.lists, lists) : picks.lists.filter((id) => !lists.includes(id))
	};
}

/**
 * The phone's files to take: every song this computer doesn't have with All your downloads ticked; without it, the
 * ones liked (with Liked songs) or in a playlist ticked, so those still play offline.
 */
export function filesToTake(offer: Offer, lib: PhoneLibrary | null, picks: Picks): string[] {
	const fresh = offer.songs.filter((s) => !s.here);
	if (picks.downloads) return fresh.map((s) => s.file);
	const names = new Set((lib?.playlists ?? []).filter((p) => picks.lists.includes(p.id)).map((p) => p.name));
	return fresh.filter((s) => (picks.likes.length > 0 && s.liked) || s.playlists.some((n) => names.has(n))).map((s) => s.file);
}

/** The body of `POST /api/phone/take` for what is ticked. A phone too old to send its library sends songs only. */
export function takeBody(offer: Offer, lib: PhoneLibrary | null, picks: Picks): Record<string, unknown> {
	const files = filesToTake(offer, lib, picks);
	const songs = picks.downloads ? { all: true } : files.length ? { files } : { files: [], none: true };
	if (!lib) return { sent_at_ms: offer.sent_at_ms, ...songs };
	return {
		sent_at_ms: offer.sent_at_ms,
		...songs,
		// The services whose likes come; an older phone's likes say nowhere, so all of them or none.
		library: { likes: lib.likes.by?.length ? picks.likes : picks.likes.length > 0, plays: picks.plays, playlists: picks.lists, only_here: picks.onlyHere }
	};
}

/** `likes`: what of the likes comes, said in a few words (null: none). `albums`: of the lists ticked, saved albums. */
export type Tally = { files: number; bytes: number; away: number; lists: number; albums: number; likes: string | null; nothing: boolean };

/**
 * How much comes in: songs as files (and their size), and at most how many more stream (a song in two lists ticked
 * counts in both, so it is "up to").
 */
export function tally(offer: Offer, lib: PhoneLibrary | null, picks: Picks): Tally {
	const chosen = new Set(filesToTake(offer, lib, picks));
	const taken = offer.songs.filter((s) => chosen.has(s.file));
	const parts = likeParts(lib);
	const liked = parts.filter((l) => picks.likes.includes(l.service));
	let away = 0;
	if (lib && !picks.onlyHere) {
		for (const l of liked) away += Math.max(0, l.songs - l.downloaded);
		for (const p of lib.playlists) if (picks.lists.includes(p.id)) away += Math.max(0, p.songs - p.downloaded);
	}
	const ticked = (lib?.playlists ?? []).filter((p) => picks.lists.includes(p.id));
	const albums = ticked.filter((p) => p.kind === 'album').length;
	const likes = !liked.length
		? null
		: liked.length === parts.length && parts.length > 1
			? 'all your likes'
			: liked.length === 1
				? LIKE_NAME[liked[0].service] ?? 'Liked songs'
				: `${joinAnd(liked.map((l) => SERVICE[l.service] ?? l.service))} likes`;
	const nothing = !taken.length && !(lib && (liked.length || picks.plays || picks.lists.length));
	return { files: taken.length, bytes: taken.reduce((sum, s) => sum + s.bytes, 0), away, lists: ticked.length - albums, albums, likes, nothing };
}

const joinAnd = (a: string[]) => (a.length < 2 ? a.join('') : `${a.slice(0, -1).join(', ')} and ${a[a.length - 1]}`);
const whole = new Intl.NumberFormat('en-US');
export const n = (x: number) => whole.format(x);
export const gb = (bytes: number) => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1e6))} MB`);

/** The bar's two lines: what comes in, and what of it plays offline or streams. */
export function lines(t: Tally, picks: Picks): { top: string; sub: string } {
	const parts: string[] = [];
	if (t.files) parts.push(t.files === 1 ? '1 downloaded song' : `${n(t.files)} downloaded songs`);
	if (t.likes) parts.push(t.likes);
	if (t.lists) parts.push(t.lists === 1 ? '1 playlist' : `${n(t.lists)} playlists`);
	if (t.albums) parts.push(t.albums === 1 ? '1 album' : `${n(t.albums)} albums`);
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
