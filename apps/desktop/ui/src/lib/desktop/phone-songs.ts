/**
 * Choosing which of a phone's songs come in (PhoneSongs.svelte, the core's `GET /api/phone/songs`), apart from the screen
 * so it is tested. Nothing is chosen to begin with: songs are ticked one by one, or all those a filter shows at once.
 * Filters only choose what is shown; they never take a song out of the choice. Songs on this computer already are not
 * chosen (the phone lets go of its copies of them once the rest are in).
 */

/** One song a phone sent, as the core offers it. */
export type Offered = {
	file: string;
	title: string;
	artist: string;
	album: string | null;
	album_artist: string | null;
	/** The phone's word for the file's format: "flac", "opus", "aac", "alac", "mp3"… (older phones send none). */
	format: string | null;
	bits: number | null;
	sample_rate: number | null;
	duration_ms: number | null;
	art_url: string | null;
	liked: boolean;
	playlists: string[];
	/** Its size on the phone. */
	bytes: number;
	/** The library has it as a file already. */
	here: boolean;
};

export type Offer = { phone: string; sent_at_ms: number; songs: Offered[] };

export type Quality = 'hires' | 'cd' | 'lossy';

export const QUALITIES: { id: Quality; label: string; hint: string }[] = [
	{ id: 'hires', label: 'Hi-res', hint: 'Lossless, past CD quality (more than 16-bit or 48 kHz)' },
	{ id: 'cd', label: 'CD quality', hint: 'Lossless, 16-bit up to 48 kHz' },
	{ id: 'lossy', label: 'Lossy', hint: 'AAC, Opus, MP3 and the like' }
];

const NAMES: Record<string, string> = {
	flac: 'FLAC',
	alac: 'ALAC',
	aac: 'AAC',
	m4a: 'AAC',
	mp4: 'AAC',
	opus: 'Opus',
	webm: 'Opus',
	mp3: 'MP3',
	vorbis: 'Vorbis',
	ogg: 'Vorbis',
	wav: 'WAV',
	aiff: 'AIFF',
	wavpack: 'WavPack',
	wv: 'WavPack',
	ape: 'APE'
};
const LOSSLESS = new Set(['FLAC', 'ALAC', 'WAV', 'AIFF', 'WavPack', 'APE']);

/** The song's format by name ("FLAC", "AAC"…): what the phone says, else its file's extension. */
export function formatOf(s: Pick<Offered, 'format' | 'file'>): string {
	const word = (s.format ?? '').trim().toLowerCase() || (s.file.includes('.') ? s.file.split('.').pop()!.toLowerCase() : '');
	return NAMES[word] ?? (word ? word.toUpperCase() : 'Other');
}

/** Hi-res, CD quality or lossy, as Statistics counts them: lossless past 16-bit or 48 kHz is hi-res. */
export function qualityOf(s: Pick<Offered, 'format' | 'file' | 'bits' | 'sample_rate'>): Quality {
	if (!LOSSLESS.has(formatOf(s))) return 'lossy';
	return (s.bits ?? 0) > 16 || (s.sample_rate ?? 0) > 48000 ? 'hires' : 'cd';
}

/** "FLAC · 24/96", "AAC", "FLAC · 16/44.1": the format, and a lossless file's depth and rate when the phone says. */
export function detailOf(s: Pick<Offered, 'format' | 'file' | 'bits' | 'sample_rate'>): string {
	const name = formatOf(s);
	if (!LOSSLESS.has(name) || !s.bits || !s.sample_rate) return name;
	return `${name} · ${s.bits}/${Number((s.sample_rate / 1000).toFixed(1))}`;
}

/** Tagged well enough to keep: a title, an artist, an album, and a cover. */
export const complete = (s: Pick<Offered, 'title' | 'artist' | 'album' | 'art_url'>): boolean =>
	!!s.title.trim() && !!s.artist.trim() && !!s.album?.trim() && !!s.art_url;

/** Who a song is by, for choosing by artist: its album's artist when the phone says, else its own. */
export const artistOf = (s: Pick<Offered, 'artist' | 'album_artist'>): string => (s.album_artist?.trim() || s.artist.trim() || 'Unknown artist');

