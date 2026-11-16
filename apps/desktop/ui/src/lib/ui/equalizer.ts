// The equalizer's words: the rows of Settings › Sound that set it, the line that says what it does to the sound, and the
// command palette's switch for it. Everything is read from what the core says of the output in use (`eq`).
import type { CoreEq, Device } from '../core';

/** An output's name for the listener: "The system’s choice", or the system's own name for it. */
export const outName = (d: Device) => (d.name === 'auto' ? 'The system’s choice' : d.description || d.name);

/** The output in use has something set: a correction, or a tone other than As is. */
const isSet = (eq: CoreEq) => eq.correction !== null || eq.bass !== 'as-is' || eq.treble !== 'as-is';

const count = (n: number, one: string, more: string) => `${n} ${n === 1 ? one : more}`;

/** What the Equalizer row says under its name: which output it is for (`single`: there is one output only). */
export function eqSub(eq: CoreEq, single: boolean): string {
	if (single) return 'Changes the sound of every song.';
	const keeps = 'Stash keeps one for each output and puts it back whenever you play through it.';
	// "auto" is an output the engine names no further: it is the system's choice already.
	if (eq.output.name === 'auto') return `For the system’s choice. ${keeps}`;
	if (eq.system_choice) return `For ${outName(eq.output)}, the system’s choice right now. ${keeps}`;
	return `For ${outName(eq.output)}. ${keeps}`;
}

type Shown = NonNullable<CoreEq['correction']>;

/** The correction, in one line: "Sennheiser HD 650 · 10 filters", and the filters the file had switched off. */
export function correctionLine(c: Shown): string {
	const off = c.switched_off ? ` · ${c.switched_off} switched off in the file ${c.switched_off === 1 ? 'was' : 'were'} left out` : '';
	return `${c.name} · ${count(c.filters, 'filter', 'filters')}${off}`;
}

/** What the equalizer does to the sound now, in a line (null when there is nothing to say). */
export function eqStatus(eq: CoreEq): string | null {
	const again = 'Bit-perfect again from the next pause.';
	if (!isSet(eq)) return eq.reopen_at_pause ? again : null;
	if (!eq.on) return eq.reopen_at_pause ? `Off: songs play as they were made. ${again}` : 'Off: songs play as they were made.';
	const changed = 'While the equalizer is on, the samples are changed, so nothing is bit-perfect. Songs that run straight into each other may tick where they meet.';
	return eq.softer_db >= 0.05 ? `Stash plays it ${eq.softer_db.toFixed(1)} dB softer to leave room for the boosts. ${changed}` : changed;
}

/** Why the equalizer was turned off by itself: the core's own sentence (`CoreEq.trouble`), ended with a full stop. */
export function eqTrouble(trouble: string | null): string | null {
	if (!trouble) return null;
	return /[.!?]$/.test(trouble) ? trouble : `${trouble}.`;
}

/** The command palette's entry: the switch, offered only when the output in use has something set. `on` is what a choice
 * turns it to. */
export function eqEntry(eq: CoreEq): { label: string; sub: string; on: boolean } | null {
	if (!isSet(eq)) return null;
	return { label: eq.on ? 'Equalizer off' : 'Equalizer on', sub: outName(eq.output), on: !eq.on };
}
