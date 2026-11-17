// YouTube Music in the window: the account the listener signed in with, and the home page YouTube Music makes (theirs
// when they signed in, else the one it shows anyone). The core asks YouTube; this keeps what it answered.

import * as core from '../core';
import { L, ui } from '../ui/state.svelte';

/** How long a home page that was read is shown before it is read again. */
const HOME_FOR_MS = 30 * 60 * 1000;

class Tube {
	/** The account, once the core said (null: not asked yet, or YouTube Music is off). */
	account = $state<core.TubeAccount | null>(null);
	home = $state<core.TubeShelf[] | null>(null);
	homeTrouble = $state<string | null>(null);
	homeLoading = $state(false);
	#homeAt = 0;
	/** Who the home page was read for: it is read again when someone else signs in. */
	#homeFor = '';

	/** YouTube Music is one of the listener's sources, and switched on. */
	get on(): boolean {
		return L.sources.some((a) => a.id === core.YOUTUBE && a.enabled);
	}

	get signedIn(): boolean {
		return !!this.account?.signed_in && !this.account.spent;
	}

	async refreshAccount(): Promise<core.TubeAccount | null> {
		if (!this.on) return (this.account = null);
		try {
			this.account = await core.tubeAccount();
		} catch {
			this.account = null;
		}
		return this.account;
	}

	/** Takes what the core answered to a sign-in or a sign-out. */
	took(account: core.TubeAccount): void {
		this.account = account;
		this.#homeAt = 0;
	}

	async loadHome(force = false): Promise<void> {
		if (!this.on || ui.offline || this.homeLoading) return;
		const who = this.signedIn ? (this.account?.account?.name ?? 'signed in') : '';
		if (!force && this.home && who === this.#homeFor && Date.now() - this.#homeAt < HOME_FOR_MS) return;
		this.homeLoading = true;
		try {
			this.home = await core.tubeHome();
			this.homeTrouble = null;
			this.#homeAt = Date.now();
			this.#homeFor = who;
		} catch (e) {
			this.homeTrouble = core.said(e);
		} finally {
			this.homeLoading = false;
		}
	}
}

export const T = new Tube();
