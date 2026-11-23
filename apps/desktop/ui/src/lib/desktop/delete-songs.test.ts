import { describe, expect, it } from 'vitest';
import { deleteLines, deleteSaid, sizeText } from './delete-songs';

const none = { songs: 0, in_playlists: 0, playlists: 0, saved_files: 0, saved_bytes: 0, folder_files: 0, folder_bytes: 0 };

describe('the delete question', () => {
	it('says nothing more for songs that are only entries in the library', () => {
		expect(deleteLines({ ...none, songs: 12 })).toEqual([]);
	});

	it('names the playlists, the downloads and the files, each with its count', () => {
		const lines = deleteLines({ songs: 40, in_playlists: 3, playlists: 2, saved_files: 30, saved_bytes: 1.5 * 1024 ** 3, folder_files: 1, folder_bytes: 9.4 * 1024 ** 2 });
		expect(lines.map((l) => l.text)).toEqual([
			'3 are taken out of 2 playlists',
			'30 downloads are deleted from this computer (1.5 GB)'
		]);
	});

	it('leaves the files of the music folders to the switch in the question', () => {
		expect(deleteLines({ ...none, songs: 2, folder_files: 2, folder_bytes: 50 * 1024 ** 2 })).toEqual([]);
	});

	it('speaks of one playlist and one download in the singular', () => {
		const lines = deleteLines({ ...none, songs: 1, in_playlists: 1, playlists: 1, saved_files: 1, saved_bytes: 300 * 1024 ** 2 });
		expect(lines.map((l) => l.text)).toEqual(['1 is taken out of 1 playlist', '1 download is deleted from this computer (300 MB)']);
	});
});

describe('sizes', () => {
	it('reads as megabytes, then gigabytes', () => {
		expect(sizeText(0.94 * 1024 ** 2)).toBe('0.9 MB');
		expect(sizeText(412 * 1024 ** 2)).toBe('412 MB');
		expect(sizeText(38.5 * 1024 ** 3)).toBe('38.5 GB');
	});
});

describe('what was done', () => {
	it('counts what went and what stayed', () => {
		expect(deleteSaid({ removed: 891, in_folders: 0, in_playlists: 0 })).toBe('Deleted 891 songs');
		expect(deleteSaid({ removed: 1, in_folders: 0, in_playlists: 0 })).toBe('Deleted 1 song');
		expect(deleteSaid({ removed: 10, in_folders: 2, in_playlists: 0 })).toBe('Deleted 10 songs · 2 stayed');
		expect(deleteSaid({ removed: 0, in_folders: 1, in_playlists: 0 })).toBe('Nothing was deleted · 1 stayed');
	});
});
