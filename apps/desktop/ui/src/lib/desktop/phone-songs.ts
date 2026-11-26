/**
 * Choosing which of a phone's songs come in (PhoneSongs.svelte, the core's `GET /api/phone/songs`), apart from the screen
 * so it is tested. The filters choose what is shown; what is shown and ticked is taken. A song unticked stays unticked
 * while the filters change.
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

export type Filters = {
	/** Words in the title, artist or album. */
	text: string;
	/** Only songs not on this computer yet. */
	newOnly: boolean;
	/** Only songs with a title, artist, album and cover. */
	completeOnly: boolean;
	likedOnly: boolean;
	/** Each empty: any. */
	formats: string[];
	qualities: Quality[];
	artists: string[];
	albums: string[];
	playlists: string[];
};

export const NO_FILTERS: Filters = { text: '', newOnly: true, completeOnly: false, likedOnly: false, formats: [], qualities: [], artists: [], albums: [], playlists: [] };

const fold = (t: string) => t.normalize('NFKD').replace(/\p{M}/gu, '').toLowerCase();

export function shows(s: Offered, f: Filters): boolean {
	if (f.newOnly && s.here) return false;
	if (f.completeOnly && !complete(s)) return false;
	if (f.likedOnly && !s.liked) return false;
	if (f.formats.length && !f.formats.includes(formatOf(s))) return false;
	if (f.qualities.length && !f.qualities.includes(qualityOf(s))) return false;
	if (f.artists.length && !f.artists.includes(artistOf(s))) return false;
	if (f.albums.length && !f.albums.includes(s.album?.trim() || '')) return false;
	if (f.playlists.length && !s.playlists.some((p) => f.playlists.includes(p))) return false;
	const words = fold(f.text).split(/\s+/).filter(Boolean);
	if (words.length) {
		const hay = fold(`${s.title} ${s.artist} ${s.album ?? ''} ${s.album_artist ?? ''}`);
		if (!words.every((w) => hay.includes(w))) return false;
	}
	return true;
}

/** A filter's choices, each with how many of the songs have it: the most first, then by name. */
export type Facet = { value: string; label: string; count: number };

function facet(values: string[], label = (v: string) => v): Facet[] {
	const counts = new Map<string, number>();
	for (const v of values) counts.set(v, (counts.get(v) ?? 0) + 1);
	return [...counts]
		.map(([value, count]) => ({ value, label: label(value), count }))
		.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
}

/** What each filter can choose from, among `songs`. */
export function facets(songs: Offered[]) {
	return {
		formats: facet(songs.map(formatOf)),
		qualities: QUALITIES.map((q) => ({ value: q.id, label: q.label, count: songs.filter((s) => qualityOf(s) === q.id).length })).filter((q) => q.count > 0),
		artists: facet(songs.map(artistOf)),
		albums: facet(songs.map((s) => s.album?.trim() || '').filter(Boolean)),
		playlists: facet(songs.flatMap((s) => s.playlists))
	};
}

/** The songs to take: those shown and not unticked. */
export const chosen = (songs: Offered[], f: Filters, unticked: ReadonlySet<string>): Offered[] => songs.filter((s) => shows(s, f) && !unticked.has(s.file));

export const bytesOf = (songs: Offered[]): number => songs.reduce((n, s) => n + s.bytes, 0);

/** "4.1 GB", "380 MB". */
export const size = (bytes: number): string => (bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.max(bytes ? 1 : 0, Math.round(bytes / 1e6))} MB`);
