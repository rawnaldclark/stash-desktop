// Playlist files (.m3u, .m3u8) in and out: another player's playlist comes in as a playlist here (the core finds each
// entry: a file it has, a file on this computer it adds, or the song the entry names), and a playlist goes out as a
// file other players read (its songs that are files on this computer).
import { goto } from '$app/navigation';
import { playlistFromFile } from '../app';
import * as core from '../core';
import { said } from '../core';
import { ui } from '../ui/state.svelte';

const songsWord = (n: number) => (n === 1 ? '1 song' : `${n} songs`);

/** Asks for a playlist file (the system's own dialog) and makes a playlist of it: opened, and said what came of it. */
export async function importPlaylistFile(): Promise<void> {
	try {
		const brought = await playlistFromFile();
		if (!brought) return;
		const out = brought.left_out ? `; ${brought.left_out} left out (web addresses, or naming no song)` : '';
		ui.toast(`Made ${brought.name}: ${songsWord(brought.songs)}${out}`);
		await goto(`/playlist/${brought.id}`);
	} catch (e) {
		ui.toast(said(e));
	}
}

/** Saves a playlist (`null`: the liked songs) as an .m3u8 file other players read, and says how many songs went in. */
export async function savePlaylistFile(id: string | null): Promise<void> {
	try {
		const file = await core.playlistFile(id === null ? null : Number(id));
		// A file no other player could play a song of is not worth having.
		if (!file.songs) {
			ui.toast('Nothing to save: none of its songs is a file on this computer yet');
			return;
		}
		const a = document.createElement('a');
		a.href = URL.createObjectURL(file.blob);
		a.download = file.name;
		document.body.append(a);
		a.click();
		a.remove();
		setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
		const out = file.leftOut
			? `. ${file.leftOut === 1 ? 'One song' : `${file.leftOut} songs`} only your add-ons play ${file.leftOut === 1 ? 'is' : 'are'} not in it`
			: '';
		ui.toast(`Saved ${file.name}: ${songsWord(file.songs)}${out}`);
	} catch (e) {
		ui.toast(said(e));
	}
}
