import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { closeReason, roomUrl, RoomSocket, type Frame, type RoomStatus, type SocketLike } from './room';

class FakeSocket implements SocketLike {
	sent: Frame[] = [];
	closed: number | null = null;
	onopen: SocketLike['onopen'] = null;
	onmessage: SocketLike['onmessage'] = null;
	onclose: SocketLike['onclose'] = null;
	onerror: SocketLike['onerror'] = null;
	constructor(readonly url: string) {}
	send(d: string) {
		this.sent.push(JSON.parse(d));
	}
	close(code = 1000) {
		this.closed = code;
	}
	open() {
		this.onopen?.({});
	}
	recv(m: object) {
		this.onmessage?.({ data: JSON.stringify(m) });
	}
	drop(code = 1006) {
		this.onclose?.({ code });
	}
}

const ORIGIN = 'https://stashfm.app/v1/rooms';
let sockets: FakeSocket[];
let statuses: RoomStatus[];
let frames: Frame[];
const make = (hello = { name: 'Lis', communityKey: 'k'.repeat(43) }) =>
	new RoomSocket(ORIGIN, 'ABCDEFGH', hello, (f) => frames.push(f), (s) => statuses.push(s), {
		open: (url) => {
			const s = new FakeSocket(url);
			sockets.push(s);
			return s;
		}
	});
const welcome = { t: 'welcome', memberId: 'MEMBER22', token: 'tok-1', state: {} };

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(1_790_000_000_000);
	sockets = [];
	statuses = [];
	frames = [];
});
afterEach(() => vi.useRealTimers());

describe('the room socket', () => {
	it("is the player's own origin, wss on https, ?r=1 only when coming back", () => {
		expect(roomUrl(ORIGIN, 'ABCDEFGH', false)).toBe('wss://stashfm.app/v1/rooms/ABCDEFGH/ws');
		expect(roomUrl('ws://127.0.0.1:8811/v1/rooms/', 'ABCDEFGH', true)).toBe('ws://127.0.0.1:8811/v1/rooms/ABCDEFGH/ws?r=1');
		expect(roomUrl('http://127.0.0.1:8811/v1/rooms', 'ABCDEFGH', false)).toBe('ws://127.0.0.1:8811/v1/rooms/ABCDEFGH/ws');
	});

	it('says hello v3 with the Community key (never both keys), then passes every frame on', () => {
		const r = make({ name: 'Lis', communityKey: 'k'.repeat(43), hostKey: 'h'.repeat(43) } as never);
		r.connect();
		sockets[0]!.open();
		expect(sockets[0]!.sent[0]).toEqual({ t: 'hello', v: 3, name: 'Lis', hostKey: 'h'.repeat(43) });
		sockets[0]!.recv(welcome);
		sockets[0]!.recv({ t: 'reaction', from: 'X', emoji: 'k:shrug' });
		expect(frames.map((f) => f.t)).toEqual(['welcome', 'reaction']);
		expect(statuses.at(-1)).toEqual({ kind: 'open', memberId: 'MEMBER22' });
		expect(r.send({ t: 'react', emoji: 'k:yay' })).toBe(true);
	});

	it('nothing is sent before the welcome', () => {
		const r = make();
		r.connect();
		sockets[0]!.open();
		expect(r.send({ t: 'react', emoji: 'k:yay' })).toBe(false);
	});

	it("keeps the room's clock from the quickest ping", () => {
		const r = make();
		r.connect();
		sockets[0]!.open();
		sockets[0]!.recv(welcome);
		const ping = sockets[0]!.sent.find((f) => f.t === 'ping')!;
		vi.advanceTimersByTime(100); // a 100 ms trip; the room read its clock half-way, 5 s ahead of this tab
		sockets[0]!.recv({ t: 'pong', c: ping.c, r: (ping.c as number) + 50 + 5_000 });
		expect(r.synced).toBe(true);
		expect(r.roomNow() - Date.now()).toBe(5_000);
		// A slower trip doesn't replace it.
		sockets[0]!.recv({ t: 'pong', c: Date.now() - 2_000, r: Date.now() + 9_000 });
		expect(r.roomNow() - Date.now()).toBe(5_000);
	});

	it('comes back after a drop with its token and ?r=1, and stops trying once the slot is gone', () => {
		const r = make();
		r.connect();
		sockets[0]!.open();
		sockets[0]!.recv(welcome);
		sockets[0]!.drop();
		expect(statuses.at(-1)).toEqual({ kind: 'retrying', attempt: 1 });
		vi.advanceTimersByTime(1_000);
		expect(sockets[1]!.url).toMatch(/\?r=1$/);
		sockets[1]!.open();
		expect(sockets[1]!.sent[0]).toMatchObject({ t: 'hello', resumeToken: 'tok-1' });
		// Now it never gets back.
		const tried = sockets.length;
		for (let i = 0; i < 20 && statuses.at(-1)?.kind !== 'closed'; i++) {
			sockets.at(-1)!.drop();
			vi.advanceTimersByTime(10_000);
		}
		expect(statuses.at(-1)).toEqual({ kind: 'closed', reason: 'lost' });
		expect(sockets.length - tried).toBeLessThanOrEqual(8); // about a minute of tries, then no more
		vi.advanceTimersByTime(60_000);
		expect(statuses.at(-1)).toEqual({ kind: 'closed', reason: 'lost' });
	});

	it('an ending or a refusal is final; Leave lets the slot go', () => {
		expect([4000, 4426, 4401, 4403, 4409, 1006].map(closeReason)).toEqual(['ended', 'update', 'key', 'booted', 'full', null]);
		const r = make();
		r.connect();
		sockets[0]!.open();
		sockets[0]!.drop(4409);
		vi.advanceTimersByTime(30_000);
		expect(sockets).toHaveLength(1);
		expect(statuses.at(-1)).toEqual({ kind: 'closed', reason: 'full' });

		const l = make();
		l.connect();
		sockets[1]!.open();
		sockets[1]!.recv(welcome);
		sockets[1]!.recv({ t: 'ended', reason: 'host_ended' });
		expect(frames.at(-1)).toEqual({ t: 'ended', reason: 'host_ended' });
		expect(statuses.at(-1)).toEqual({ kind: 'closed', reason: 'ended' });

		const m = make();
		m.connect();
		sockets[2]!.open();
		sockets[2]!.recv(welcome);
		m.leave();
		expect(sockets[2]!.closed).toBe(1000);
		expect(statuses.at(-1)).toEqual({ kind: 'closed', reason: 'left' });
	});
});
