// The artists a song's credit names ("Drake & Future", "Newcomer feat. Drake"), each with their artist in the library
// when it knows them by themselves: the core splits a credit (see `crates/stash-core/src/credits.rs`), and this keeps
// its answers. A credit is asked once; the credits shown at one moment (a list's rows) are asked together.
import { SvelteMap } from 'svelte/reactivity';
import { library } from '../app';
import * as core from '../core';

/** Words that may join two artists. A credit without any names one artist, and is not asked about. */
const JOINS = /[,;&/]|\s(?:and|x|with|vs\.?|feat\.?|ft\.?|featuring)\s|\(\s*(?:feat|ft)/i;
const NONE: core.CreditPart[] = [];
/** The most credits one ask carries (the core takes as many). */
const ASKED = 400;
/** How long after an ask that failed it is made again. */
const AGAIN_MS = 5000;

// What was said is kept until the library changes: as many credits as were shown, and no more than its songs have
// (plus the add-ons' songs looked at), so nothing is let go while it is shown.
const known = new SvelteMap<string, core.CreditPart[]>();
/** To be asked, and being asked. */
const wanted = new Set<string>();
const asking = new Set<string>();
let timer: ReturnType<typeof setTimeout> | null = null;
/** Goes up when the library changes: answers asked for before are let go. */
let generation = 0;

/** The artists `credit` names, two or more, each with their artist here when known by themselves; none for one artist,
 * or until the core has answered (what reads this then reads it again). */
export function creditOf(credit: string | undefined): core.CreditPart[] {
	if (!credit || !JOINS.test(credit)) return NONE;
	const parts = known.get(credit);
	if (parts) return parts;
	if (!wanted.has(credit) && !asking.has(credit)) {
		wanted.add(credit);
		timer ??= setTimeout(ask, 30);
	}
	return NONE;
}

/** Each part the library knows, as its link. */
export const knownParts = (credit: string | undefined) => creditOf(credit).filter((p): p is core.CreditPart & { id: number } => p.id !== undefined);

async function ask(): Promise<void> {
	timer = null;
	const batch = [...wanted].slice(0, ASKED);
	for (const credit of batch) {
		wanted.delete(credit);
		asking.add(credit);
	}
	const asked = generation;
	let said: Awaited<ReturnType<typeof core.credits>> | null = null;
	try {
		said = await core.credits(batch);
	} catch {
		said = null;
	}
	for (const credit of batch) asking.delete(credit);
	if (said && asked === generation) {
		for (const credit of batch) {
			const parts = said.credits[credit] ?? NONE;
			known.set(credit, parts.length >= 2 ? parts : NONE);
		}
	} else {
		// The library changed while it was asked (what was said may be out of date), or the ask failed: asked again.
		for (const credit of batch) wanted.add(credit);
	}
	if (wanted.size) {
		// After a failure, whatever was wanted meanwhile waits too: the core just failed.
		if (!said && timer) {
			clearTimeout(timer);
			timer = null;
		}
		timer ??= setTimeout(ask, said ? 30 : AGAIN_MS);
	}
}

/** Lets go of what was said, so it is asked again when shown: who the library knows by themselves may have changed (its
 * songs were read again, which can rename artists however many songs there are). */
export function forgetCredits(): void {
	generation += 1;
	known.clear();
}

// Who the library knows by themselves changes with its songs, albums and artists.
let seen: string | null = null;
library.counts.subscribe((c) => {
	if (!c) return;
	const now = `${c.tracks}/${c.albums}/${c.artists}`;
	if (seen !== null && now !== seen) forgetCredits();
	seen = now;
});
