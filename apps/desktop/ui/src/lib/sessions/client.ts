/**
 * The window's side of Sessions' station calls: the core's `/api/sessions/*` (stashd passes them to the share service and its
 * answers back as they are). Answers are checked here (rules.ts). A failure is a `SessionsProblem` whose code the page turns into
 * one line (`sessionsMessage`): the share service's own error codes are read the way the web player's proxy reads them. This
 * computer's Community key goes on every call that has one. The room itself is room.ts.
 */
import { KEY_HEADER } from '../community/key';
import { at } from '../core';
import {
	cleanFollowed,
	cleanFollowResult,
	cleanLiveSession,
	cleanOpenedRoom,
	cleanOwnStation,
	cleanStationPage,
	cleanUpcoming,
	cleanVoteResult,
	isStationId,
	type FollowedStation,
	type FollowResult,
	type LiveSession,
	type OpenedRoom,
	type OwnStation,
	type StationInput,
	type StationPage,
	type UpcomingStation,
	type Vote,
	type VoteResult
} from './rules';

export type SessionsCode =
	| 'off'
	| 'unavailable'
	| 'rate_limited'
	| 'signed_out'
	| 'bad_key'
	| 'bad_request'
	| 'forbidden'
	| 'gone'
	| 'not_yours'
	| 'own_station'
	| 'blocked'
	| 'listen_first'
	| 'have_station'
	| 'too_many';

export class SessionsProblem extends Error {
	constructor(
		readonly code: SessionsCode,
		readonly id: string | null = null
	) {
		super(code);
		this.name = 'SessionsProblem';
	}
}

/** The one line a refusal shows. */
export function sessionsMessage(e: unknown): string {
	switch (e instanceof SessionsProblem ? e.code : 'unavailable') {
		case 'off':
			return 'Sessions are coming soon.';
		case 'rate_limited':
			return 'Sessions are busy right now. Try again in a minute.';
		case 'bad_key':
			return 'Community didn’t accept this computer’s key.';
		case 'gone':
			return 'This station isn’t available.';
		case 'not_yours':
			return 'That station isn’t yours.';
		case 'own_station':
			return 'That’s your own station.';
		case 'blocked':
			return 'You can’t do that in Community.';
		case 'listen_first':
			return 'Listen for 2 minutes to vote.';
		case 'have_station':
			return 'This computer has a station already.';
		case 'too_many':
			return 'You follow as many stations as you can.';
		case 'unavailable':
			return 'Couldn’t reach Sessions. Try again.';
		default:
			return 'Something went wrong. Try again.';
	}
}

export type SessionsClient = ReturnType<typeof createSessionsClient>;

/**
 * A refusal as one of this page's codes: the core's own (`{ error: { code } }`), or the share service's (`{ error: "code" }`) read
 * the way the web player's proxy reads it (stash-player worker/sessions.ts).
 */
export function problemOf(status: number, body: unknown): { code: SessionsCode; id: string | null } {
	const e = body && typeof body === 'object' ? (body as { error?: unknown; id?: unknown }).error : undefined;
	const raw = typeof e === 'string' ? e : e && typeof e === 'object' && typeof (e as { code?: unknown }).code === 'string' ? (e as { code: string }).code : null;
	const idRaw = (body as { id?: unknown } | null)?.id ?? (e && typeof e === 'object' ? (e as { id?: unknown }).id : undefined);
	const id = isStationId(idRaw) ? (idRaw as string) : null;
	switch (status) {
		case 400:
		case 413:
		case 415:
			return { code: 'bad_request', id };
		case 401:
			return { code: 'bad_key', id };
		case 403:
			return { code: raw === 'not_yours' || raw === 'own_station' || raw === 'blocked' || raw === 'listen_first' ? raw : 'forbidden', id };
		case 404:
			return { code: 'gone', id };
		case 409:
			return { code: raw === 'have_station' || raw === 'too_many' ? raw : 'bad_request', id };
		case 429:
			return { code: 'rate_limited', id };
		case 503:
			return { code: raw === 'sessions_off' || raw === 'off' ? 'off' : 'unavailable', id };
		default:
			return { code: 'unavailable', id };
	}
}

