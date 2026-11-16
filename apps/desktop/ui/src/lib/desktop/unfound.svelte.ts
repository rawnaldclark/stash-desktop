// Songs no add-on could find: how many wait on Library › Not found, and every song known not to play (drawn dimmed
// wherever it is listed). Read again when a song could not be played or saved, and after the listener fixed one.
import { goto } from '$app/navigation';
import * as core from '../core';
import { LIB_REF } from '../identity';

/** Each opening is its own address, so asking again for the song already shown opens it again. */
let opened = 0;

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

	/** Library › Not found, opened on the song `id`: put back on the list first if it was set aside. */
	async show(id: number) {
		try {
			this.take(await core.setAside([id], false));
		} catch {
			/* it opens anyway */
		}
		await goto(`/library?tab=notfound&song=${id}&n=${++opened}`);
	}

	has(item: { refs?: Record<string, string> }) {
		const row = item.refs?.[LIB_REF];
		return row !== undefined && this.marked.has(Number(row));
	}
}

export const NF = new Unfound();