/** The filters, in the order they sit in the row. */
export type FacetId = 'playlist' | 'artist' | 'album' | 'format' | 'quality' | 'tags';

export const FACETS: { id: FacetId; label: string; many: string; find: string; least: number }[] = [
	{ id: 'playlist', label: 'Playlist', many: 'playlists', find: 'Find a playlist', least: 1 },
	{ id: 'artist', label: 'Artist', many: 'artists', find: 'Find an artist', least: 2 },
	{ id: 'album', label: 'Album', many: 'albums', find: 'Find an album', least: 1 },
	{ id: 'format', label: 'Format', many: 'formats', find: 'Find a format', least: 2 },
	{ id: 'quality', label: 'Quality', many: 'qualities', find: 'Find a quality', least: 2 },
	{ id: 'tags', label: 'Tags', many: 'tags', find: 'Find', least: 2 }
];

/** The phone's Liked songs, chosen from with its playlists. */
export const LIKED = 'Liked songs';
export const COMPLETE_TAGS = 'Complete tags';
export const MISSING_TAGS = 'Missing tags';

/** What a song is under a filter: its playlists (Liked songs too when liked), artist, "Album · Artist", format, quality or tags. */
export function keysOf(s: Offered, facet: FacetId): string[] {
	switch (facet) {
		case 'playlist':
			return s.liked ? [LIKED, ...s.playlists] : s.playlists;
		case 'artist':
			return [artistOf(s)];
		case 'album':
			return s.album?.trim() ? [`${s.album.trim()} · ${artistOf(s)}`] : [];
		case 'format':
			return [formatOf(s)];
		case 'quality':
			return [QUALITIES.find((q) => q.id === qualityOf(s))!.label];
		case 'tags':
			return [complete(s) ? COMPLETE_TAGS : MISSING_TAGS];
	}
}

export type Filters = {
	/** Words in the title, artist or album. */
	text: string;
	/** Songs on this computer already are shown too (greyed: they are not chosen). */
	showHere: boolean;
	/** For each filter, what is ticked in it: a song shows when it is any of them. None ticked is any. */
	picked: Partial<Record<FacetId, string[]>>;
};

export const noFilters = (): Filters => ({ text: '', showHere: false, picked: {} });

const fold = (t: string) => t.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();

export function shows(s: Offered, f: Filters): boolean {
	if (s.here && !f.showHere) return false;
	for (const { id } of FACETS) {
		const want = f.picked[id];
		if (!want?.length) continue;
		const has = keysOf(s, id);
		if (!want.some((k) => has.includes(k))) return false;
	}
	const words = fold(f.text).split(/\s+/).filter(Boolean);
	if (!words.length) return true;
	const hay = fold(`${s.title} ${s.artist} ${s.album ?? ''} ${s.album_artist ?? ''}`);
	return words.every((w) => hay.includes(w));
}

/** Whether any filter or words narrow the songs shown. */
export const narrowed = (f: Filters): boolean => !!f.text.trim() || FACETS.some(({ id }) => !!f.picked[id]?.length);

/** One of a filter's choices, with how many of the songs are it. */
export type Facet = { value: string; count: number };

/** What a filter can choose from among `songs`: the most first, then by name. */
export function facet(songs: Offered[], id: FacetId): Facet[] {
	const counts = new Map<string, number>();
	for (const s of songs) for (const k of keysOf(s, id)) counts.set(k, (counts.get(k) ?? 0) + 1);
	return [...counts].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count || a.value.localeCompare(b.value));
}

/** The songs to take: those chosen that are still offered and not on this computer already. */
export const toTake = (songs: Offered[], chosen: ReadonlySet<string>): Offered[] => songs.filter((s) => !s.here && chosen.has(s.file));

export const bytesOf = (songs: Offered[]): number => songs.reduce((n, s) => n + s.bytes, 0);

/** "4.1 GB", "380 MB". */
export const size = (bytes: number): string => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(bytes ? 1 : 0, Math.round(bytes / 1e6))} MB`);
