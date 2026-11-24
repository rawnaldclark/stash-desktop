/** The Edit tags form's fields and what is sent (EditTags.svelte), apart from the screen so they are tested. */
import type { EditReport, TagsNow } from '../core';

export type FieldId = 'title' | 'artist' | 'album' | 'album_artist' | 'genre' | 'year' | 'track_no' | 'disc_no' | 'composer';

export const FIELDS: { id: FieldId; name: string; wide?: boolean; number?: boolean }[] = [
	{ id: 'title', name: 'Title', wide: true },
	{ id: 'artist', name: 'Artist', wide: true },
	{ id: 'album', name: 'Album', wide: true },
	{ id: 'album_artist', name: 'Album artist', wide: true },
	{ id: 'genre', name: 'Genre', wide: true },
	{ id: 'year', name: 'Year', number: true },
	{ id: 'track_no', name: 'Track', number: true },
	{ id: 'composer', name: 'Composer', wide: true },
	{ id: 'disc_no', name: 'Disc', number: true }
];

/**
 * The fields to write: the ones typed in whose text is not what every song already says. A field never touched is
 * left out, and so is one typed back to what it was. Where the songs differ, anything typed (even nothing, which
 * clears it) is a change.
 */
export function changedFields(now: TagsNow, typed: Partial<Record<FieldId, string>>): Partial<Record<FieldId, string>> {
	const out: Partial<Record<FieldId, string>> = {};
	for (const f of FIELDS) {
		const text = typed[f.id];
		if (text === undefined) continue;
		const same = now[f.id].value;
		if (same !== null && text.trim() === same.trim()) continue;
		out[f.id] = text.trim();
	}
	return out;
}

/** What an edit did, in one line. */
export function editSaid(r: EditReport): string {
	const n = (count: number, one: string, many: string) => (count === 1 ? one : many.replace('#', count.toLocaleString()));
	const done = r.written === 0 ? 'Nothing was written' : n(r.written, 'Saved to 1 file', 'Saved to # files');
	if (!r.failed.length) return done;
	const why = r.failed[0][1];
	return `${done} · ${n(r.failed.length, '1 file could not be written', '# files could not be written')} (${why})`;
}
