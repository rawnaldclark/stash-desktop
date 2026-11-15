// Songs no add-on could find: how many wait on Library › Not found, and every song known not to play (drawn dimmed
// wherever it is listed). Read again when a song could not be played or saved, and after the listener fixed one.
import * as core from '../core';
import { LIB_REF } from '../identity';

class Unfound {
	/** How many are on the list (not set aside). */
	open = $state(0);
	/** The library rows known not to play: on the list or set aside. */
	marked = $state(new Set<number>());

	take(u: { open: number; marked: number[] }) {
		this.open = u.open;
		this.marked = new Set(u.marked);
	}

	async refresh() {
		try {
			this.take(await core.unfound(0, 0));
		} catch {
			/* the core is not answering: the marks stay as they were */
		}
	}

	has(item: { refs?: Record<string, string> }) {
		const row = item.refs?.[LIB_REF];
		return row !== undefined && this.marked.has(Number(row));
	}
}

export const NF = new Unfound();