export function createSessionsClient(fetchFn: typeof fetch = (i, o) => fetch(i, o), base = '') {
	async function call(path: string, key: string | null, init: { method?: string; body?: unknown } = {}): Promise<Response> {
		const headers: Record<string, string> = { Accept: 'application/json' };
		if (key) headers[KEY_HEADER] = key;
		if (init.body !== undefined) headers['Content-Type'] = 'application/json';
		let res: Response;
		try {
			res = await fetchFn(base ? `${base}/api/sessions${path}` : at(`/api/sessions${path}`), {
				method: init.method ?? 'GET',
				headers,
				body: init.body === undefined ? undefined : JSON.stringify(init.body),
				signal: AbortSignal.timeout(12_000)
			});
		} catch {
			throw new SessionsProblem('unavailable');
		}
		if (res.ok) return res;
		const j = (await res.json().catch(() => null)) as unknown;
		const p = problemOf(res.status, j);
		throw new SessionsProblem(p.code, p.id);
	}
	const body = async (res: Response): Promise<Record<string, unknown>> => {
		const j = (await res.json().catch(() => null)) as unknown;
		if (!j || typeof j !== 'object' || Array.isArray(j)) throw new SessionsProblem('unavailable');
		return j as Record<string, unknown>;
	};
	const list = <T>(v: unknown, clean: (x: unknown) => T | null): T[] => (Array.isArray(v) ? v.map(clean).filter((x): x is T => !!x) : []);
	const must = <T>(v: T | null): T => {
		if (!v) throw new SessionsProblem('unavailable');
		return v;
	};

	return {
		/** Is Sessions reachable from here, and where a room's socket goes (`wss://…/v1/rooms`). */
		async status(): Promise<{ on: boolean; rooms: string | null }> {
			const j = await body(await call('/status', null));
			const rooms = typeof j.rooms === 'string' && /^wss?:\/\//.test(j.rooms) ? j.rooms : null;
			return { on: j.on === true && !!rooms, rooms };
		},
		async live(key: string | null): Promise<LiveSession[]> {
			return list((await body(await call('/live', key))).sessions, cleanLiveSession);
		},
		async upcoming(key: string | null): Promise<UpcomingStation[]> {
			return list((await body(await call('/upcoming', key))).stations, cleanUpcoming);
		},
		async following(key: string): Promise<FollowedStation[]> {
			return list((await body(await call('/following', key))).stations, cleanFollowed);
		},
		async station(id: string, key: string | null): Promise<StationPage> {
			return must(cleanStationPage(await body(await call(`/stations/${id}`, key))));
		},
		async mine(key: string): Promise<OwnStation | null> {
			const j = await body(await call('/stations/mine', key));
			return j.station === null ? null : must(cleanOwnStation(j.station));
		},
		async create(key: string, input: StationInput): Promise<string> {
			const j = await body(await call('/stations', key, { method: 'POST', body: input }));
			if (!isStationId(j.id)) throw new SessionsProblem('unavailable');
			return j.id;
		},
		async edit(key: string, id: string, input: StationInput): Promise<OwnStation> {
			return must(cleanOwnStation((await body(await call(`/stations/${id}`, key, { method: 'PATCH', body: input }))).station));
		},
		async remove(key: string, id: string): Promise<void> {
			await call(`/stations/${id}`, key, { method: 'DELETE' });
		},
		async heard(key: string, id: string): Promise<void> {
			await call(`/stations/${id}/heard`, key, { method: 'PUT' });
		},
		async vote(key: string, id: string, value: Vote): Promise<VoteResult> {
			return must(cleanVoteResult(await body(await call(`/stations/${id}/vote`, key, { method: 'PUT', body: { value } }))));
		},
		async follow(key: string, id: string, on: boolean): Promise<FollowResult> {
			return must(cleanFollowResult(await body(await call(`/stations/${id}/follow`, key, { method: 'PUT', body: { on } }))));
		},
		async goLive(key: string, stationId: string, hostName?: string): Promise<OpenedRoom> {
			return must(cleanOpenedRoom(await body(await call('/rooms', key, { method: 'POST', body: { stationId, ...(hostName ? { hostName } : {}) } }))));
		}
	};
}
