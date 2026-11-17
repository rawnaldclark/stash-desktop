import type { LibraryItem } from '../identity';
import type { Artwork } from './contract';

/**
 * One section of the search page: the library ("Your library"), or one add-on. `tags` holds a small label for some of its songs,
 * by the section's own id for the song (what an add-on says a track's best quality is).
 */
export type SearchSection = { sourceId: string; name: string; items: LibraryItem[]; tags?: Record<string, string> };

/** An artist or an album a search found, with the page it opens. */
export type SearchArtist = { sourceId: string; id: string; name: string; artwork?: Artwork[] };
export type SearchAlbum = { sourceId: string; id: string; title: string; artist: string; year?: number; artwork?: Artwork[] };
/** A playlist a search found (YouTube Music's), with who made it. It opens like an album. */
export type SearchPlaylist = { sourceId: string; id: string; title: string; author: string; artwork?: Artwork[] };
/** What the "Top result" card is: the first song, or the artist or album the listener typed the name of. */
export type TopPick = { kind: 'track' } | { kind: 'artist'; artist: SearchArtist } | { kind: 'album'; album: SearchAlbum };

export type SearchResult = {
	/** Merged songs, round-robin across sections in their order, duplicates folded (a folded duplicate carries every section's ref). */
	top: LibraryItem[];
	/** The library's artists and albums the text names. */
	artists: SearchArtist[];
	albums: SearchAlbum[];
	/** Playlists the text names, from YouTube Music. */
	playlists: SearchPlaylist[];
	topPick: TopPick;
	bySource: SearchSection[];
	/** Sources that did not answer (the UI shows its empty state when all of them failed). */
	failed: { sourceId: string; name: string; message: string }[];
	/** The names of the add-ons still being asked (only in the first, library-only answer). */
	pending: string[];
};

export const TOP_LIMIT = 10;

export const emptySearch = (failed: SearchResult['failed'] = []): SearchResult => ({ top: [], artists: [], albums: [], playlists: [], topPick: { kind: 'track' }, bySource: [], failed, pending: [] });

/**
 * "Top results", as the web player merges them: round-robin across the sections in order (every section's first song, then every
 * second…), a song two sections both have folded into one that carries both refs, the first `TOP_LIMIT` kept.
 */
export function mergeTop(sections: SearchSection[], limit = TOP_LIMIT): LibraryItem[] {
	const out: LibraryItem[] = [];
	const index = new Map<string, LibraryItem>();
	const depth = Math.max(0, ...sections.map((s) => s.items.length));
	for (let rank = 0; rank < depth; rank++) {
		for (const sec of sections) {
			const it = sec.items[rank];
			if (!it) continue;
			const hit = index.get(it.key);
			if (hit) {
				for (const [k, v] of Object.entries(it.refs)) hit.refs[k] ??= v;
				continue;
			}
			if (out.length >= limit) continue;
			const copy: LibraryItem = { ...it, refs: { ...it.refs } };
			index.set(it.key, copy);
			out.push(copy);
		}
	}
	return out;
}
