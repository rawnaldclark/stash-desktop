/**
 * Playback speed and Tuning in words, after Android's (`PlaybackSpeedText.kt`, `TuningMath`): its presets, its limits and
 * its way of reading what is typed (one decimal mark, a comma taken as one).
 */

/** YouTube Music's presets: 0.25x to 2x in quarter steps. */
export const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2];
export const MIN_SPEED = 0.1;
export const MAX_SPEED = 4;
/** Tuning's quick choices: as recorded, then a little lower and a little higher. */
export const TUNINGS = [440, 432, 444];
export const MIN_HZ = 392;
export const MAX_HZ = 494;

/** "1x", "1.25x". */
export const formatSpeed = (s: number) => `${Number.isInteger(s) ? s : Number(s.toFixed(2))}x`;
/** What the picker calls a speed: 1x is "Normal". */
export const speedLabel = (s: number) => (s === 1 ? 'Normal' : formatSpeed(s));
/** The speed on the button, or nothing as recorded. */
export const speedMark = (s: number) => (s === 1 ? null : formatSpeed(s));
/** Tuning on the button ("A432"), or nothing at 440. */
export const tuningMark = (hz: number) => (hz === 440 ? null : `A${hz}`);
/** "Tuning · Standard", or "Tuning · A = 432 Hz". */
export const tuningHeading = (hz: number) => (hz === 440 ? 'Tuning · Standard' : `Tuning · A = ${hz} Hz`);

/** What a typed number asks for: nothing typed, a value, or something that is not one within the limits. */
export type Entry = { value: number | null; wrong: boolean };
function read(text: string, min: number, max: number, places: number): Entry {
	const t = text.trim().replace(',', '.');
	if (!t) return { value: null, wrong: false };
	if (!new RegExp(`^\\d*(\\.\\d{0,${places}})?$`).test(t)) return { value: null, wrong: true };
	const v = Number(t);
	if (!Number.isFinite(v) || v < min || v > max) return { value: null, wrong: true };
	return { value: v, wrong: false };
}
/** A speed of 0.1x to 4x, two decimals at most. */
export const readSpeed = (text: string) => read(text, MIN_SPEED, MAX_SPEED, 2);
/** A tuning of 392 to 494 Hz, one decimal at most. */
export const readTuning = (text: string) => read(text, MIN_HZ, MAX_HZ, 1);
