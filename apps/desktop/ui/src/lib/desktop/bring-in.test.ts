import { describe, expect, it } from 'vitest';
import { broughtLine, filesToTake, firstPicks, flip, isOn, likeParts, setKeys, lines, listLine, ownLists, syncedLists, takeBody, tally, type PhoneLibrary, type SentList } from './bring-in';
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
/** An older phone's library: its likes say nowhere. */
const lib: PhoneLibrary = {
	sent_at_ms: 8,
	phone: 'Pixel 6 Pro',
	downloads: { songs: 3, bytes: 120e6, here: 1 },
	likes: { songs: 3194, downloaded: 238 },
	plays: 5236,
	playlists: [list({}), list({ id: 'sp-1', name: 'Daily Mix 2', from: 'spotify', songs: 50, downloaded: 29 })],
	albums: []
};
/** Today's phone: likes per service, and every kind of list. */
const all: PhoneLibrary = {
	...lib,
	likes: {
		songs: 2947,
		downloaded: 743,
		by: [
			{ service: 'stash', songs: 296, downloaded: 238 },
			{ service: 'spotify', songs: 1354, downloaded: 247 },
			{ service: 'youtube', songs: 1561, downloaded: 595 }
		]
	},
	playlists: [
		list({ kind: 'playlist' }),
		list({ id: 'stash-mix-1', name: 'Daily Discover', kind: 'stash_mix', songs: 40, downloaded: 0 }),
		list({ id: 'spotify-album-1', name: 'Bleach', from: 'spotify', kind: 'album', songs: 13, downloaded: 13 }),
		list({ id: 'sp-1', name: 'Daily Mix 2', from: 'spotify', kind: 'mix', songs: 50, downloaded: 29 }),
		list({ id: 'sp-2', name: 'Pink Floyd Radio', from: 'spotify', kind: 'radio', songs: 50, downloaded: 3 })
	]
};

describe('what is ticked as the screen opens', () => {
	it('is everything on the phone: downloads, likes, plays and every playlist, synced ones too', () => {
		expect(firstPicks(offer, lib)).toEqual({ downloads: true, likes: ['all'], plays: true, lists: ['app-1', 'sp-1'], onlyHere: false });
		expect(ownLists(lib).map((p) => p.id)).toEqual(['app-1']);
		expect(syncedLists(lib).map((p) => p.id)).toEqual(['sp-1']);
	});
	it('ticks the likes of every service the phone tells apart', () => {
		expect(firstPicks(offer, all).likes).toEqual(['stash', 'spotify', 'youtube']);
		expect(likeParts(all).map((l) => l.service)).toEqual(['stash', 'spotify', 'youtube']);
		expect(likeParts(lib)).toEqual([{ service: 'all', songs: 3194, downloaded: 238 }]);
		expect(ownLists(all).map((p) => p.id)).toEqual(['app-1', 'stash-mix-1']);
	});
	it('from a phone too old to send its library, is its downloads', () => {
		expect(firstPicks(offer, null)).toEqual({ downloads: true, likes: [], plays: false, lists: [], onlyHere: false });
	});
	it('has no downloads to tick when the computer has every song already', () => {
		expect(firstPicks({ ...offer, songs: [song({ here: true })] }, lib).downloads).toBe(false);
	});	it('brought in before, is as it was left: what was left out stays out, what is new since comes', () => {
		const choice = { likes_left_out: ['youtube'], all_likes_left_out: false, plays: false, playlists_left_out: ['sp-1', 'app-1'], only_here: true };
		expect(firstPicks(offer, { ...all, choice })).toEqual({
			downloads: true,
			likes: ['stash', 'spotify'],
			plays: false,
			lists: ['stash-mix-1', 'spotify-album-1', 'sp-2'],
			onlyHere: true
		});
		expect(firstPicks(offer, { ...all, choice: { ...choice, all_likes_left_out: true, plays: true } })).toMatchObject({ likes: [], plays: true });
		expect(firstPicks(offer, { ...lib, choice: { ...choice, likes_left_out: [] } }).likes).toEqual(['all']);
		// All your downloads as it was left; ticked for a choice kept before it was kept.
		expect(firstPicks(offer, { ...all, choice: { ...choice, downloads: false } }).downloads).toBe(false);
		expect(firstPicks(offer, { ...all, choice }).downloads).toBe(true);
		expect(takeBody(offer, all, { ...firstPicks(offer, all), downloads: false })).toMatchObject({ library: { downloads: false } });
		expect(takeBody({ ...offer, songs: [song({ here: true })] }, all, firstPicks(offer, all))).toMatchObject({ library: { downloads: true } });
	});
});

describe('ticking', () => {
	it('flips one part, one service’s likes, or one playlist', () => {
		const p = firstPicks(offer, all);
		expect(flip(p, 'plays').plays).toBe(false);
		expect(flip(p, 'likes:youtube').likes).toEqual(['stash', 'spotify']);
		expect(flip(flip(p, 'likes:youtube'), 'likes:youtube').likes).toEqual(['stash', 'spotify', 'youtube']);
		expect(flip(p, 'sp-2').lists).toEqual(['app-1', 'stash-mix-1', 'spotify-album-1', 'sp-1']);
	});
});

