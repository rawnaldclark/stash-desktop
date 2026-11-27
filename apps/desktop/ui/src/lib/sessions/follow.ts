/**
 * A listener's player, kept with the session's DJ: the room's song (`prepare`, or the welcome's state for someone who joins
 * mid-song) and its timeline (`timeline`: where the song was at a moment of the room's clock, and whether it plays) become the
 * player's follow* calls (engine.ts, `setListening`). The DJ's phone plays the song; a listener only ever plays along.
 *
 * - A new song loads paused at its start position; the timeline's `playing` starts it at the room's `atRoomMs` (a moment ahead,
 *   so every phone starts together), or at once, further in, when that moment has passed.
 * - Every 2 s, a listener playing more than 1.2 s off the room is put back where the room is (a stall, a slow start, a tab that
 *   slept). A listener who paused is left alone: their play goes back to live (the engine's listenHeld, watched here).
 * - A listener never holds the room up: the room doesn't wait for them (only the host's status counts).
 */
import type { LibraryItem } from '../identity';
import type { WireTrack } from '../share/rules';

export type Timeline = { positionMs: number; atRoomMs?: number; playing: boolean };

/** The player, as the follower uses it (engine.ts, or the fake). */
export interface FollowPlayer {
	followLoad(item: LibraryItem, positionMs: number, autoplay: boolean): Promise<void>;
	followSeek(ms: number): void;
	followPlay(): void;
	followPause(): void;
	position(): number;
}

/** What the follower reads of the player's state each time it changes. */
export type FollowView = { playing: boolean; buffering: boolean; listenHeld: boolean; currentKey: string | null };

export type FollowDeps = {
	/** The room's time now (room.ts `roomNow`). */
	roomNow: () => number;
	/** A shared song as a playable item (share/links.ts `itemFromWire`). */
	itemOf: (t: WireTrack) => LibraryItem;
	setTimeout?: (fn: () => void, ms: number) => unknown;
	clearTimeout?: (h: unknown) => void;
	setInterval?: (fn: () => void, ms: number) => unknown;
	clearInterval?: (h: unknown) => void;
};

const DRIFT_MS = 1_200;
const CHECK_MS = 2_000;

/** Where the room's song is at room time [now]: still its position before the start moment, then running from it. */
export function roomPosition(t: Timeline, now: number): number {
	if (!t.playing) return t.positionMs;
	const at = typeof t.atRoomMs === 'number' && Number.isFinite(t.atRoomMs) ? t.atRoomMs : now;
	return t.positionMs + Math.max(0, now - at);
}

/** The room's song number, or null. */
export const songKey = (v: unknown): number | null => (typeof v === 'number' && Number.isSafeInteger(v) && v > 0 ? v : null);

const isTimeline = (v: unknown): v is Timeline =>
	!!v && typeof v === 'object' && typeof (v as Timeline).positionMs === 'number' && Number.isFinite((v as Timeline).positionMs) && typeof (v as Timeline).playing === 'boolean';

export class Follower {
	/** The room's song number (`trackKey`: 1, 2, … as the room counts its songs). */
	private trackKey: number | null = null;
	private itemKey: string | null = null;
	private timeline: Timeline | null = null;
	private startTimer: unknown = null;
	private checkTimer: unknown;
	private view: FollowView = { playing: false, buffering: false, listenHeld: false, currentKey: null };
	private loading: Promise<void> | null = null;
	private disposed = false;

	private readonly later: (fn: () => void, ms: number) => unknown;
	private readonly cancel: (h: unknown) => void;
	private readonly stopEvery: (h: unknown) => void;

	constructor(
		private readonly player: FollowPlayer,
		private readonly deps: FollowDeps
	) {
		this.later = deps.setTimeout ?? ((fn, ms) => setTimeout(fn, ms));
		this.cancel = deps.clearTimeout ?? ((h) => clearTimeout(h as ReturnType<typeof setTimeout>));
		this.stopEvery = deps.clearInterval ?? ((h) => clearInterval(h as ReturnType<typeof setInterval>));
		const every = deps.setInterval ?? ((fn, ms) => setInterval(fn, ms));
		this.checkTimer = every(() => this.check(), CHECK_MS);
	}

