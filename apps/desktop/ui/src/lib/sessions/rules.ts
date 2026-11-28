/**
 * Stash Sessions' data rules (live listening: a host plays on a station, others tune in), in one dependency-free file that both the
 * player Worker's `/api/sessions` proxy (worker/sessions.ts) and the browser use. Mirrors the share Worker's stations (MP3APK
 * `infra/share-worker/src/stations.js` `summaryOfStation`, `stations-api.js` `ownView` and `liveOf`, and its README's "Sessions"): the
 * same field names and limits. Keep them in step with those files.
 *
 * A station belongs to one Community key (this browser's, lib/community/store.ts); going live opens a public room for it.
 */
import { cleanWireTrack, isEditKey, type WireTrack } from '../share/rules.ts';

export type Vote = -1 | 0 | 1;
export type AutoLive = 'off' | 'schedule' | 'always';

/** A station as others see it. With this browser's key the service adds how it stands with it. */
export type StationSummary = {
	id: string;
	name: string;
	hostName: string | null;
	vibe: string | null;
	up: number;
	down: number;
	followers: number;
	myVote?: Vote;
	mine?: boolean;
	heard?: boolean;
	following?: boolean;
};

/** Hours on the air, on the host's wall clock in `tz`. Day 1 is Monday; an end before its start runs past midnight. */
export type Schedule = { tz: string; windows: { day: number; start: string; end: string }[] };
/** The window in progress, or else the next one, in ms. */
export type Window = { start: number; end: number };

/** The owner's own view: everything others see, plus its settings and its standing. */
export type OwnStation = StationSummary & {
	autoLive: AutoLive;
	schedule: Schedule | null;
	hidden: boolean;
	removed: boolean;
	lastLiveAt: number | null;
	deleteAt: number;
};

/** Up to 4 faces of a session's crowd, host first: an initial (null without a name) and the member id. */
export type CrowdFace = { i: string | null; id: string };

/** A session on the air: the room's code to join, its song, how many are listening. `full` at 50. */
export type Live = { room: string; track: WireTrack | null; listeners: number; paused: boolean; full: boolean; crowd?: CrowdFace[] };

/** One row of Live now. */
export type LiveSession = Live & { station: StationSummary; crowd: CrowdFace[]; startedAt: number };

/** A station's page: its summary (plus its hours when it runs on a schedule) and its session while it's on the air. */
export type StationPage = { station: StationSummary & { schedule?: Schedule | null; next?: Window | null }; live: Live | null };

export type FollowedStation = StationSummary & { live?: Live; next?: Window };
export type UpcomingStation = StationSummary & { next: Window };
export type VoteResult = { up: number; down: number; myVote: Vote };
export type FollowResult = { following: boolean; followers: number };
/** A session just opened: its room's code and the key only the host's tab holds. */
export type OpenedRoom = { code: string; hostKey: string };

/** What a browser may set on its station. `null` (or blank) clears hostName and vibe, `null` clears the schedule. */
export type StationInput = { name?: string; hostName?: string | null; vibe?: string | null; autoLive?: AutoLive; schedule?: Schedule | null };

/** Lengths in UTF-16 units after trimming, as the service counts them. */
export const MAX_STATION_NAME = 40;
export const MAX_HOST_NAME = 40;
export const MAX_VIBE = 60;
export const MAX_WINDOWS = 7;
/** A session holds the host, 3 co-hosts and 50 listeners. */
export const MAX_LISTENERS = 50;
/** How many rows Live now and Upcoming ask for (the service's cap is 50). */
export const LIVE_LIMIT = 50;
export const UPCOMING_LIMIT = 20;
/** A sanity bound on a count. */
const MAX_COUNT = 1_000_000_000;

const AUTO: readonly AutoLive[] = ['off', 'schedule', 'always'];
// Control characters and bidi overrides never reach the page (the same rule as share/rules.ts).
const BAD = /[\p{Cc}\u202A-\u202E\u2066-\u2069]/gu;
const HHMM = /^([01]\d|2[0-3]):[0-5]\d$/;

