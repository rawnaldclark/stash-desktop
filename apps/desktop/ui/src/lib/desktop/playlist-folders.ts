/**
 * Playlist folders in the window: the menus of a playlist and of a folder, and what they do. A folder holds playlists
 * (the sidebar folds it shut or open; Library › Playlists opens it), and deleting one keeps its playlists.
 */
import { goto } from '$app/navigation';
import { library } from '../app';
import type { Playlist, PlaylistFolder } from '../api';
import { L, ui, type Choice } from '../ui/state.svelte';

/** Where a menu opens: a point, and the element focus goes back to. */
export type At = { x: number; y: number; anchor?: HTMLElement | null; up?: boolean };

/** Under a button. */
export function under(el: HTMLElement): At {
	const r = el.getBoundingClientRect();
	return { x: r.left, y: r.bottom + 4, anchor: el };
}
/** Where a right-click (or the menu key, which names no point) opened it. */
export function atEvent(e: MouseEvent): At {
	const el = e.currentTarget instanceof HTMLElement ? e.currentTarget : null;
	if ((e.clientX || e.clientY) || !el) return { x: e.clientX, y: e.clientY, anchor: el };
	return under(el);
}

export const folderHref = (id: number) => `/library?tab=playlists&folder=${id}`;
/** "3 playlists". */
export const playlistsWord = (n: number) => (n === 1 ? '1 playlist' : `${n} playlists`);

/** Puts a playlist in a folder (or, null, out of any), saying so when `say`. */
export function moveTo(p: Pick<Playlist, 'id' | 'name' | 'folder'>, folder: number | null, say = true): void {
	if ((p.folder ?? null) === folder) return;
	const into = folder === null ? undefined : L.folders.find((f) => f.id === folder)?.name;
	void library.moveToFolder(p.id, folder).then(
		() => say && ui.toast(into ? `Moved ${p.name} to ${into}` : `${p.name} is out of its folder`),
		() => {}
	);
}

/** A playlist's menu: the folder it is in (or none), and a new folder for it. */
export function playlistMenu(p: Playlist, at: At): void {
	const choices: Choice[] = L.folders.map((f) => ({ icon: 'folder', label: f.name, checked: p.folder === f.id, run: () => moveTo(p, f.id) }));
	choices.push({ icon: 'library_music', label: 'Not in a folder', checked: !p.folder, run: () => moveTo(p, null) });
	choices.push({
		icon: 'create_new_folder',
		label: 'New folder',
		ask: {
			placeholder: 'Folder name',
			submit: 'Create',
			make: (name) => void library.createFolder(name, p.id).then(() => ui.toast(`Moved ${p.name} to ${name}`), () => {})
		}
	});
	ui.openChoices({ ...at, label: `Folder for ${p.name}`, title: 'Move to folder', choices });
}

/** Deletes a folder; its playlists stay, out of any folder. Said with a way back: the folder again, with them in it. */
export function deleteFolder(f: PlaylistFolder): void {
	const inside = L.playlists.filter((p) => p.folder === f.id).map((p) => p.id);
	const undo = async () => {
		const id = await library.createFolder(f.name);
		for (const p of inside) await library.moveToFolder(p, id);
		if (f.collapsed) await library.foldFolder(id, true);
	};
	void library.deleteFolder(f.id).then(
		() => ui.toast(inside.length ? `Deleted ${f.name}. Its playlists stay` : `Deleted ${f.name}`, { label: 'Undo', run: () => void undo().catch(() => {}) }),
		() => {}
	);
}

/** A folder's menu: open it, rename it, delete it. */
export function folderMenu(f: PlaylistFolder, at: At): void {
	ui.openChoices({
		...at,
		label: `Options for ${f.name}`,
		choices: [
			{ icon: 'folder_open', label: 'Open', run: () => void goto(folderHref(f.id)) },
			{ icon: 'edit', label: 'Rename', ask: { value: f.name, placeholder: 'Folder name', submit: 'Save', make: (name) => void library.renameFolder(f.id, name) } },
			{ icon: 'delete', label: 'Delete folder', danger: true, run: () => deleteFolder(f) }
		]
	});
}

/** The sidebar's "new" menu: a playlist (opened once made), a smart playlist, a folder. */
export function newMenu(at: At): void {
	ui.openChoices({
		...at,
		label: 'Make something new',
		choices: [
			{
				icon: 'add',
				label: 'New playlist',
				ask: {
					placeholder: 'Playlist name',
					submit: 'Create',
					make: (name) => void library.createPlaylist(name).then((p) => goto(`/playlist/${p.id}`), () => {})
				}
			},
			{ icon: 'queue_music', label: 'New smart playlist', run: () => (ui.smart = { id: null }) },
			{ icon: 'create_new_folder', label: 'New folder', ask: { placeholder: 'Folder name', submit: 'Create', make: (name) => void library.createFolder(name).catch(() => {}) } }
		]
	});
}
