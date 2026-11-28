/**
 * This computer's Community key: what a session knows it by (its station is this key's; votes count once per key). The core
 * keeps it (`GET /api/community`, `POST /api/community/join`); nothing is made or sent before Join. It is this computer's own,
 * not a phone's: a phone that joined Community has a key of its own.
 */
import { writable, type Readable } from 'svelte/store';
import { get, post } from '../core';

/** The header the share service reads the key from (as the web player and the app send it). */
export const KEY_HEADER = 'X-Stash-Community-Key';

/** Whether this computer has joined: null until the core has said. */
const joinedStore = writable<boolean | null>(null);
let key: string | null | undefined;
let asking: Promise<string | null> | null = null;

function read(): Promise<string | null> {
	asking ??= get<{ key: string | null }>('/api/community')
		.then((a) => {
			key = typeof a.key === 'string' ? a.key : null;
			joinedStore.set(!!key);
			return key;
		})
		.catch(() => null)
		.finally(() => (asking = null));
	return asking;
}

export const community = {
	joined: { subscribe: joinedStore.subscribe } as Readable<boolean | null>,
	/** The key, or null before Join (or when the core can't be reached). */
	async key(): Promise<string | null> {
		return key !== undefined ? key : read();
	},
	/** Join: the core makes this computer's key (once; never a second one). */
	async join(): Promise<string | null> {
		const a = await post<{ key: string }>('/api/community/join', {});
		key = typeof a.key === 'string' ? a.key : null;
		joinedStore.set(!!key);
		return key;
	}
};
