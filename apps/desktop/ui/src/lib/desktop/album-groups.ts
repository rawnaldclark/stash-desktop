/** Library › Albums grouped: by name, artist, decade, genre or when they came. Apart from the screen so it is tested. */
import type { AlbumCard } from '../core';
import { fold } from '../identity';

export type GroupBy = 'name' | 'artist' | 'year' | 'genre' | 'added';
export const GROUPINGS: { id: GroupBy; label: string }[] = [
	{ id: 'name', label: 'Name' },
	{ id: 'artist', label: 'Artist' },
	{ id: 'year', label: 'Year' },
	{ id: 'genre', label: 'Genre' },
	{ id: 'added', label: 'Recently added' }
];
export const isGroupBy = (v: unknown): v is GroupBy => GROUPINGS.some((g) => g.id === v);

export type Group = { key: string; label: string; albums: AlbumCard[] };

const DAY = 86_400_000;
const byTitle = (a: AlbumCard, b: AlbumCard) => fold(a.title).localeCompare(fold(b.title));
/** Oldest first, an album with no year last. */
const byYear = (a: AlbumCard, b: AlbumCard) => (a.year ?? 99_999) - (b.year ?? 99_999) || byTitle(a, b);

/** The letter an album files under: A to Z, else "#". */
export function letterOf(title: string): string {
	const c = fold(title).charAt(0).toUpperCase();
	return c >= 'A' && c <= 'Z' ? c : '#';
}

/** How long ago an album came, as a heading: the last week, month and three months, then the year. */
export function addedBucket(ms: number, now: number): { key: string; label: string; order: number } {
	const age = now - ms;
	if (ms <= 0) return { key: 'unknown', label: 'Unknown', order: Number.MAX_SAFE_INTEGER };
	if (age < 7 * DAY) return { key: 'week', label: 'This week', order: 0 };
	if (age < 30 * DAY) return { key: 'month', label: 'This month', order: 1 };
	if (age < 91 * DAY) return { key: 'months', label: 'The last three months', order: 2 };
	const year = new Date(ms).getFullYear();
	return { key: String(year), label: String(year), order: 10_000 - year };
}

/**
 * The albums in groups. Name: by first letter, A to Z then "#". Artist: by album artist A to Z, each artist's albums
 * oldest first. Year: by decade, newest first, then "No year". Genre: A to Z, then "No genre". Recently added: newest
 * first, in the buckets of [addedBucket].
 */
export function groupAlbums(cards: AlbumCard[], by: GroupBy, now: number): Group[] {
	// Groups go in order of a rank, then of their text: a rank of 1 puts "#", "No year" and "No genre" last.
	const groups = new Map<string, Group & { order: [number, string] }>();
	const put = (key: string, label: string, order: [number, string], card: AlbumCard) => {
		let g = groups.get(key);
		if (!g) groups.set(key, (g = { key, label, order, albums: [] }));
		g.albums.push(card);
	};
	for (const c of cards) {
		if (by === 'name') {
			const l = letterOf(c.title);
			put(l, l, l === '#' ? [1, ''] : [0, l], c);
		} else if (by === 'artist') {
			put(`a:${fold(c.artist)}`, c.artist, [0, fold(c.artist)], c);
		} else if (by === 'year') {
			if (c.year == null) put('none', 'No year', [Number.MAX_SAFE_INTEGER, ''], c);
			else {
				const d = Math.floor(c.year / 10) * 10;
				put(String(d), `${d}s`, [10_000 - d, ''], c);
			}
		} else if (by === 'genre') {
			const g = c.genre?.trim();
			if (g) put(`g:${fold(g)}`, g, [0, fold(g)], c);
			else put('none', 'No genre', [1, ''], c);
		} else {
			const b = addedBucket(c.added_ms, now);
			put(b.key, b.label, [b.order, ''], c);
		}
	}
	const out = [...groups.values()].sort((a, b) => a.order[0] - b.order[0] || a.order[1].localeCompare(b.order[1]));
	for (const g of out) {
		if (by === 'artist') g.albums.sort(byYear);
		else if (by === 'year') g.albums.sort((a, b) => (b.year ?? 0) - (a.year ?? 0) || byTitle(a, b));
		else if (by === 'added') g.albums.sort((a, b) => b.added_ms - a.added_ms || byTitle(a, b));
		else if (by === 'genre') g.albums.sort((a, b) => fold(a.artist).localeCompare(fold(b.artist)) || byYear(a, b));
		// By name, the library's own order (its sort key) stands.
	}
	return out.map(({ key, label, albums }) => ({ key, label, albums }));
}

/** The groups cut to the first `upTo` albums, so a long grid is drawn a part at a time. */
export function upToCards(groups: Group[], upTo: number): Group[] {
	const out: Group[] = [];
	let left = upTo;
	for (const g of groups) {
		if (left <= 0) break;
		out.push(g.albums.length <= left ? g : { ...g, albums: g.albums.slice(0, left) });
		left -= g.albums.length;
	}
	return out;
}
