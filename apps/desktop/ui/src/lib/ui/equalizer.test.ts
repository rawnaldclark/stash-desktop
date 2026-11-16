import { describe, expect, it } from 'vitest';
import type { CoreEq } from '../core';
import { correctionLine, eqEntry, eqStatus, eqSub, eqTrouble, outName } from './equalizer';

const phones = {
	name: 'Test Phones',
	file: 'Test Phones ParametricEQ.txt',
	filters: 3,
	switched_off: 0,
	passed_over: 0,
	summary: 'Read 3 filters and a preamp of −4.0 dB from Test Phones ParametricEQ.txt.'
};

const eq = (over: Partial<CoreEq> = {}): CoreEq => ({
	output: { name: 'wasapi/{test-a}', description: 'Headphones (Test Audio)' },
	system_choice: false,
	on: false,
	bass: 'as-is',
	treble: 'as-is',
	correction: null,
	softer_db: 0,
	describe: null,
	reopen_at_pause: false,
	trouble: null,
	...over
});

describe('eqEntry (the command palette’s Equalizer on/off)', () => {
	it('not offered when the output in use has nothing set, on or off', () => {
		expect(eqEntry(eq())).toBeNull();
		expect(eqEntry(eq({ on: true }))).toBeNull();
	});
	it('turns the switch over, named for the output in use', () => {
		expect(eqEntry(eq({ on: true, bass: 'more', softer_db: 5 }))).toEqual({ label: 'Equalizer off', sub: 'Headphones (Test Audio)', on: false });
		expect(eqEntry(eq({ correction: phones }))).toEqual({ label: 'Equalizer on', sub: 'Headphones (Test Audio)', on: true });
		expect(eqEntry(eq({ treble: 'less', output: { name: 'auto', description: 'Autoselect device' } }))?.sub).toBe('The system’s choice');
	});
});

describe('eqSub (under the Equalizer row’s name)', () => {
	it('names the output it is for', () => {
		expect(eqSub(eq(), false)).toBe('For Headphones (Test Audio). Stash keeps one for each output and puts it back whenever you play through it.');
		expect(eqSub(eq({ system_choice: true }), false)).toBe(
			'For Headphones (Test Audio), the system’s choice right now. Stash keeps one for each output and puts it back whenever you play through it.'
		);
	});
	it('an output the engine names no further is the system’s choice, in the middle of the sentence too', () => {
		const auto = { name: 'auto', description: '' };
		const said = 'For the system’s choice. Stash keeps one for each output and puts it back whenever you play through it.';
		expect(eqSub(eq({ output: auto }), false)).toBe(said);
		expect(eqSub(eq({ output: auto, system_choice: true }), false)).toBe(said);
	});
	it('with one output only, it changes every song', () => {
		expect(eqSub(eq({ system_choice: true }), true)).toBe('Changes the sound of every song.');
	});
});

describe('the correction row', () => {
	it('its line, with the filters the file switched off', () => {
		expect(correctionLine(phones)).toBe('Test Phones · 3 filters');
		expect(correctionLine({ ...phones, filters: 1 })).toBe('Test Phones · 1 filter');
		expect(correctionLine({ ...phones, switched_off: 1 })).toBe('Test Phones · 3 filters · 1 switched off in the file was left out');
		expect(correctionLine({ ...phones, switched_off: 2 })).toBe('Test Phones · 3 filters · 2 switched off in the file were left out');
	});
});

describe('eqStatus (what the equalizer does to the sound)', () => {
	const changed = 'While the equalizer is on, the samples are changed, so nothing is bit-perfect. Songs that run straight into each other may tick where they meet.';
	it('on with boosts: how much softer, and what that costs', () => {
		expect(eqStatus(eq({ on: true, bass: 'more', softer_db: 5 }))).toBe(`Stash plays it 5.0 dB softer to leave room for the boosts. ${changed}`);
		expect(eqStatus(eq({ on: true, correction: phones, softer_db: 7.4 }))).toBe(`Stash plays it 7.4 dB softer to leave room for the boosts. ${changed}`);
	});
	it('on with cuts only: nothing is played softer', () => {
		expect(eqStatus(eq({ on: true, bass: 'less' }))).toBe(changed);
	});
	it('off with something kept: songs as they were made, and bit-perfect again from the next pause when that waits', () => {
		expect(eqStatus(eq({ correction: phones }))).toBe('Off: songs play as they were made.');
		expect(eqStatus(eq({ correction: phones, reopen_at_pause: true }))).toBe('Off: songs play as they were made. Bit-perfect again from the next pause.');
	});
	it('nothing to say with nothing set', () => {
		expect(eqStatus(eq())).toBeNull();
		expect(eqStatus(eq({ on: true }))).toBeNull();
	});
});

describe('eqTrouble', () => {
	it('why the engine could not use it, as the core says it', () => {
		expect(eqTrouble(null)).toBeNull();
		const said = 'Your equalizer could not be used and was turned off: Audio filter initialized failed!';
		expect(eqTrouble(said)).toBe(said);
		expect(eqTrouble('Your equalizer could not be used and was turned off: no such filter')).toBe('Your equalizer could not be used and was turned off: no such filter.');
	});
	it('a correction that could not be read again is said as the core says it', () => {
		const said = 'Your correction “Test Phones” could not be read again: Line 4: no frequency (Fc … Hz).';
		expect(eqTrouble(said)).toBe(said);
	});
});

describe('outName', () => {
	it('the system’s choice, or the system’s own name for the output', () => {
		expect(outName({ name: 'auto', description: 'Autoselect device' })).toBe('The system’s choice');
		expect(outName({ name: 'wasapi/{test-b}', description: '' })).toBe('wasapi/{test-b}');
	});
});
