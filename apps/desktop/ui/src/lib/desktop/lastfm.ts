/**
 * Last.fm for scrobbling (the core's `scrobbling.rs` and `lastfm.rs`): whether this build has it, and signing in on
 * Last.fm's own page, where the listener allows Stash. The core keeps what Last.fm gives it and never hands it back.
 */
import { get, post, type ScrobbleTarget } from '../core';

/** Whether this build of Stash has Last.fm. */
export const lastfmAvailable = () => get<{ available: boolean }>('/api/scrobbling/lastfm');

/** Begins a sign-in: Last.fm's page, and whether the core opened it in the browser (in the window it does; in a page
 *  served to a browser, the page opens it). */
export const startLastfm = () => post<{ url: string; opened: boolean }>('/api/scrobbling/lastfm/start', {});

/** Whether the listener allowed Stash on Last.fm's page: not yet, or the services with Last.fm kept. A sign-in that is
 *  over (or none) is refused with 409, "Sign in again". */
export type LastfmFinish = { waiting: true } | { targets: ScrobbleTarget[]; id: number };
export const finishLastfm = () => post<LastfmFinish>('/api/scrobbling/lastfm/finish', {});
