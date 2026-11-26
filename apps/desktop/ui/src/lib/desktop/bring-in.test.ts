import { describe, expect, it } from 'vitest';
import { filesToTake, firstPicks, flip, lines, listLine, ownLists, syncedLists, takeBody, tally, type PhoneLibrary, type SentList } from './bring-in';
import type { Offer, Offered } from './phone-songs';

const song = (o: Partial<Offered>): Offered => ({
	file: '1.flac',
	title: 'Song',
	artist: 'Band',
	album: 'Album',
	album_artist: null,
	format: 'flac',
	bits: 16,
	sample_rate: 44100,
	duration_ms: 200_000,
	art_url: null,
	liked: false,
	playlists: [],
	bytes: 40e6,
	here: false,
	...o
});
const list = (o: Partial<SentList>): SentList => ({ id: 'app-1', name: 'In Via', from: null, songs: 108, downloaded: 19, covers: [], ...o });

const offer: Offer = {
	phone: 'Pixel 6 Pro',
	sent_at_ms: 8,
	songs: [
		song({ file: '1.flac', liked: true, playlists: ['In Via'] }),
		song({ file: '2.flac', playlists: ['Daily Mix 2'] }),
		song({ file: '3.flac' }),
		song({ file: '4.flac', here: true, liked: true })
	]
};
const lib: PhoneLibrary = {
	sent_at_ms: 8,
	phone: 'Pixel 6 Pro',
	downloads: { songs: 3, bytes: 120e6, here: 1 },
	likes: { songs: 3194, downloaded: 238 },
	plays: 5236,
	playlists: [list({}), list({ id: 'sp-1', name: 'Daily Mix 2', from: 'spotify', songs: 50, downloaded: 29 })],
	albums: []
};

describe('what is ticked as the screen opens', () => {
	it('is everything of the listener’s own: downloads, likes, plays and their own playlists, not the synced ones', () => {
		expect(firstPicks(offer, lib)).toEqual({ downloads: true, likes: true, plays: true, lists: ['app-1'], onlyHere: false });
		expect(ownLists(lib).map((p) => p.id)).toEqual(['app-1']);
		expect(syncedLists(lib).map((p) => p.id)).toEqual(['sp-1']);
	});
	it('from a phone too old to send its library, is its downloads', () => {
		expect(firstPicks(offer, null)).toEqual({ downloads: true, likes: false, plays: false, lists: [], onlyHere: false });
	});
	it('has no downloads to tick when the computer has every song already', () => {
		expect(firstPicks({ ...offer, songs: [song({ here: true })] }, lib).downloads).toBe(false);
	});
});

describe('ticking', () => {
	it('flips one part or one playlist', () => {
		const p = firstPicks(offer, lib);
		expect(flip(p, 'likes').likes).toBe(false);
		expect(flip(p, 'sp-1').lists).toEqual(['app-1', 'sp-1']);
		expect(flip(p, 'app-1').lists).toEqual([]);
	});
});

describe('the files taken', () => {
	it('are every song the computer lacks with All your downloads', () => {
		expect(filesToTake(offer, lib, firstPicks(offer, lib))).toEqual(['1.flac', '2.flac', '3.flac']);
	});
	it('without it, are the liked ones and those in a playlist ticked', () => {
		const p = { ...firstPicks(offer, lib), downloads: false };
		expect(filesToTake(offer, lib, p)).toEqual(['1.flac']);
		expect(filesToTake(offer, lib, flip(p, 'sp-1'))).toEqual(['1.flac', '2.flac']);
		expect(filesToTake(offer, lib, { ...p, likes: false, lists: [] })).toEqual([]);
	});
});

describe('the take', () => {
	it('takes the whole send and the library parts ticked', () => {
		expect(takeBody(offer, lib, firstPicks(offer, lib))).toEqual({
			sent_at_ms: 8,
			all: true,
			library: { likes: true, plays: true, playlists: ['app-1'], only_here: false }
		});
	});
	it('names its files without All your downloads, and takes none when nothing of the send is wanted', () => {
		const p = { ...firstPicks(offer, lib), downloads: false };
		expect(takeBody(offer, lib, p)).toMatchObject({ files: ['1.flac'] });
		expect(takeBody(offer, lib, { ...p, likes: false, lists: [] })).toMatchObject({ files: [], none: true, library: { plays: true } });
	});
	it('says only_here for songs downloaded only, and carries no library for an old phone', () => {
		expect(takeBody(offer, lib, { ...firstPicks(offer, lib), onlyHere: true })).toMatchObject({ library: { only_here: true } });
		expect(takeBody(offer, null, firstPicks(offer, null))).toEqual({ sent_at_ms: 8, all: true });
	});
});

describe('the bar', () => {
	it('says what comes in, what plays offline and up to how many stream', () => {
		const p = firstPicks(offer, lib);
		const t = tally(offer, lib, p);
		expect(t).toMatchObject({ files: 3, away: 2956 + 89, lists: 1, nothing: false });
		expect(lines(t, p)).toEqual({
			top: '3 downloaded songs · Liked songs · 1 playlist · your plays',
			sub: '3 play offline (120 MB) · up to 3,045 more stream when you’re online'
		});
	});
	it('says nothing streams with only songs downloaded', () => {
		const p = { ...firstPicks(offer, lib), onlyHere: true };
		expect(lines(tally(offer, lib, p), p).sub).toBe('3 play offline (120 MB). Nothing streams.');
	});
	it('says so when nothing is ticked', () => {
		const p = { downloads: false, likes: false, plays: false, lists: [], onlyHere: false };
		const t = tally(offer, lib, p);
		expect(t.nothing).toBe(true);
		expect(lines(t, p)).toEqual({ top: 'Nothing picked yet', sub: 'Pick something to bring it in.' });
	});
});

describe('a playlist’s line', () => {
	it('says how many songs and how many are downloaded', () => {
		expect(listLine(list({}), false)).toBe('108 songs · 19 downloaded');
		expect(listLine(list({ songs: 2, downloaded: 2 }), false)).toBe('2 songs · all downloaded');
		expect(listLine(list({ downloaded: 0 }), false)).toBe('108 songs · none downloaded');
		expect(listLine(list({}), true)).toBe('19 songs · all downloaded');
	});
});