describe('many at once (a drag across covers, Select all, Select none)', () => {
	it('leaves every one out, of any kind', () => {
		const p = setKeys(firstPicks(offer, all), ['downloads', 'likes:spotify', 'sp-1', 'sp-2'], false);
		expect(p).toMatchObject({ downloads: false, plays: true, likes: ['stash', 'youtube'], lists: ['app-1', 'stash-mix-1', 'spotify-album-1'] });
		expect(['downloads', 'likes:spotify', 'sp-2', 'app-1'].map((k) => isOn(p, k))).toEqual([false, false, false, true]);
	});
	it('ticks every one, keeping the order and never twice', () => {
		const none = setKeys(firstPicks(offer, all), ['plays', 'likes:stash', 'likes:spotify', 'likes:youtube', 'app-1', 'stash-mix-1', 'spotify-album-1', 'sp-1', 'sp-2'], false);
		expect(none).toMatchObject({ plays: false, likes: [], lists: [] });
		const some = setKeys(none, ['sp-2', 'likes:youtube', 'sp-1'], true);
		expect(setKeys(some, ['sp-1', 'app-1'], true)).toMatchObject({ likes: ['youtube'], lists: ['sp-2', 'sp-1', 'app-1'] });
	});
});

describe('the files taken', () => {
	it('are every song the computer lacks with All your downloads', () => {
		expect(filesToTake(offer, lib, firstPicks(offer, lib))).toEqual(['1.flac', '2.flac', '3.flac']);
	});
	it('without it, are the liked ones and those in a playlist ticked', () => {
		const p = { ...firstPicks(offer, lib), downloads: false };
		expect(filesToTake(offer, lib, p)).toEqual(['1.flac', '2.flac']);
		expect(filesToTake(offer, lib, flip(p, 'sp-1'))).toEqual(['1.flac']);
		expect(filesToTake(offer, lib, { ...p, likes: [], lists: [] })).toEqual([]);
	});
	it('for likes, are the songs liked on a service whose likes are ticked (one placed nowhere was liked in Stash)', () => {
		const placed = { ...all, liked_files: { '1.flac': ['spotify', 'youtube'], '4.flac': ['stash'] } };
		const sent = { ...offer, songs: [...offer.songs, song({ file: '5.flac', liked: true })] };
		const p = { ...firstPicks(sent, placed), downloads: false, lists: [] };
		expect(filesToTake(sent, placed, { ...p, likes: ['youtube'] })).toEqual(['1.flac']);
		expect(filesToTake(sent, placed, { ...p, likes: ['stash'] })).toEqual(['5.flac']);
		expect(filesToTake(sent, placed, { ...p, likes: ['soundcloud'] })).toEqual([]);
	});
});

describe('the take', () => {
	it('takes the whole send and every library part; an older phone’s likes all or none', () => {
		expect(takeBody(offer, lib, firstPicks(offer, lib))).toEqual({
			sent_at_ms: 8,
			all: true,
			library: { likes: true, plays: true, playlists: ['app-1', 'sp-1'], only_here: false, downloads: true }
		});
		expect(takeBody(offer, lib, flip(firstPicks(offer, lib), 'likes:all'))).toMatchObject({ library: { likes: false } });
	});
	it('names the services whose likes come', () => {
		expect(takeBody(offer, all, flip(firstPicks(offer, all), 'likes:youtube'))).toMatchObject({ library: { likes: ['stash', 'spotify'] } });
	});
	it('names its files without All your downloads, and takes none when nothing of the send is wanted', () => {
		const p = { ...firstPicks(offer, lib), downloads: false };
		expect(takeBody(offer, lib, p)).toMatchObject({ files: ['1.flac', '2.flac'] });
		expect(takeBody(offer, lib, { ...p, likes: [], lists: [] })).toMatchObject({ files: [], none: true, library: { plays: true } });
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
		expect(t).toMatchObject({ files: 3, away: 2956 + 89 + 21, lists: 2, albums: 0, nothing: false });
		expect(lines(t, p)).toEqual({
			top: '3 downloaded songs · Liked songs · 2 playlists · your plays',
			sub: '3 play offline (120 MB) · up to 3,066 more stream when you’re online'
		});
	});
	it('says which likes come, and counts saved albums apart', () => {
		const p = firstPicks(offer, all);
		expect(lines(tally(offer, all, p), p).top).toBe('3 downloaded songs · all your likes · 4 playlists · 1 album · your plays');
		expect(tally(offer, all, flip(p, 'likes:youtube')).likes).toBe('Stash and Spotify likes');
		expect(tally(offer, all, { ...p, likes: ['spotify'] }).likes).toBe('Spotify Liked Songs');
	});
	it('says nothing streams with only songs downloaded', () => {
		const p = { ...firstPicks(offer, lib), onlyHere: true };
		expect(lines(tally(offer, lib, p), p).sub).toBe('3 play offline (120 MB). Nothing streams.');
	});
	it('says so when nothing is ticked', () => {
		const p = { downloads: false, likes: [], plays: false, lists: [], onlyHere: false };
		const t = tally(offer, lib, p);
		expect(t.nothing).toBe(true);
		expect(lines(t, p)).toEqual({ top: 'Nothing picked yet', sub: 'Pick something to bring it in.' });
	});
});

describe('what the library brought, on the done card', () => {
	it('says what is new, or that it was up to date', () => {
		const none = { likes: 0, playlists_new: 0, playlists_updated: 0, plays: 0, songs: 0 };
		expect(broughtLine(none)).toBe('Your likes, playlists and plays were up to date.');
		expect(broughtLine({ ...none, playlists_new: 1, playlists_updated: 4, songs: 10 })).toBe('From your library: 1 new playlist, 4 updated and 10 new songs to stream.');
		expect(broughtLine({ ...none, likes: 1, plays: 1200 })).toBe('From your library: 1 new like and 1,200 new plays.');
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
