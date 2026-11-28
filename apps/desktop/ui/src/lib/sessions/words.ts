/**
 * What a session's ids say and wear on this page: the quick phrases (sent as ids, `PHRASES` in the share Worker's room.js; the
 * same words as the app's strings.xml `phrase_*`), the reasons a session ends, and each person's colour (the app's
 * PersonColor.kt: the same eight, picked by the same hash of the member id, so a person wears one colour on every screen).
 */

export const PHRASES = ['hi', 'who', 'tune', 'again', 'louder', 'more', 'chills', 'thanks', 'morning', 'night'] as const;
export type PhraseId = (typeof PHRASES)[number];

const PHRASE_WORDS: Record<PhraseId, string> = {
	hi: 'Hi all',
	who: 'Who is this?',
	tune: 'Tune!',
	again: 'Run it back!',
	louder: 'Turn it up',
	more: 'More like this',
	chills: 'Chills',
	thanks: 'Thanks for the set',
	morning: 'Good morning',
	night: 'Good night'
};

export const isPhrase = (v: unknown): v is PhraseId => typeof v === 'string' && Object.hasOwn(PHRASE_WORDS, v);
export const phraseText = (id: string): string | null => (isPhrase(id) ? PHRASE_WORDS[id] : null);

/** Why this tab is out of the session, in a line. */
export function endedText(reason: string, host: string): string {
	switch (reason) {
		case 'host_ended':
			return `${host} ended the session.`;
		case 'host_left':
			return `${host} left, so the session ended.`;
		case 'paused':
			return 'The session ended after 15 minutes paused.';
		case 'max_age':
			return 'The session ended after 12 hours.';
		case 'station_removed':
			return 'This station is no longer available.';
		case 'replaced':
			return `${host} started a new session.`;
		case 'last_song':
			return 'That was the last song. Thanks for listening.';
		case 'booted':
			return `${host} removed you from the session.`;
		case 'update':
			return 'This player needs an update to join. Reload the page.';
		case 'full':
			return 'This session is full (50 listeners).';
		case 'key':
			return 'Join Community in this browser to tune in.';
		case 'lost':
			return 'Lost the connection to the session.';
		default:
			return 'The session ended.';
	}
}

/** The app's eight person colours. */
export const PERSON_COLORS = ['#A78BFA', '#F472B6', '#22D3EE', '#FBBF24', '#34D399', '#FB923C', '#60A5FA', '#A3E635'] as const;

/** Java's String.hashCode, as the app picks a colour by it. */
function javaHash(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) h = (Math.imul(h, 31) + s.charCodeAt(i)) | 0;
	return h;
}

/** A person's colour, from their member id (Math.floorMod, as the app does). */
export function personColor(memberId: string): string {
	const n = PERSON_COLORS.length;
	return PERSON_COLORS[((javaHash(memberId) % n) + n) % n]!;
}

/** A person's initial for their face: the first letter of their name, or "?" without one. */
export function initialOf(name: string | null | undefined): string {
	const c = (name ?? '').trim().match(/\p{L}|\p{N}/u)?.[0];
	return c ? c.toUpperCase() : '?';
}
