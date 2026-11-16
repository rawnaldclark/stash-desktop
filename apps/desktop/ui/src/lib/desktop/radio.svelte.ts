// Whether radio can be started: this build of Stash has similar music from Last.fm, and it is switched on (Settings ›
// Sound). Every place that offers a radio reads this; Settings switches it.
import * as core from '../core';

class Radio {
	/** This build knows Stash's Last.fm proxy. */
	available = $state(false);
	/** Similar music from Last.fm is on. */
	on = $state(false);

	/** A radio can be started. */
	get ready() {
		return this.available && this.on;
	}

	async refresh() {
		try {
			const s = await core.similarSettings();
			this.available = s.available;
			this.on = s.available && s.on;
		} catch {
			/* the core is not answering: as it was */
		}
	}

	/** Switches similar music on or off (off, what Last.fm said is deleted). */
	async set(on: boolean) {
		const s = await core.setSimilar(on);
		this.available = s.available;
		this.on = s.on;
	}
}

export const R = new Radio();
