import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LibraryItem } from '../identity';
import { Follower, roomPosition, type FollowPlayer } from './follow';

/** A player that keeps its own position as a clock does, and logs what it was asked. */
class FakePlayer implements FollowPlayer {
	log: string[] = [];
	playing = false;
	held = false;
	key: string | null = null;
	/** Loads that fail (the song isn't found), counted down. */
	fail = 0;
	private pos = 0;
	private at = 0;
	async followLoad(item: LibraryItem, positionMs: number, autoplay: boolean) {
		if (this.fail > 0) {
			this.fail--;
			this.log.push(`load ${item.title} failed`);
			throw new Error('not found');
		}
		this.log.push(`load ${item.title} @${positionMs} ${autoplay ? 'play' : 'paused'}`);
		this.key = item.key;
		this.pos = positionMs;
		this.at = Date.now();
		this.playing = autoplay && !this.held;
	}
	followSeek(ms: number) {
		this.log.push(`seek ${Math.round(ms)}`);
		this.pos = ms;
		this.at = Date.now();
	}
	followPlay() {
		if (this.held || this.playing) return;
		this.log.push('play');
		this.pos = this.position();
		this.at = Date.now();
		this.playing = true;
	}
	followPause() {
		if (!this.playing) return;
		this.log.push('pause');
		this.pos = this.position();
		this.playing = false;
	}
	position() {
		return this.playing ? this.pos + (Date.now() - this.at) : this.pos;
	}
	view() {
		return { playing: this.playing, buffering: false, listenHeld: this.held, currentKey: this.key };
	}
}

const T0 = 1_790_000_000_000;
let p: FakePlayer;
let f: Follower;
const SONG = { t: 'Pink + White', a: 'Frank Ocean', yt: 'uzS3WG6__G4' };
const flush = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
	vi.useFakeTimers();
	vi.setSystemTime(T0);
	p = new FakePlayer();
	// The room's clock runs 3 s ahead of this tab's.
	f = new Follower(p, { roomNow: () => Date.now() + 3_000, itemOf: (t) => ({ key: `k:${t.t}`, title: t.t }) as LibraryItem });
});
afterEach(() => {
	f.dispose();
	vi.useRealTimers();
});

