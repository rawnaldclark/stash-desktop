import { describe, expect, it } from 'vitest';
import { decadeName, folderRows } from './browse';

describe('the folders of the library', () => {
	const rows = [
		{ folder: 'D:\\Music\\Radiohead\\In Rainbows', songs: 10 },
		{ folder: 'D:\\Music\\Radiohead\\Kid A', songs: 11 },
		{ folder: 'D:\\Music\\Singles', songs: 3 },
		{ folder: 'C:\\Users\\me\\Music\\Stash\\Miles Davis', songs: 5 }
	];

	it('are listed under their music folder, with the folders between', () => {
		const out = folderRows(rows, ['D:\\Music\\']);
		const under = out.filter((f) => f.path.startsWith('D:'));
		expect(under.map((f) => [f.name, f.depth, f.songs, f.below])).toEqual([
			['D:\\Music', 0, 0, 24],
			['Radiohead', 1, 0, 21],
			['In Rainbows', 2, 10, 10],
			['Kid A', 2, 11, 11],
			['Singles', 1, 3, 3]
		]);
	});

	it('show a folder outside every music folder by itself', () => {
		const out = folderRows(rows, ['D:\\Music']);
		const alone = out.find((f) => f.path.startsWith('C:'))!;
		expect([alone.depth, alone.songs, alone.name]).toEqual([0, 5, 'C:\\Users\\me\\Music\\Stash\\Miles Davis']);
	});

	it('do not take a folder whose name only starts the same for one inside it', () => {
		const out = folderRows([{ folder: 'D:\\Music2\\A', songs: 1 }], ['D:\\Music']);
		expect(out.map((f) => [f.path, f.depth])).toEqual([['D:\\Music2\\A', 0]]);
	});
});

describe('a decade', () => {
	it('is said as people say it', () => {
		expect(decadeName(1990)).toBe('1990s');
		expect(decadeName(null)).toBe('No year');
	});
});
