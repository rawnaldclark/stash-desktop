/**
 * A session's room, from this tab: its socket (through the player's own origin, /api/sessions/rooms/{code}/ws, worker/sessions.ts),
 * the hello, coming back after a drop, and the room's clock. The room's messages are the share Worker's (MP3APK
 * infra/share-worker README, "Sessions protocol"); this file only carries them, follow.ts and session.svelte.ts act on them.
 *
 * - hello v3, with this browser's Community key (a listener) or the host key (the host's tab). The room keeps only the key's hash.
 * - A drop (not an ending) comes back with the welcome's token as `resumeToken` and `?r=1`, so a full room still lets this member
 *   back into the slot it holds for 60 s. Backoff 1, 2, 4, 8 s, then every 10 s, for as long as the slot can be held.
 * - The room's clock: every ping carries this tab's time, the pong the room's; the sample with the shortest round trip wins
 *   (the half-way point of that trip is when the room read its clock).
 * - The endings (`ended`, then close 4000) and the refusals at hello (4426 update, 4401 key, 4403 booted, 4409 full) are final.
 */

/** What this tab sends first. */
export type Hello = { name?: string; communityKey?: string; hostKey?: string };

export type RoomStatus =
	| { kind: 'connecting' }
	| { kind: 'open'; memberId: string }
	| { kind: 'retrying'; attempt: number }
	| { kind: 'closed'; reason: CloseReason };

/** Why this tab is out of the room for good. */
export type CloseReason = 'ended' | 'update' | 'key' | 'booted' | 'full' | 'lost' | 'left';

export type Frame = { t: string } & Record<string, unknown>;

/** The browser's WebSocket, as far as this needs it (tests hand in their own). */
export type SocketLike = {
	send(data: string): void;
	close(code?: number, reason?: string): void;
	onopen: ((ev: unknown) => void) | null;
	onmessage: ((ev: { data: unknown }) => void) | null;
	onclose: ((ev: { code: number; reason?: string }) => void) | null;
	onerror: ((ev: unknown) => void) | null;
};

export type RoomDeps = {
	open: (url: string) => SocketLike;
	now?: () => number;
	setTimeout?: (fn: () => void, ms: number) => unknown;
	clearTimeout?: (h: unknown) => void;
	setInterval?: (fn: () => void, ms: number) => unknown;
	clearInterval?: (h: unknown) => void;
};

const PING_MS = 15_000;
/** Quick pings after the welcome, for a good clock sample before the first song starts. */
const FIRST_PINGS = [250, 900, 2_000];
/** The room holds a dropped member's slot 60 s; past that a resume would be a new member anyway. */
const RETRY_FOR_MS = 60_000;
const BACKOFF = [1_000, 2_000, 4_000, 8_000];
const BACKOFF_MAX = 10_000;
const CLOCK_SAMPLES = 8;

/** The room's socket URL on this page's origin. */
export function roomUrl(rooms: string, code: string, resume: boolean): string {
	// Desktop: straight to the share service, as a phone connects (the core says where: `rooms`, e.g. wss://stashfm.app/v1/rooms).
	const u = new URL(`${rooms.replace(/\/+$/, '')}/${code}/ws`);
	if (u.protocol === 'https:') u.protocol = 'wss:';
	else if (u.protocol === 'http:') u.protocol = 'ws:';
	if (resume) u.search = '?r=1';
	return u.toString();
}

/** Close codes the room ends a socket with on purpose, and what each means here. */
export function closeReason(code: number): CloseReason | null {
	switch (code) {
		case 4000:
			return 'ended';
		case 4426:
			return 'update';
		case 4401:
			return 'key';
		case 4403:
			return 'booted';
		case 4409:
			return 'full';
		default:
			return null;
	}
}

export class RoomSocket {
	private ws: SocketLike | null = null;
	private memberId: string | null = null;
	private token: string | null = null;
	private attempt = 0;
	private droppedAt: number | null = null;
	private retryTimer: unknown = null;
	private pingTimer: unknown = null;
	private firstPings: unknown[] = [];
	private done = false;
	/** Clock samples: [round trip, room minus this tab]. */
	private samples: [number, number][] = [];
	private offset: number | null = null;

	private readonly now: () => number;
	private readonly later: (fn: () => void, ms: number) => unknown;
	private readonly cancel: (h: unknown) => void;
	private readonly every: (fn: () => void, ms: number) => unknown;
	private readonly stopEvery: (h: unknown) => void;

	constructor(
		private readonly origin: string,
		readonly code: string,
		private readonly hello: Hello,
		private readonly onFrame: (m: Frame) => void,
		private readonly onStatus: (s: RoomStatus) => void,
		private readonly deps: RoomDeps
	) {
		this.now = deps.now ?? Date.now;
		this.later = deps.setTimeout ?? ((fn, ms) => setTimeout(fn, ms));
		this.cancel = deps.clearTimeout ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
		this.every = deps.setInterval ?? ((fn, ms) => setInterval(fn, ms));
		this.stopEvery = deps.clearInterval ?? ((h) => clearInterval(h as ReturnType<typeof setInterval>));
	}

