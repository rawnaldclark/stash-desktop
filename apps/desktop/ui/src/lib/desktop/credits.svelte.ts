// The artists a song's credit names ("Drake & Future", "Newcomer feat. Drake"), each with their artist in the library
// when it knows them by themselves: the core splits a credit (see `crates/stash-core/src/credits.rs`), and this keeps
// its answers. A credit is asked once; the credits shown at one moment (a list's rows) are asked together.
import { SvelteMap } from 'svelte/reactivity';
import { library } from '../app';
import * as core from '../core';

/** Words that may join two artists. A credit without any names one artist, and is not asked about. */
const JOINS = /[,;&/]|\s(?:and|x|with|vs\.?|feat\.?|ft\.?|featuring)\s|\(\s*(?:feat|ft)/i;
const NONE: core.CreditPart[] = [];
/** The most credits kept; past it, they are let go and asked again when shown. */
const KEPT = 4000;
/** The most credits one ask carries (the core takes as many). */
const ASKED = 400;

const known = new SvelteMap<string, core.CreditPart[]>();
const wanted = new Set<string>();
let timer: ReturnType<typeof setTimeout> | null = null;

/** The artists `credit` names, two or more, each with their artist here when known by themselves; none for one artist,
 * or until the core has answered (what reads this then reads it again). */
export function creditOf(credit: string | undefined): core.CreditPart[] {
	if (!credit || !JOINS.test(credit)) return NONE;
	const parts = known.get(credit);
	if (parts) return parts;
	if (!wanted.has(credit)) {
		wanted.add(credit);
		timer ??= setTimeout(ask, 30);
	}
	return NONE;
}

/** Each part the library knows, as its link. */
export const knownParts = (credit: string | undefined) => creditOf(credit).filter((p): p is core.CreditPart & { id: number } => p.id !== undefined);

async function ask(): Promise<void> {
	const batch = [...wanted].slice(0, ASKED);
	try {
		const said = await core.credits(batch);
		if (known.size + batch.length > KEPT) known.clear();
		for (const credit of batch) {
			const parts = said.credits[credit] ?? NONE;
			known.set(credit, parts.length >= 2 ? parts : NONE);
		}
	} catch {
		// Not asked again until shown again after the library changes: one name, as before.
		for (const credit of batch) known.set(credit, NONE);
	} finally {
		for (const credit of batch) wanted.delete(credit);
		timer = wanted.size ? setTimeout(ask, 30) : null;
	}
}

// Who the library knows by themselves changes with its artists: what was said is let go, and asked again when shown.
let artists: number | null = null;
library.counts.subscribe((c) => {
	if (!c) return;
	if (artists !== null && c.artists !== artists) known.clear();
	artists = c.artists;
});