/** Station ids are the service's 8 letters or digits. */
export const isStationId = (v: unknown): v is string => typeof v === 'string' && /^[A-Za-z0-9]{8}$/.test(v);
/** A room's code: 8 of the share Worker's room letters (no I, O, 0 or 1). */
export const isRoomCode = (v: unknown): v is string => typeof v === 'string' && /^[A-HJ-NP-Z2-9]{8}$/.test(v);
/** A member's id inside a room: the same alphabet as a room's code. */
export const isMemberId = (v: unknown): v is string => isRoomCode(v);
/** A host key: base64url of 32 random bytes, 43 characters, like a Community key. */
export const isHostKey = (v: unknown): v is string => isEditKey(v);
export const isVote = (v: unknown): v is Vote => v === -1 || v === 0 || v === 1;

function text(v: unknown, max: number): string | null {
	if (typeof v !== 'string') return null;
	const s = v.replace(BAD, '').trim();
	return s && s.length <= max ? s : null;
}
const count = (v: unknown, max = MAX_COUNT): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= max;
const time = (v: unknown): v is number => count(v, Number.MAX_SAFE_INTEGER) && v > 0;
const obj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

/** A station summary as the service sends it, reduced to known fields and checked; null when it is not one. */
export function cleanStation(v: unknown): StationSummary | null {
	if (!obj(v) || !isStationId(v.id)) return null;
	const name = text(v.name, MAX_STATION_NAME);
	if (!name || !count(v.up) || !count(v.down) || !count(v.followers)) return null;
	const out: StationSummary = {
		id: v.id,
		name,
		hostName: text(v.hostName, MAX_HOST_NAME),
		vibe: text(v.vibe, MAX_VIBE),
		up: v.up,
		down: v.down,
		followers: v.followers
	};
	if (isVote(v.myVote)) out.myVote = v.myVote;
	if (typeof v.mine === 'boolean') out.mine = v.mine;
	if (typeof v.heard === 'boolean') out.heard = v.heard;
	if (typeof v.following === 'boolean') out.following = v.following;
	return out;
}

/** A schedule, checked as the service checks one; null when it is not one. */
export function cleanSchedule(v: unknown): Schedule | null {
	if (!obj(v) || typeof v.tz !== 'string' || !/^[A-Za-z0-9_+\-/]{1,64}$/.test(v.tz) || !Array.isArray(v.windows)) return null;
	if (v.windows.length < 1 || v.windows.length > MAX_WINDOWS) return null;
	const windows: Schedule['windows'] = [];
	for (const w of v.windows) {
		if (!obj(w) || !Number.isInteger(w.day) || (w.day as number) < 1 || (w.day as number) > 7) return null;
		if (typeof w.start !== 'string' || typeof w.end !== 'string' || !HHMM.test(w.start) || !HHMM.test(w.end) || w.start === w.end) return null;
		windows.push({ day: w.day as number, start: w.start, end: w.end });
	}
	return { tz: v.tz, windows };
}

export function cleanWindow(v: unknown): Window | null {
	return obj(v) && time(v.start) && time(v.end) && v.end > v.start ? { start: v.start, end: v.end } : null;
}

/** The owner's view of their station; null when it is not one. */
export function cleanOwnStation(v: unknown): OwnStation | null {
	const s = cleanStation(v);
	if (!s || !obj(v) || !AUTO.includes(v.autoLive as AutoLive) || typeof v.hidden !== 'boolean' || typeof v.removed !== 'boolean') return null;
	if (!time(v.deleteAt) || !(v.lastLiveAt === null || time(v.lastLiveAt))) return null;
	return {
		...s,
		autoLive: v.autoLive as AutoLive,
		schedule: v.schedule === null ? null : cleanSchedule(v.schedule),
		hidden: v.hidden,
		removed: v.removed,
		lastLiveAt: (v.lastLiveAt as number | null) ?? null,
		deleteAt: v.deleteAt
	};
}

function cleanCrowd(v: unknown): CrowdFace[] {
	if (!Array.isArray(v)) return [];
	const out: CrowdFace[] = [];
	for (const f of v.slice(0, 4)) {
		if (!obj(f) || !isMemberId(f.id)) continue;
		const i = typeof f.i === 'string' ? text(f.i, 2) : null;
		out.push({ i, id: f.id });
	}
	return out;
}