describe('following the DJ', () => {
	it("knows where the room's song is: held before its start moment, running after", () => {
		expect(roomPosition({ positionMs: 5_000, atRoomMs: 100, playing: true }, 50)).toBe(5_000);
		expect(roomPosition({ positionMs: 5_000, atRoomMs: 100, playing: true }, 1_100)).toBe(6_000);
		expect(roomPosition({ positionMs: 5_000, atRoomMs: 100, playing: false }, 9_999)).toBe(5_000);
	});

	it("loads the DJ's next song paused, then starts it on the room's moment", async () => {
		f.prepare({ trackKey: 1, track: SONG, positionMs: 0 });
		await flush();
		expect(p.log).toEqual(['load Pink + White @0 paused']);
		const room = Date.now() + 3_000;
		f.timelineMsg({ trackKey: 1, positionMs: 0, atRoomMs: room + 800, playing: true });
		await flush();
		expect(p.playing).toBe(false);
		await vi.advanceTimersByTimeAsync(800);
		expect(p.playing).toBe(true);
		expect(p.log.at(-1)).toBe('play');
	});

	it('someone joining mid-song starts where the room is', async () => {
		const room = Date.now() + 3_000;
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 10_000, atRoomMs: room - 30_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		expect(p.log[0]).toBe('load Pink + White @40000 play');
		expect(p.playing).toBe(true);
	});

	it("follows the DJ's pause and seek; a timeline for another song is ignored", async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		await vi.advanceTimersByTimeAsync(5_000);
		f.timelineMsg({ trackKey: 1, positionMs: 5_000, playing: false });
		await flush();
		expect(p.playing).toBe(false);
		f.timelineMsg({ trackKey: 1, positionMs: 60_000, atRoomMs: Date.now() + 3_000, playing: true });
		await flush();
		expect(p.log.slice(-2)).toEqual(['seek 60000', 'play']);
		f.timelineMsg({ trackKey: 2, positionMs: 0, playing: false });
		await flush();
		expect(p.playing).toBe(true);
	});

	it('puts a listener back with the room after a stall, and leaves one who paused alone until they play', async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		f.observe(p.view());
		// A 3 s stall: the player's clock stops while the room's runs.
		p.followPause();
		p.playing = true;
		(p as unknown as { at: number }).at = Date.now() + 3_000;
		await vi.advanceTimersByTimeAsync(2_000);
		expect(p.log.at(-1)).toMatch(/^seek /);
		expect(Math.abs(p.position() - (f.position() ?? 0))).toBeLessThan(50);

		// Their own pause: no corrections; their play goes straight back to live.
		p.held = true;
		p.followPause();
		f.observe(p.view());
		const before = p.log.length;
		await vi.advanceTimersByTimeAsync(10_000);
		expect(p.log.length).toBe(before);
		p.held = false;
		f.observe(p.view());
		await flush();
		expect(p.log.slice(-2)[0]).toMatch(/^seek /);
		expect(p.playing).toBe(true);
		expect(Math.abs(p.position() - (f.position() ?? 0))).toBeLessThan(50);
	});

	it('a player stopped from elsewhere plays again where the room is, once two checks have found it stopped', async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		await vi.advanceTimersByTimeAsync(10_000);
		p.followPause(); // a skip past the end of its one-song queue, a stop from the tray: not the listener's pause
		f.observe(p.view());
		const before = p.log.length;
		await vi.advanceTimersByTimeAsync(2_000);
		expect(p.log.length).toBe(before); // once: the state may be a moment behind
		await vi.advanceTimersByTimeAsync(2_000);
		expect(p.playing).toBe(true);
		expect(Math.abs(p.position() - (f.position() ?? 0))).toBeLessThan(50);
	});

	it("a player playing through the DJ's pause is paused", async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 5_000, playing: false }, phase: { kind: 'playing' } });
		await flush();
		p.followPlay(); // a play from elsewhere that isn't the listener's
		f.observe(p.view());
		await vi.advanceTimersByTimeAsync(4_000);
		expect(p.playing).toBe(false);
		expect(p.log.at(-1)).toBe('pause');
	});

	it('a song that fails to load stops what played before, is tried once more, and starts with the room when it loads', async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		f.observe(p.view());
		p.fail = 1;
		f.prepare({ trackKey: 2, track: { t: 'Ivy', a: 'Frank Ocean' }, positionMs: 0 });
		await flush();
		expect(p.log.slice(-2)).toEqual(['load Ivy failed', 'pause']);
		f.observe(p.view());
		f.timelineMsg({ trackKey: 2, positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true });
		await flush();
		expect(p.playing).toBe(false); // not the song before under Ivy's timeline
		await vi.advanceTimersByTimeAsync(4_000);
		expect(p.log.some((l) => l.startsWith('load Ivy @'))).toBe(true);
		f.observe(p.view());
		expect(p.playing).toBe(true);
		expect(p.key).toBe('k:Ivy');
	});

	it("a song that still can't be loaded is not tried a third time", async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		f.observe(p.view());
		p.fail = 5;
		f.prepare({ trackKey: 2, track: { t: 'Ivy', a: 'Frank Ocean' }, positionMs: 0 });
		await flush();
		f.observe(p.view());
		await vi.advanceTimersByTimeAsync(20_000);
		expect(p.log.filter((l) => l === 'load Ivy failed').length).toBe(2);
		expect(p.playing).toBe(false);
	});

	it("another song in the player (a skip from elsewhere) gives way to the room's song", async () => {
		f.welcome({ track: SONG, trackKey: 1, timeline: { positionMs: 0, atRoomMs: Date.now() + 3_000, playing: true }, phase: { kind: 'playing' } });
		await flush();
		p.key = 'k:something else';
		f.observe(p.view());
		await vi.advanceTimersByTimeAsync(4_000);
		expect(p.log.at(-1)).toMatch(/^load Pink \+ White @\d+ play$/);
		expect(p.key).toBe('k:Pink + White');
	});
});
