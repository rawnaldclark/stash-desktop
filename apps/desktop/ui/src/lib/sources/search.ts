import type { LibraryItem } from '../identity';

/**
 * One section of the search page: the library ("Your library"), or one add-on. `tags` holds a small label for some of its songs,
 * by the section's own id for the song (what an add-on says a track's best quality is).
 */
export type SearchSection = { sourceId: string; name: string; items: LibraryItem[]; tags?: Record<string, string> };

export type SearchResult = {
	/** Merged songs, round-robin across sections in their order, duplicates folded (a folded duplicate carries every section's ref). */
	top: LibraryItem[];
	bySource: SearchSection[];
	/** Sources that did not answer (the UI shows its empty state when all of them failed). */
	failed: { sourceId: string; name: string; message: string }[];
	/** The names of the add-ons still being asked (only in the first, library-only answer). */
	pending: string[];
};

export const TOP_LIMIT = 10;

export const emptySearch = (failed: SearchResult['failed'] = []): SearchResult => ({ top: [], bySource: [], failed, pending: [] });

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
