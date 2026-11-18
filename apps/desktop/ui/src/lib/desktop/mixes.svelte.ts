/**
 * Stash's mixes (the core's `discover.rs`): Daily Discover, and Recommended by Last.fm with a Last.fm account. Forty
 * YouTube Music songs each, new every day; Home shows them first, as Android's Discover does.
 */
import { get, post, type TubeSong } from '../core';

export type Mix = { id: 'daily' | 'lastfm'; title: string; description: string; made_ms: number | null; songs: TubeSong[] };

export const M = $state<{ list: Mix[] }>({ list: [] });

/** The mixes on offer, as they stand. */
export async function loadMixes(): Promise<void> {
	const r = await get<{ mixes: Mix[] }>('/api/discover');
	M.list = r.mixes;
}

/** One mix with its songs of the day. */
export const mixOf = (id: string) => get<Mix>(`/api/discover/${encodeURIComponent(id)}`);

/** Another draw of the mix now (new finds are looked for when the last look is an hour old). */
export async function refreshMix(id: string): Promise<Mix> {
	const mix = await post<Mix>(`/api/discover/${encodeURIComponent(id)}/refresh`, {});
	M.list = M.list.map((m) => (m.id === mix.id ? mix : m));
	return mix;
}