/** A session on the air; null when it is not one. */
export function cleanLive(v: unknown): Live | null {
	if (!obj(v) || !isRoomCode(v.room) || !count(v.listeners, MAX_LISTENERS + 4) || typeof v.paused !== 'boolean' || typeof v.full !== 'boolean') return null;
	const out: Live = { room: v.room, track: v.track ? cleanWireTrack(v.track, true) : null, listeners: v.listeners, paused: v.paused, full: v.full };
	if (Array.isArray(v.crowd)) out.crowd = cleanCrowd(v.crowd);
	return out;
}

/** One row of Live now; null when it is not one (the row is left out). */
export function cleanLiveSession(v: unknown): LiveSession | null {
	const live = cleanLive(v);
	const station = obj(v) ? cleanStation(v.station) : null;
	if (!live || !station || !obj(v) || !time(v.startedAt)) return null;
	return { ...live, station, crowd: cleanCrowd(v.crowd), startedAt: v.startedAt };
}

/** A station's page; null when it is not one. */
export function cleanStationPage(v: unknown): StationPage | null {
	if (!obj(v)) return null;
	const s = cleanStation(v.station);
	if (!s) return null;
	const raw = v.station as Record<string, unknown>;
	const station: StationPage['station'] = { ...s };
	if ('schedule' in raw) station.schedule = raw.schedule === null ? null : cleanSchedule(raw.schedule);
	if ('next' in raw) station.next = cleanWindow(raw.next);
	return { station, live: v.live ? cleanLive(v.live) : null };
}

export function cleanFollowed(v: unknown): FollowedStation | null {
	const s = cleanStation(v);
	if (!s || !obj(v)) return null;
	const out: FollowedStation = { ...s };
	const live = v.live ? cleanLive(v.live) : null;
	if (live) out.live = live;
	const next = v.next ? cleanWindow(v.next) : null;
	if (next) out.next = next;
	return out;
}

export function cleanUpcoming(v: unknown): UpcomingStation | null {
	const s = cleanStation(v);
	const next = obj(v) ? cleanWindow(v.next) : null;
	return s && next ? { ...s, next } : null;
}

export function cleanVoteResult(v: unknown): VoteResult | null {
	return obj(v) && count(v.up) && count(v.down) && isVote(v.myVote) ? { up: v.up, down: v.down, myVote: v.myVote } : null;
}

export function cleanFollowResult(v: unknown): FollowResult | null {
	return obj(v) && typeof v.following === 'boolean' && count(v.followers) ? { following: v.following, followers: v.followers } : null;
}

export function cleanOpenedRoom(v: unknown): OpenedRoom | null {
	return obj(v) && isRoomCode(v.code) && isHostKey(v.hostKey) ? { code: v.code, hostKey: v.hostKey } : null;
}

/**
 * A station change from the browser, rebuilt from known fields and checked as the service will: the cleaned body, or null when any
 * field is wrong (the service refuses the whole change then too). [create]: a name is required.
 */
export function cleanStationInput(v: unknown, create: boolean): StationInput | null {
	if (!obj(v)) return null;
	const out: StationInput = {};
	if ('name' in v) {
		const name = text(v.name, MAX_STATION_NAME);
		if (!name) return null;
		out.name = name;
	} else if (create) return null;
	for (const [k, max] of [['hostName', MAX_HOST_NAME], ['vibe', MAX_VIBE]] as const) {
		if (!(k in v)) continue;
		if (v[k] === null || (typeof v[k] === 'string' && !(v[k] as string).trim())) out[k] = null;
		else {
			const t = text(v[k], max);
			if (!t) return null;
			out[k] = t;
		}
	}
	if ('autoLive' in v) {
		if (!AUTO.includes(v.autoLive as AutoLive)) return null;
		out.autoLive = v.autoLive as AutoLive;
	}
	if ('schedule' in v) {
		if (v.schedule === null) out.schedule = null;
		else {
			const s = cleanSchedule(v.schedule);
			if (!s) return null;
			out.schedule = s;
		}
	}
	return Object.keys(out).length ? out : null;
}
