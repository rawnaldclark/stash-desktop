import type { Artwork, Descriptor, Track } from './sources/contract';

/**
 * The ref key of a song of the desktop library: `refs.lib` is its row id there. Every other ref key is an add-on's id, and its value
 * that add-on's id for the track.
 */
export const LIB_REF = 'lib';
/** The "source" of the library's own album and artist pages (`/album/library/<id>`). */
export const LIBRARY_SOURCE = 'library';

/** A song as the library knows it: what it is (Descriptor), where to find it (refs), when it was added. Never a stream URL. */
export type LibraryItem = Descriptor & {
	/** Stable identity of the recording, from descriptorKey(). */
	key: string;
	/** sourceId -> that source's track id. */
	refs: Record<string, string>;
	addedAt: number;
	artwork?: Artwork[];
	/** sourceId -> that source's album id for this song (lets the UI link the album name). */
	albumRefs?: Record<string, string>;
	/** sourceId -> that source's artist id for this song. */
	artistRefs?: Record<string, string>;
	/** False for a song that does not play with no connection (it plays from an add-on): offline, it is drawn dimmed. */
	here?: boolean;
};

/**
 * A bracketed credit that starts with "feat." / "ft." / "featuring" is the only
 * decoration folded away: the same recording is listed with and without it.
 * Everything else in brackets is part of the identity: "Dance (Without You)",
 * "Stay (Alive)", "(Live)", "(Remastered 2011)" and "(Acoustic)" are different
 * recordings or titles and keep different keys. Markers are matched as whole
 * words at the start of the bracket, never inside a word.
 */
const FEAT_BRACKET = /\s*[(\[]\s*(?:featuring|feat|ft)(?![\p{L}\p{N}])\.?[^)\]]*[)\]]/giu;

export const fold = (s: string) =>
	s
		.normalize('NFKD')
		.replace(/\p{M}/gu, '')
		.toLowerCase()
		.replace(FEAT_BRACKET, '')
		.replace(/[^\p{L}\p{N}]+/gu, ' ')
		.trim();

/** Normalised "title|artist" (ISRC wins when present). Used for de-duplication, likes and ref caches. */
export function descriptorKey(d: Pick<Descriptor, 'title' | 'artist' | 'isrc'>): string {
	if (d.isrc) return `isrc:${d.isrc.toUpperCase()}`;
	return `${fold(d.title)}|${fold(d.artist)}`;
}

/** A song's words alone: folded "title|artist", whatever its ISRC (`descriptorKey` without the ISRC). */
export const textKey = (d: Pick<Descriptor, 'title' | 'artist'>) => `${fold(d.title)}|${fold(d.artist)}`;

export type SongLike = Pick<Descriptor, 'title' | 'artist' | 'isrc'> & { key?: string };
/** The song's stored key, else its `descriptorKey`. */
export const keyOf = (d: SongLike) => d.key || descriptorKey(d);
/** The song's ISRC, upper-cased, from its field or its stored `isrc:` key; '' or undefined when it has none. */
export const isrcOf = (d: SongLike) => d.isrc?.toUpperCase() || (keyOf(d).startsWith('isrc:') ? keyOf(d).slice(5) : undefined);

/**
 * One song under two keys: an ISRC key and the words' key are the same song when the ISRCs don't disagree, that is when one of the two
 * has none (an app song with an ISRC, and the same song liked here from YouTube, which gives none). Two different ISRCs stay two
 * recordings. Likes, an import's merge and History's de-duplication look songs up this way; the stored key is still `descriptorKey`.
 */
export function sameSong(a: SongLike, b: SongLike): boolean {
	if (keyOf(a) === keyOf(b)) return true;
	if (textKey(a) !== textKey(b)) return false;
	return !isrcOf(a) || !isrcOf(b);
}

/**
 * Whether a list row is the song playing (`b` the player's current item): the same library row when both are songs of the library
 * (one song can be in the library as two files), else the same song.
 */
export const sameCopy = (a: Pick<LibraryItem, 'key' | 'refs'>, b: Pick<LibraryItem, 'key' | 'refs'>) =>
	a.refs?.[LIB_REF] !== undefined && b.refs?.[LIB_REF] !== undefined ? a.refs[LIB_REF] === b.refs[LIB_REF] : a.key === b.key;

export type SongIndex<T> = {
	/** The value of the song in the index that is `sameSong` as `d` (its own key first), or undefined. */
	find(d: SongLike): T | undefined;
	has(d: SongLike): boolean;
	add(d: SongLike, v: T): void;
};

/** A lookup by `sameSong` that stays O(1) for a big library (a 14k-song Liked list checked for every row on screen). */
export function songIndex<T>(entries: Iterable<readonly [SongLike, T]> = []): SongIndex<T> {
	const byKey = new Map<string, T>();
	const byText = new Map<string, { isrc: string | undefined; v: T }[]>();
	const idx: SongIndex<T> = {
		find(d) {
			const hit = byKey.get(keyOf(d));
			if (hit !== undefined) return hit;
			const isrc = isrcOf(d);
			return byText.get(textKey(d))?.find((e) => !e.isrc || !isrc)?.v;
		},
		has: (d) => idx.find(d) !== undefined,
		add(d, v) {
			const k = keyOf(d);
			if (!byKey.has(k)) byKey.set(k, v);
			const t = textKey(d);
			const list = byText.get(t);
			if (list) list.push({ isrc: isrcOf(d), v });
			else byText.set(t, [{ isrc: isrcOf(d), v }]);
		}
	};
	for (const [d, v] of entries) idx.add(d, v);
	return idx;
}

export function trackToDescriptor(t: Track): Descriptor {
	const d: Descriptor = { title: t.title, artist: t.artist };
	if (t.album) d.album = t.album;
	if (t.durationMs !== undefined) d.durationMs = t.durationMs;
	if (t.isrc) d.isrc = t.isrc;
	return d;
}

/**
 * Turns a search hit into a library item that already knows its source's id for it. `from` is the source the track
 * (and so its albumId / artistId) came from; it defaults to the first source in `refs`.
 */
export function toLibraryItem(t: Track, refs: Record<string, string>, now = Date.now(), from: string | undefined = Object.keys(refs)[0]): LibraryItem {
	const d = trackToDescriptor(t);
	const item: LibraryItem = { ...d, key: descriptorKey(d), refs: { ...refs }, addedAt: now };
	if (t.artwork?.length) item.artwork = t.artwork;
	if (from && t.albumId) item.albumRefs = { [from]: t.albumId };
	if (from && t.artistId) item.artistRefs = { [from]: t.artistId };
	return item;
}

export type BrowseLink = { sourceId: string; id: string };

/** Where this song's album page is (the first source that gave an album id), or null: then the name is plain text. */
export function albumLink(item: Pick<LibraryItem, 'albumRefs'>): BrowseLink | null {
	const e = Object.entries(item.albumRefs ?? {})[0];
	return e ? { sourceId: e[0], id: e[1] } : null;
}

export function artistLink(item: Pick<LibraryItem, 'artistRefs'>): BrowseLink | null {
	const e = Object.entries(item.artistRefs ?? {})[0];
	return e ? { sourceId: e[0], id: e[1] } : null;
}

export const albumHref = (l: BrowseLink) => `/album/${encodeURIComponent(l.sourceId)}/${encodeURIComponent(l.id)}`;
export const artistHref = (l: BrowseLink) => `/artist/${encodeURIComponent(l.sourceId)}/${encodeURIComponent(l.id)}`;
