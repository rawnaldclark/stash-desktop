import { describe, expect, it } from 'vitest';
import type { AlbumCard } from '../core';
import { addedBucket, groupAlbums, letterOf, upToCards } from './album-groups';

const DAY = 86_400_000;
const NOW = new Date(2026, 10, 24).getTime();
let next = 1;
const card = (title: string, artist: string, year: number | null, genre: string | null, addedDaysAgo: number): AlbumCard => ({
	id: next++,
	title,
	artist,
	artist_id: 1,
	year,
	tracks: 10,
	art: null,
	genre,
	added_ms: NOW - addedDaysAgo * DAY,
	songs_here: 10
});
const cards = [
	card('In Rainbows', 'Radiohead', 2007, 'Alternative', 2),
	card('Kid A', 'Radiohead', 2000, 'Alternative', 400),
	card('Kind of Blue', 'Miles Davis', 1959, 'Jazz', 20),
	card('21', 'Adele', 2011, 'Pop', 60),
	card('Élan', 'Someone', null, null, 900)
];
const shape = (by: Parameters<typeof groupAlbums>[1]) => groupAlbums(cards, by, NOW).map((g) => [g.label, g.albums.map((a) => a.title)]);

describe('albums grouped', () => {
	it('by name file under their first letter, accents folded, numbers under #', () => {
		expect(letterOf('Élan')).toBe('E');
		expect(letterOf('21')).toBe('#');
		expect(shape('name').map(([l]) => l)).toEqual(['E', 'I', 'K', '#']);
	});

	it('by artist, each artist oldest record first', () => {
		expect(shape('artist')).toEqual([
			['Adele', ['21']],
			['Miles Davis', ['Kind of Blue']],
			['Radiohead', ['Kid A', 'In Rainbows']],
			['Someone', ['Élan']]
		]);
	});

	it('by year, newest decade first, no year last', () => {
		expect(shape('year')).toEqual([
			['2010s', ['21']],
			['2000s', ['In Rainbows', 'Kid A']],
			['1950s', ['Kind of Blue']],
			['No year', ['Élan']]
		]);
	});

	it('by genre, A to Z, no genre last', () => {
		expect(shape('genre').map(([l]) => l)).toEqual(['Alternative', 'Jazz', 'Pop', 'No genre']);
	});

	it('by when they came, newest first', () => {
		expect(shape('added')).toEqual([
			['This week', ['In Rainbows']],
			['This month', ['Kind of Blue']],
			['The last three months', ['21']],
			['2025', ['Kid A']],
			['2024', ['Élan']]
		]);
		expect(addedBucket(0, NOW).label).toBe('Unknown');
	});

	it('are drawn a part at a time', () => {
		const groups = groupAlbums(cards, 'artist', NOW);
		const cut = upToCards(groups, 3);
		expect(cut.map((g) => [g.label, g.albums.length])).toEqual([
			['Adele', 1],
			['Miles Davis', 1],
			['Radiohead', 1]
		]);
		expect(upToCards(groups, 100)).toEqual(groups);
	});
});
