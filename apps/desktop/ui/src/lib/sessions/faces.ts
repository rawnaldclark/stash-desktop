/**
 * A session's reactions: old-school text faces, sent as ids (owner 2026-11-27: "get rid of emoji reactions they look cheap… an
 * assortment of oldschool text/symbol emojies like /shrug"). The same list, in the same order, as the share Worker's `FACES`
 * (MP3APK infra/share-worker/src/room.js) and the app's `RoomProtocol.FACES` with `faceFrames` (feature/nowplaying SessionLifeUi.kt):
 * keep them in step. A session takes only these; an id this player doesn't know is shown as nothing.
 *
 * Each face is its frames: one for most, two for a big one, which plays over everyone's screen, larger, stepping between them as it
 * rises. The last frame is the face at rest, as the React tray shows it.
 */
export type FaceId = (typeof FACES)[number];

export const FACES = [
	'k:shrug',
	'k:lenny',
	'k:yay',
	'k:love',
	'k:cool',
	'k:look',
	'k:wow',
	'k:cry',
	'k:wave',
	'k:notes',
	'k:heart',
	'k:bear',
	'k:tableflip',
	'k:unflip',
	'k:dance',
	'k:rockon',
	'k:sparkle'
] as const;

const FRAMES: Record<FaceId, readonly string[]> = {
	'k:shrug': [String.raw`¯\_(ツ)_/¯`],
	'k:lenny': ['( ͡° ͜ʖ ͡°)'],
	'k:yay': [String.raw`\(^o^)/`],
	'k:love': ['(♥‿♥)'],
	'k:cool': ['(⌐■_■)'],
	'k:look': ['ಠ_ಠ'],
	'k:wow': ['(⊙_⊙)'],
	'k:cry': ['(╥﹏╥)'],
	'k:wave': ['( ﾟ◡ﾟ)/'],
	'k:notes': ['♪♫♪'],
	'k:heart': ['<3'],
	'k:bear': ['ʕ•ᴥ•ʔ'],
	'k:tableflip': ['(°□°) ┬─┬', '(╯°□°)╯︵ ┻━┻'],
	'k:unflip': ['┻━┻ ( º _ º)', '┬─┬ノ( º _ ºノ)'],
	'k:dance': ['┏(･o･)┛♪', '♪┗(･o･)┓'],
	'k:rockon': [String.raw`\m/ (>_<) \m/`, String.raw`\m/ (^o^) \m/`],
	'k:sparkle': ['(ﾉ◕ヮ◕)ﾉ', '(ﾉ◕ヮ◕)ﾉ*:･ﾟ✧']
};

export const isFace = (v: unknown): v is FaceId => typeof v === 'string' && Object.hasOwn(FRAMES, v);

/** A face's frames, or null for an id this player doesn't know. */
export const faceFrames = (id: string): readonly string[] | null => (isFace(id) ? FRAMES[id] : null);

/** The face at rest, or null for an id this player doesn't know. */
export const faceText = (id: string): string | null => faceFrames(id)?.at(-1) ?? null;

/** A big one: it plays over everyone's screen, larger, stepping between its frames. */
export const isBigFace = (id: string): boolean => (faceFrames(id)?.length ?? 0) > 1;

/** Its name, as the tray shows it under the face and as it is typed: "/shrug". A screen reader says it without the slash. */
export const faceName = (id: string): string => '/' + id.replace(/^k:/, '');

/**
 * What a typed command picks: "/shrug" or "shrug", or the start of one ("/tab" is /tableflip) when only one face starts so. Null
 * when nothing or more than one matches.
 */
export function faceFromCommand(typed: string): FaceId | null {
	const t = typed.trim().toLowerCase().replace(/^\//, '');
	if (!t) return null;
	const exact = FACES.find((f) => f === `k:${t}`);
	if (exact) return exact;
	const starts = FACES.filter((f) => f.startsWith(`k:${t}`));
	return starts.length === 1 ? starts[0] : null;
}