	/** Opens the socket (once: a drop reconnects by itself). */
	connect(): void {
		if (this.done || this.ws) return;
		this.onStatus(this.attempt ? { kind: 'retrying', attempt: this.attempt } : { kind: 'connecting' });
		let ws: SocketLike;
		try {
			ws = this.deps.open(roomUrl(this.origin, this.code, !!this.token));
		} catch {
			return this.dropped();
		}
		this.ws = ws;
		ws.onopen = () => {
			const hello: Frame = { t: 'hello', v: 3 };
			if (this.hello.name) hello.name = this.hello.name;
			if (this.hello.hostKey) hello.hostKey = this.hello.hostKey;
			else if (this.hello.communityKey) hello.communityKey = this.hello.communityKey;
			if (this.token) hello.resumeToken = this.token;
			this.sendNow(hello);
		};
		ws.onmessage = (ev) => this.received(ev.data);
		ws.onclose = (ev) => {
			if (this.ws !== ws) return;
			this.ws = null;
			this.stopPings();
			const reason = closeReason(ev.code);
			if (reason) return this.finish(reason);
			this.dropped();
		};
		ws.onerror = () => {}; // a close follows
	}

	/** Sends a message to the room; false when the socket isn't open (the caller says so, nothing is queued). */
	send(m: Frame): boolean {
		if (!this.ws || !this.memberId) return false;
		return this.sendNow(m);
	}

	/** Leaves the room (the tab's own Leave): the slot is let go and nothing reconnects. */
	leave(): void {
		const ws = this.ws;
		this.ws = null;
		this.stopPings();
		try {
			ws?.close(1000, 'left');
		} catch {
			/* gone already */
		}
		this.finish('left');
	}

	/** The room's time now, by this tab's clock and the best sample (this tab's own time until there is one). */
	roomNow(): number {
		return this.now() + (this.offset ?? 0);
	}

	/** Whether the room's clock is known yet. */
	get synced(): boolean {
		return this.offset !== null;
	}

	get me(): string | null {
		return this.memberId;
	}

	private sendNow(m: Frame): boolean {
		try {
			this.ws?.send(JSON.stringify(m));
			return true;
		} catch {
			return false;
		}
	}

	private received(data: unknown): void {
		if (typeof data !== 'string' || data.length > 1_000_000) return;
		let m: unknown;
		try {
			m = JSON.parse(data);
		} catch {
			return;
		}
		if (!m || typeof m !== 'object' || Array.isArray(m) || typeof (m as Frame).t !== 'string') return;
		const f = m as Frame;
		if (f.t === 'pong') return this.pong(f);
		if (f.t === 'welcome') {
			if (typeof f.memberId !== 'string' || typeof f.token !== 'string') return;
			this.memberId = f.memberId;
			this.token = f.token;
			this.attempt = 0;
			this.droppedAt = null;
			this.startPings();
			this.onStatus({ kind: 'open', memberId: f.memberId });
		}
		if (f.t === 'ended') {
			this.onFrame(f);
			return this.finish('ended');
		}
		this.onFrame(f);
	}

	private pong(f: Frame): void {
		const c = f.c;
		const r = f.r;
		if (typeof c !== 'number' || typeof r !== 'number' || !Number.isFinite(c) || !Number.isFinite(r)) return;
		const back = this.now();
		const rtt = back - c;
		if (rtt < 0 || rtt > 30_000) return;
		this.samples.push([rtt, r - (c + rtt / 2)]);
		if (this.samples.length > CLOCK_SAMPLES) this.samples.shift();
		let best = this.samples[0]!;
		for (const s of this.samples) if (s[0] < best[0]) best = s;
		this.offset = best[1];
	}

	private startPings(): void {
		this.stopPings();
		const ping = () => this.sendNow({ t: 'ping', c: this.now() });
		ping();
		this.firstPings = FIRST_PINGS.map((ms) => this.later(ping, ms));
		this.pingTimer = this.every(ping, PING_MS);
	}

	private stopPings(): void {
		if (this.pingTimer !== null) this.stopEvery(this.pingTimer);
		this.pingTimer = null;
		for (const h of this.firstPings) this.cancel(h);
		this.firstPings = [];
	}

	/** A drop, not an ending: back in a moment, while the room still holds the slot. */
	private dropped(): void {
		if (this.done) return;
		const now = this.now();
		this.droppedAt ??= now;
		if (now - this.droppedAt > RETRY_FOR_MS) return this.finish('lost');
		const wait = BACKOFF[this.attempt] ?? BACKOFF_MAX;
		this.attempt++;
		this.onStatus({ kind: 'retrying', attempt: this.attempt });
		this.retryTimer = this.later(() => {
			this.retryTimer = null;
			this.connect();
		}, wait);
	}

	private finish(reason: CloseReason): void {
		if (this.done) return;
		this.done = true;
		if (this.retryTimer !== null) this.cancel(this.retryTimer);
		this.retryTimer = null;
		this.stopPings();
		this.onStatus({ kind: 'closed', reason });
	}
}
