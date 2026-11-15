import type { HistoryEntry, LibraryItem } from '../api';

const DAY = 24 * 3600_000;

/** How "Jump back in" picks: old favourites you haven't played lately, never what "Recently played" already shows. */
export const JUMP = {
	/** Plays this far back count. */
	windowMs: 28 * DAY,
	/** An album played this recently is "recent", not something to jump back into. */
	restMs: DAY,
	/** Plays an album needs inside the window to count as a favourite. */
	minPlays: 2,
	/** Fewer albums than this and the shelf stays hidden (no thin, repetitive shelf for new listeners). */
	minAlbums: 2,
	max: 8
} as const;

/**
 * Albums for "Jump back in": grouped by album + artist from the listening history, played at least `minPlays` times in the
 * last `windowMs`, not played in the last `restMs`, and sharing no song with "Recently played" (`recentKeys`). Most played
 * first, then the longest-unplayed. Each group lists its songs most played first, so a tap plays the favourites.
 */
export function jumpBackIn(history: readonly HistoryEntry[], recentKeys: ReadonlySet<string>, now = Date.now()): LibraryItem[][] {
	type G = { plays: number; last: number; songs: Map<string, { item: LibraryItem; plays: number }> };
	const groups = new Map<string, G>();
	for (const h of history) {
		const i = h.item;
		if (!i.album || now - h.playedAt > JUMP.windowMs) continue;
		const k = `${i.album.toLowerCase()}|${i.artist.toLowerCase()}`;
		let g = groups.get(k);
		if (!g) groups.set(k, (g = { plays: 0, last: 0, songs: new Map() }));
		g.plays++;
		g.last = Math.max(g.last, h.playedAt);
		const s = g.songs.get(i.key);
		if (s) s.plays++;
		else g.songs.set(i.key, { item: i, plays: 1 });
	}
	const picked = [...groups.values()]
		.filter((g) => g.plays >= JUMP.minPlays && now - g.last > JUMP.restMs && ![...g.songs.keys()].some((key) => recentKeys.has(key)))
		.sort((a, b) => b.plays - a.plays || a.last - b.last)
		.slice(0, JUMP.max)
		.map((g) => [...g.songs.values()].sort((a, b) => b.plays - a.plays).map((s) => s.item));
	return picked.length >= JUMP.minAlbums ? picked : [];
}
