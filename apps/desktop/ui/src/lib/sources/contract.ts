/**
 * The types the UI shares with whatever finds and plays music, from the Stash web player's source contract. Only the types: on the
 * desktop the core talks to the add-ons, so the validators and the source runtime are not here.
 */

export type Artwork = { url: string; width?: number; height?: number };

export type Track = {
	id: string;
	title: string;
	artist: string;
	album?: string;
	/** Opaque id the same source's `album` route understands, when it has one. */
	albumId?: string;
	/** Opaque id the same source's `artist` route understands, when it has one. */
	artistId?: string;
	durationMs?: number;
	isrc?: string;
	explicit?: boolean;
	artwork?: Artwork[];
};

export type Album = { id: string; title: string; artist: string; year?: number; artwork?: Artwork[]; tracks: Track[] };
export type ArtistAlbum = { id: string; title: string; year?: number; artwork?: Artwork[]; trackCount?: number };
/** `withOthers`: songs credited to the artist together with others ("A & B", "B feat. A"). */
export type Artist = { id: string; name: string; artwork?: Artwork[]; topTracks: Track[]; albums: ArtistAlbum[]; withOthers?: Track[] };

export type Descriptor = { title: string; artist: string; album?: string; durationMs?: number; isrc?: string };

export type LyricWord = { startMs: number; endMs?: number; text: string };
export type LyricLine = { startMs: number; endMs?: number; text: string; words?: LyricWord[] };
export type Lyrics = { synced?: LyricLine[]; plain?: string };
