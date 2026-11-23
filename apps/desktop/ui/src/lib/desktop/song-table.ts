/** The song table's columns and what they show (SongTable.svelte), apart from the screen so they are tested. */
import type { TableRow, TableSort } from '../core';

export type ColumnId = 'track' | 'title' | 'artist' | 'album' | 'genre' | 'year' | 'length' | 'format' | 'added' | 'plays';

export type Column = {
	id: ColumnId;
	name: string;
	/** A share of the width left after the fixed columns, or a fixed width in px. */
	width: string;
	sort?: TableSort;
	/** Numbers, set to the right. */
	num?: boolean;
	/** Always shown. */
	fixed?: boolean;
};

export const COLUMNS: Column[] = [
	{ id: 'track', name: '#', width: '36px', num: true },
	{ id: 'title', name: 'Title', width: 'minmax(140px, 3fr)', sort: 'title', fixed: true },
	{ id: 'artist', name: 'Artist', width: 'minmax(100px, 2fr)', sort: 'artist' },
	{ id: 'album', name: 'Album', width: 'minmax(100px, 2fr)', sort: 'album' },
	{ id: 'genre', name: 'Genre', width: 'minmax(80px, 1.2fr)', sort: 'genre' },
	{ id: 'year', name: 'Year', width: '60px', sort: 'year', num: true },
	{ id: 'length', name: 'Length', width: '76px', sort: 'length', num: true },
	{ id: 'format', name: 'Format', width: '112px', sort: 'format' },
	{ id: 'added', name: 'Added', width: '104px', sort: 'added' },
	{ id: 'plays', name: 'Plays', width: '64px', sort: 'plays', num: true }
];

/** The columns shown until the listener chooses. */
export const DEFAULT_COLUMNS: ColumnId[] = ['title', 'artist', 'album', 'genre', 'year', 'length', 'format'];

/** The listener's kept choice of columns, in the table's own order; the default when there is none or it is nonsense. */
export function readColumns(kept: string | null): ColumnId[] {
	if (!kept) return DEFAULT_COLUMNS;
	const asked = new Set(kept.split(','));
	const known = COLUMNS.filter((c) => c.fixed || asked.has(c.id)).map((c) => c.id);
	return known.length > 1 || asked.has('title') ? known : DEFAULT_COLUMNS;
}

export const gridOf = (columns: Column[]): string => columns.map((c) => c.width).join(' ');

/** A song's format as a listener reads it: "FLAC 24/96", "FLAC 16/44.1", "MP3 320", "AAC". */
export function formatOf(r: Pick<TableRow, 'codec' | 'bit_depth' | 'sample_rate' | 'bitrate'>): string {
	if (!r.codec) return '';
	const name = r.codec === 'pcm' ? 'WAV' : r.codec.toUpperCase();
	if (r.bit_depth && r.sample_rate) {
		const khz = r.sample_rate / 1000;
		return `${name} ${r.bit_depth}/${Number.isInteger(khz) ? khz : khz.toFixed(1)}`;
	}
	return r.bitrate ? `${name} ${r.bitrate}` : name;
}
