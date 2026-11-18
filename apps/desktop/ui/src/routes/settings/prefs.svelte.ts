/**
 * What the Settings categories read from the core, read once when Settings opens (and the outputs and the add-ons' use
 * again when the window comes back to the front), so a category opened later shows its switches as they are at once.
 */
import * as core from '../../lib/core';

type Prefs = {
	/** Where the sound goes: the system's outputs. */
	outs: core.Devices | null;
	autoplay: boolean | null;
	lrclib: boolean | null;
	tray: boolean | null;
	/** Songs kept as they were played: how much room they may take. */
	room: core.PlayedRoom | null;
	/** What each add-on was asked for today, when it last gave a song, its last trouble. */
	used: Record<string, core.AddonUse>;
};

export const prefs = $state<Prefs>({ outs: null, autoplay: null, lrclib: null, tray: null, room: null, used: {} });

/** The outputs, read again whenever the window comes back to the front (one may have been plugged in). */
export const readOuts = () =>
	core.devices().then(
		(d) => (prefs.outs = d),
		() => {}
	);

/** Today's use of each add-on (its allowance is the listener's). */
export async function readUse() {
	const midnight = new Date();
	midnight.setHours(0, 0, 0, 0);
	try {
		prefs.used = await core.addonUse(midnight.getTime());
	} catch {
		/* the core is not answering: the lines stay as they were */
	}
}
