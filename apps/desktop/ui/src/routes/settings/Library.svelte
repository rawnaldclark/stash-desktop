<!-- Settings › Library: music folders, offline mode, songs kept as they were played, and one file that carries a library
     between this computer, the phone and the web player (and playlists from other players). -->
<script lang="ts">
	import { goOffline, library } from '../../lib/app';
	import * as core from '../../lib/core';
	import { folders, lastPart, many, trouble, type Notice } from '../../lib/desktop/folders.svelte';
	import { importPlaylistFile } from '../../lib/desktop/playlist-files';
	import { ui } from '../../lib/ui/state.svelte';
	import { prefs } from './prefs.svelte';

	let importing = $state(false);
	/** What the last import or save said. */
	let fileNotice = $state<Notice | null>(null);

	/** What an import did, in a line. */
	function imported(result: core.Imported): string {
		const s = result.summary;
		const parts = [
			s.songs_added ? many(s.songs_added, 'new song', 'new songs') : '',
			s.likes_added ? many(s.likes_added, 'like', 'likes') : '',
			s.playlists_added ? many(s.playlists_added, 'playlist', 'playlists') : '',
			s.playlist_songs_added ? `${many(s.playlist_songs_added, 'song', 'songs')} in existing playlists` : '',
			s.plays_added ? many(s.plays_added, 'play', 'plays') : '',
			s.blocks_added ? `${many(s.blocks_added, 'song', 'songs')} not to play` : ''
		].filter((p) => p !== '');
		return parts.length ? `Imported ${parts.join(', ')}` : 'Nothing new to import';
	}

	async function importFile(e: Event & { currentTarget: HTMLInputElement }) {
		const file = e.currentTarget.files?.[0];
		e.currentTarget.value = '';
		if (!file || importing) return;
		importing = true;
		fileNotice = null;
		try {
			const result = await core.importLibrary(file);
			await library.reload();
			fileNotice = { text: imported(result), trouble: false };
		} catch (x) {
			fileNotice = { text: trouble(x), trouble: true };
		} finally {
			importing = false;
		}
	}

	/** Saves the library as the file the Android app and the web player import. */
	async function saveFile() {
		try {
			const { blob, likes, lists, plays } = await core.exportLibrary();
			const name = `stash-library-${new Date().toISOString().slice(0, 10)}.json`;
			const a = document.createElement('a');
			a.href = URL.createObjectURL(blob);
			a.download = name;
			document.body.append(a);
			a.click();
			a.remove();
			setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
			fileNotice = { text: `Saved ${name}: ${many(likes, 'like', 'likes')}, ${many(lists, 'playlist', 'playlists')}, ${many(plays, 'play', 'plays')}`, trouble: false };
		} catch (x) {
			fileNotice = { text: trouble(x), trouble: true };
		}
	}

	// ---- Songs kept as they were played: how much room they may take.
	const ROOMS: [number, string][] = [
		[0, 'Off'],
		[1024, '1 GB'],
		[2048, '2 GB'],
		[5120, '5 GB'],
		[10240, '10 GB']
	];
	const room = $derived(prefs.room);
	async function setRoom(cap_mb: number) {
		try {
			prefs.room = await core.setPlayedRoom(cap_mb);
		} catch (e) {
			ui.toast(core.said(e));
		}
	}
	async function clearRoom() {
		try {
			prefs.room = await core.clearPlayed();
		} catch (e) {
			ui.toast(core.said(e));
		}
	}
	const roomSize = (bytes: number) => (bytes >= 1_073_741_824 ? `${(bytes / 1_073_741_824).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1_048_576))} MB`);
</script>

<div class="set-card">
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name">Music folders</span>
			{#if folders.list.length}
				{#each folders.list as f (f)}<span class="set-sub set-path" title={f}>{f}</span>{/each}
			{:else}
				<span class="set-sub">Your files stay where they are</span>
			{/if}
			{#if folders.scan}<span class="set-sub" role="status">Scanning {folders.scan.folder ? lastPart(folders.scan.folder) : ''}… {many(folders.scan.report?.files ?? 0, 'file', 'files')}</span>{/if}
			{#if folders.notice}<span class="set-sub" class:set-trouble={folders.notice.trouble} role="status">{folders.notice.text}</span>{/if}
		</div>
		{#if !folders.scan}
			<div class="set-ctl">
				{#if folders.list.length}<button type="button" class="btn ghost" onclick={() => folders.again()}>Rescan</button>{/if}
				<button type="button" class="btn ghost" onclick={() => folders.add()}>Add folder…</button>
			</div>
		{/if}
	</div>
</div>

<div class="set-card">
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name" id="off-l">Offline mode</span>
			<span class="set-sub">Only play what’s on this computer</span>
		</div>
		<button type="button" class="sw" role="switch" aria-checked={ui.offline} aria-labelledby="off-l" onclick={() => void goOffline(!ui.offline)}><i></i></button>
	</div>
	{#if room}
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="kp-l">Keep songs you play</span>
				{#if room.songs}
					<span class="set-sub" role="status">{many(room.songs, 'song', 'songs')} kept · {roomSize(room.bytes)} <button type="button" class="set-clear" onclick={clearRoom}>Clear</button></span>
				{:else}
					<span class="set-sub">Saves streamed songs for offline play</span>
				{/if}
			</div>
			<div class="seg" role="group" aria-labelledby="kp-l">
				{#each ROOMS as [mb, label] (mb)}
					<button type="button" class="seg-i" aria-pressed={room.cap_mb === mb} onclick={() => setRoom(mb)}>{label}</button>
				{/each}
			</div>
		</div>
	{/if}
</div>

<div class="set-card">
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name">Transfer library</span>
			<span class="set-sub">Likes, playlists and plays, across devices</span>
			{#if fileNotice}<span class="set-sub" class:set-trouble={fileNotice.trouble} role="status">{fileNotice.text}</span>{/if}
		</div>
		<div class="set-ctl">
			<label class="btn ghost bk-pick" aria-disabled={importing}>{importing ? 'Importing…' : 'Import…'}<input class="vh" type="file" accept=".json,.zip,.db,application/json,application/zip" aria-label="Import a library file" disabled={importing} onchange={importFile} /></label>
			<button type="button" class="btn ghost" aria-label="Export a library file" onclick={saveFile}>Export</button>
		</div>
	</div>
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name">Import a playlist</span>
			<span class="set-sub">M3U files from other players</span>
		</div>
		<div class="set-ctl">
			<button type="button" class="btn ghost" aria-label="Import a playlist file" onclick={() => void importPlaylistFile()}>Import…</button>
		</div>
	</div>
</div>
