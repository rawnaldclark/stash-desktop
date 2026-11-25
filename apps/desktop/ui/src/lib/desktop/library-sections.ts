/**
 * Library's sections. The ways to look through the whole library are the chips on the Library page (and Ctrl+1 to
 * Ctrl+6, in this order). The collections (your likes, what you played, your downloads, the songs no add-on found, your
 * YouTube Music library) are each a page of their own, reached from the sidebar's Your library list, as Liked songs is.
 */
export const BROWSE = [
	{ id: 'songs', label: 'Songs' },
	{ id: 'playlists', label: 'Playlists' },
	{ id: 'albums', label: 'Albums' },
	{ id: 'artists', label: 'Artists' },
	{ id: 'genres', label: 'Genres' },
	{ id: 'folders', label: 'Folders' }
] as const;
export type BrowseId = (typeof BROWSE)[number]['id'];

/** The collections, by tab, with the title their page has. */
export const COLLECTIONS = {
	likes: 'Liked songs',
	history: 'History',
	downloads: 'Downloads',
	notfound: 'Not found',
	youtube: 'YouTube Music'
} as const;
export type CollectionId = keyof typeof COLLECTIONS;

export const isBrowse = (tab: string | null | undefined): tab is BrowseId => BROWSE.some((b) => b.id === tab);
export const isCollection = (tab: string | null | undefined): tab is CollectionId => !!tab && Object.prototype.hasOwnProperty.call(COLLECTIONS, tab);

/** The section Ctrl+`key` opens ("1" is the first), or null when the key is not one of them. */
export function sectionForKey(key: string): BrowseId | null {
	if (!/^[1-9]$/.test(key)) return null;
	return BROWSE[Number(key) - 1]?.id ?? null;
}

/** The keys that open a section, as the window says them ("Ctrl+1"). */
export function keysFor(id: BrowseId): string {
	return `Ctrl+${BROWSE.findIndex((b) => b.id === id) + 1}`;
}
