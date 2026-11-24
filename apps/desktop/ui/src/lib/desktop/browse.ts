/** Library › Folders: the folders that hold the library's files, as an indented list (Browse.svelte). */
import type { PlaceRow } from '../core';

export type FolderRow = {
	/** The folder's whole path. */
	path: string;
	/** What is shown: its name, or (for a music folder itself) its whole path. */
	name: string;
	/** How far below its music folder it is: 0 for the music folder itself. */
	depth: number;
	/** Songs directly in it, and songs in it and every folder below it. */
	songs: number;
	below: number;
};

const SEP = /[\\/]+/;
/** A path with one kind of separator and none at its end, lower-cased, for comparing. */
const plain = (p: string) => p.replace(/[\\/]+/g, '/').replace(/\/$/, '').toLowerCase();

/**
 * The folders in path order, each under the music folder (`roots`) it is inside, with the folders between shown too
 * (a folder holding only folders has no songs of its own but leads to them). A folder outside every music folder (a
 * song Stash saved somewhere else) stands by itself at the top level.
 */
export function folderRows(rows: PlaceRow[], roots: string[]): FolderRow[] {
	const songsIn = new Map<string, { path: string; songs: number }>();
	for (const r of rows) songsIn.set(plain(r.folder), { path: r.folder, songs: r.songs });
	const rootKeys = roots.map((r) => ({ key: plain(r), path: r.replace(/[\\/]+$/, '') })).sort((a, b) => b.key.length - a.key.length);

	// Every folder to show: the ones with songs, and each folder between them and their music folder.
	const shown = new Map<string, { path: string; depth: number; root: string }>();
	for (const [key, { path }] of songsIn) {
		const root = rootKeys.find((r) => key === r.key || key.startsWith(r.key + '/'));
		if (!root) {
			shown.set(key, { path, depth: 0, root: key });
			continue;
		}
		const parts = path.split(SEP);
		const rootParts = root.path.split(SEP).length;
		for (let n = rootParts; n <= parts.length; n++) {
			const sub = parts.slice(0, n).join(path.includes('\\') ? '\\' : '/');
			const k = plain(sub);
			if (!shown.has(k)) shown.set(k, { path: n === rootParts ? root.path : sub, depth: n - rootParts, root: root.key });
		}
	}
	const keys = [...shown.keys()].sort();
	return keys.map((key) => {
		const { path, depth } = shown.get(key)!;
		let below = 0;
		for (const [k, v] of songsIn) if (k === key || k.startsWith(key + '/')) below += v.songs;
		return { path, name: depth === 0 ? path : (path.split(SEP).pop() ?? path), depth, songs: songsIn.get(key)?.songs ?? 0, below };
	});
}

/** A decade as it is said: "1990s"; the songs with no year, "No year". */
export const decadeName = (decade: number | null): string => (decade === null ? 'No year' : `${decade}s`);
