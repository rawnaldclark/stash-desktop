/** The words of the delete question and its answer (DeleteSongs.svelte), kept apart from the screen so they are tested. */
import type { DeletePreview, Removed } from '../core';
import type { IconName } from '../ui/icons';

/** A size as a listener reads it: "38.5 GB", "412 MB", "0.9 MB". */
export function sizeText(bytes: number): string {
	const mb = bytes / (1024 * 1024);
	if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
	return `${mb >= 10 ? Math.round(mb) : mb.toFixed(1)} MB`;
}

const n = (count: number, one: string, many: string) => (count === 1 ? one : many.replace('#', count.toLocaleString()));

/** What the delete touches, one line each; nothing for songs that are only entries in the library. */
export function deleteLines(p: DeletePreview, files: boolean): { icon: IconName; text: string }[] {
	const out: { icon: IconName; text: string }[] = [];
	if (p.in_playlists) {
		const where = n(p.playlists, '1 playlist', '# playlists');
		out.push({ icon: 'playlist_add', text: `${n(p.in_playlists, '1 is', '# are')} taken out of ${where}` });
	}
	if (p.saved_files) {
		out.push({ icon: 'download', text: `${n(p.saved_files, '1 download is', '# downloads are')} deleted from this computer (${sizeText(p.saved_bytes)})` });
	}
	if (p.folder_files && files) {
		out.push({ icon: 'folder', text: `${n(p.folder_files, '1 file is', '# files are')} deleted from your music folders (${sizeText(p.folder_bytes)})` });
	}
	return out;
}

/** What was done, said in one line. */
export function deleteSaid(r: Pick<Removed, 'removed' | 'in_folders' | 'in_playlists'>): string {
	const stayed = r.in_folders + r.in_playlists;
	const gone = r.removed === 0 ? 'Nothing was deleted' : r.removed === 1 ? 'Deleted 1 song' : `Deleted ${r.removed.toLocaleString()} songs`;
	return stayed ? `${gone} · ${n(stayed, '1 stayed', '# stayed')}` : gone;
}
