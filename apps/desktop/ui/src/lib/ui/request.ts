/**
 * Asks the session's DJ for a song (Sessions: while tuned in, song rows and the song menu request instead of play).
 * The Sessions chunk is loaded already whenever a session is (tuning in loaded it), so this costs the shell nothing.
 */
import type { LibraryItem } from '../api';

export function requestSong(item: LibraryItem): void {
	void import('../sessions/session.svelte').then((m) => m.session.request(item)).catch(() => {});
}