	/** The welcome's state (also after a reconnect): the song and where it is. */
	welcome(state: Record<string, unknown>): void {
		const track = state.track as WireTrack | null | undefined;
		const key = songKey(state.trackKey);
		const tl = isTimeline(state.timeline) ? state.timeline : null;
		const phase = state.phase as { kind?: string } | undefined;
		if (!track || !key || !tl) return;
		const preparing = phase?.kind === 'preparing';
		this.song(key, track, preparing ? { ...tl, playing: false } : tl);
	}

	/** The DJ's next song: loaded paused at its start, to start on the timeline. */
	prepare(m: Record<string, unknown>): void {
		const key = songKey(m.trackKey);
		const track = m.track as WireTrack | undefined;
		if (!key || !track) return;
		const pos = typeof m.positionMs === 'number' && Number.isFinite(m.positionMs) ? m.positionMs : 0;
		this.song(key, track, { positionMs: pos, playing: false });
	}

	/** Play, pause or seek, or the song's start: for the song this follower holds. */
	timelineMsg(m: Record<string, unknown>): void {
		if (songKey(m.trackKey) === null || songKey(m.trackKey) !== this.trackKey || !isTimeline(m)) return;
		this.timeline = { positionMs: m.positionMs, atRoomMs: typeof m.atRoomMs === 'number' ? m.atRoomMs : undefined, playing: m.playing };
		void this.apply();
	}

	/** The player's state, as it changes: a listener's play after their own pause goes straight back to live. */
	observe(v: FollowView): void {
		const unheld = this.view.listenHeld && !v.listenHeld;
		this.view = v;
		if (unheld) void this.apply();
	}

	/** Where the room's song is now (for the progress bar), or null with none. */
	position(): number | null {
		return this.timeline ? roomPosition(this.timeline, this.deps.roomNow()) : null;
	}

	dispose(): void {
		this.disposed = true;
		this.stopEvery(this.checkTimer);
		if (this.startTimer !== null) this.cancel(this.startTimer);
		this.startTimer = null;
	}

	private song(key: number, track: WireTrack, tl: Timeline): void {
		if (this.disposed) return;
		this.timeline = tl;
		if (key === this.trackKey && this.loading) return void this.loading.then(() => this.apply());
		if (key === this.trackKey) return void this.apply();
		this.trackKey = key;
		const item = this.deps.itemOf(track);
		this.itemKey = item.key;
		const now = this.deps.roomNow();
		const at = roomPosition(tl, now);
		const starting = tl.playing && (tl.atRoomMs === undefined || tl.atRoomMs <= now);
		const load = this.player.followLoad(item, at, starting).catch(() => {});
		this.loading = load;
		void load.then(() => {
			if (this.loading === load) this.loading = null;
			if (this.trackKey === key) void this.apply();
		});
	}

	/** Puts the player where the room's timeline says. */
	private async apply(): Promise<void> {
		if (this.disposed || !this.timeline) return;
		if (this.loading) return; // the load's end applies it
		if (this.startTimer !== null) this.cancel(this.startTimer);
		this.startTimer = null;
		const tl = this.timeline;
		if (!tl.playing) {
			this.player.followPause();
			if (Math.abs(this.player.position() - tl.positionMs) > 250) this.player.followSeek(tl.positionMs);
			return;
		}
		const now = this.deps.roomNow();
		const at = tl.atRoomMs ?? now;
		if (at > now) {
			// The start is a moment ahead: in place, then on time.
			this.player.followPause();
			if (Math.abs(this.player.position() - tl.positionMs) > 250) this.player.followSeek(tl.positionMs);
			this.startTimer = this.later(() => {
				this.startTimer = null;
				this.player.followPlay();
			}, at - now);
			return;
		}
		const want = roomPosition(tl, now);
		if (Math.abs(this.player.position() - want) > DRIFT_MS || !this.view.playing) this.player.followSeek(want);
		this.player.followPlay();
	}

	/** Every 2 s: back with the room after a stall or a sleep. A listener who paused stays paused. */
	private check(): void {
		if (this.disposed || this.loading || !this.timeline?.playing || this.startTimer !== null) return;
		if (this.view.listenHeld || this.view.buffering || !this.view.playing) return;
		if (this.itemKey && this.view.currentKey && this.view.currentKey !== this.itemKey) return;
		const want = roomPosition(this.timeline, this.deps.roomNow());
		if (Math.abs(this.player.position() - want) > DRIFT_MS) this.player.followSeek(want);
	}
}
