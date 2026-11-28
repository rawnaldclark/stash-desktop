/**
 * The host's side of a session, from this tab: the host plays from their own player, untouched (any list, radio, skip, seek), and
 * this bridge tells the room what happens (the share Worker's room messages, MP3APK infra/share-worker README "Sessions protocol"):
 *
 * - a new song: `load {track, positionMs, queue, why}` (the song, where it is, what's up next here, and why it changed); the room's
 *   `prepare` for it is answered `status ready` at once, and the room starts everyone at its own moment, a moment ahead. The host's
 *   player is never held for it: listeners hear the host about half a second behind, which nobody can tell, and stay together.
 * - play, pause and a seek (a jump in the position the player didn't play through): `play`, `pause`, `seek {positionMs}`.
 * - Up next changing: `queue {queue}` (the host's queue is the session's; listeners see it).
 * - songs the room puts in its queue that aren't the host's (a request the host took, or Auto-accept's) are added to the host's
 *   own queue, so the player plays them; that queue entry (not every copy of the song) keeps who asked, so the room says
 *   "Lee's pick" when it plays, and only then.
 * - a co-host's skip (`command {action: "skip", trackKey}`) skips here, if that song is still the one playing.
 */
import type { QueueItem } from '../api';
import type { LibraryItem } from '../identity';
import { itemFromWire, wireTrackOf } from '../share/links';
import type { WireTrack } from '../share/rules';
import type { Frame } from './room';

/** The host's player, as the bridge uses it. */
export interface HostPlayer {
	position(): number;
	enqueue(items: LibraryItem[], opts?: { next?: boolean }): void;
	next(): void;
}

/** What the bridge reads of the player's state each time it changes. */
export type HostView = {
	/** The song loaded now (its queue slot id and the song), or null. */
	cur: { qid: string; item: LibraryItem } | null;
	playing: boolean;
	durationMs: number;
	/** What follows the current song here, in order (the first 20 are sent). */
	upcoming: QueueItem[];
	stopped: 'ended' | 'failed' | null;
};

export type SessionWire = WireTrack & { by?: string };

const QUEUE_MAX = 20;
/** A position this far from where playing would have taken it is a seek. */
const SEEK_JUMP_MS = 2_500;

/** The same song, as the room compares them (title and artist, case-blind). */
const sameSong = (a: { t: string; a: string }, b: { t: string; a: string }) => a.t.toLowerCase() === b.t.toLowerCase() && a.a.toLowerCase() === b.a.toLowerCase();

export class HostBridge {
	private me: string | null = null;
	private sentQid: string | null = null;
	private sentQueue = '';
	private playing = false;
	private lastPos = 0;
	private lastAt = 0;
	private trackKey: number | null = null;
	/** Who asked for each queue entry added here from the room's queue, by its queue id. */
	private byQid = new Map<string, string>();
	/** Songs added here whose queue entries haven't shown up yet: the next new entry for the song takes who asked. */
	private pending: { key: string; by: string }[] = [];
	/** The queue ids seen last time: an entry not among them is new. */
	private seenQids = new Set<string>();
	/** The room's songs from someone else added here, while the room's queue still has them, so each is added once. */
	private added = new Set<string>();
	private view: HostView | null = null;

	constructor(
		private readonly player: HostPlayer,
		private readonly send: (m: Frame) => boolean,
		private readonly now: () => number = Date.now
	) {}

	/**
	 * The welcome: this tab is the host (its member id); whatever plays now is sent as the first song. A reconnect to the song
	 * the room still has is not sent again (everyone would buffer it afresh): only whether it plays, and where, are put right.
	 */
	welcome(memberId: string, state: Record<string, unknown>): void {
		this.me = memberId;
		this.sentQid = null;
		this.sentQueue = '';
		this.trackKey = typeof state.trackKey === 'number' ? state.trackKey : null;
		const v = this.view;
		if (!v) return;
		const had = state.track as { t?: unknown; a?: unknown } | null | undefined;
		const tl = state.timeline as { playing?: unknown } | undefined;
		if (v.cur && had && typeof had.t === 'string' && typeof had.a === 'string' && sameSong({ t: v.cur.item.title, a: v.cur.item.artist }, { t: had.t, a: had.a })) {
			this.sentQid = v.cur.qid;
			this.playing = tl?.playing === true;
			this.lastPos = this.player.position();
			this.lastAt = this.now();
			this.observe(v); // play or pause, if the room has it the other way; Up next
			if (v.playing) this.send({ t: 'seek', positionMs: Math.max(0, Math.round(this.player.position())) });
			return;
		}
		this.observe(v);
	}

	/** The room's messages the host acts on. */
	frame(f: Frame): void {
		if (f.t === 'prepare' && typeof f.trackKey === 'number') {
			this.trackKey = f.trackKey;
			// Ours (or a song the room started itself): the host is ready at once; its player is playing it already.
			this.send({ t: 'status', status: 'ready', trackKey: f.trackKey });
		} else if (f.t === 'timeline' && typeof f.trackKey === 'number' && f.trackKey === this.trackKey) {
			// The room started (or the host's play went through): a host paused meanwhile says so.
			if (f.playing === true && this.view && !this.view.playing) this.send({ t: 'pause' });
		} else if (f.t === 'queue' && Array.isArray(f.queue)) {
			this.takeAdds(f.queue as SessionWire[]);
		} else if (f.t === 'command' && f.action === 'skip' && f.trackKey === this.trackKey) {
			this.player.next();
		}
	}

	/** The player's state, as it changes. */
	observe(v: HostView): void {
		this.view = v;
		this.claim(v);
		if (!this.me) return;
		const now = this.now();
		const pos = this.player.position();
		if (!v.cur) {
			if (this.playing) this.send({ t: 'pause' });
			this.playing = false;
			return;
		}
		if (v.cur.qid !== this.sentQid) {
			const why = this.sentQid === null ? 'pick' : this.endedNaturally(now) ? 'end' : 'skip';
			if (this.sentQid) this.byQid.delete(this.sentQid); // it played: a later copy is nobody's pick
			const queue = this.wireQueue(v.upcoming);
			this.sentQid = v.cur.qid;
			this.sentQueue = JSON.stringify(queue);
			this.playing = v.playing;
			this.lastPos = pos;
			this.lastAt = now;
			this.send({ t: 'load', track: this.wire(v.cur.item, v.cur.qid), positionMs: Math.max(0, Math.round(pos)), queue, why });
			return;
		}
		if (v.playing !== this.playing && !(v.stopped && !v.playing && !this.playing)) {
			this.playing = v.playing;
			this.send({ t: v.playing ? 'play' : 'pause' });
		} else if (this.playing && Math.abs(pos - (this.lastPos + (now - this.lastAt))) > SEEK_JUMP_MS) {
			this.send({ t: 'seek', positionMs: Math.max(0, Math.round(pos)) });
		} else if (!this.playing && Math.abs(pos - this.lastPos) > SEEK_JUMP_MS) {
			this.send({ t: 'seek', positionMs: Math.max(0, Math.round(pos)) });
		}
		this.lastPos = pos;
		this.lastAt = now;
		const queue = this.wireQueue(v.upcoming);
		const sent = JSON.stringify(queue);
		if (sent !== this.sentQueue) {
			this.sentQueue = sent;
			this.send({ t: 'queue', queue });
		}
	}

	/** A song as the room gets it: a shared song, with who asked for it when someone did. */
	private wire(item: LibraryItem, qid: string): SessionWire {
		const t: SessionWire = wireTrackOf(item, true);
		const by = this.byQid.get(qid);
		if (by) t.by = by;
		return t;
	}

	private wireQueue(items: QueueItem[]): SessionWire[] {
		return items.slice(0, QUEUE_MAX).map((i) => this.wire(i, i.qid));
	}

	/** Entries new since last time take who asked, for songs added from the room's queue. */
	private claim(v: HostView): void {
		const now = new Set<string>();
		if (v.cur) now.add(v.cur.qid);
		for (const u of v.upcoming) {
			now.add(u.qid);
			if (this.seenQids.has(u.qid) || !this.pending.length) continue;
			const i = this.pending.findIndex((p) => p.key === u.key);
			if (i < 0) continue;
			this.byQid.set(u.qid, this.pending[i].by);
			this.pending.splice(i, 1);
		}
		this.seenQids = now;
	}

	/** The last song played to (near) its end, rather than being skipped. */
	private endedNaturally(now: number): boolean {
		const dur = this.view?.durationMs ?? 0;
		if (dur <= 0) return false;
		const reached = this.lastPos + (this.playing ? now - this.lastAt : 0);
		return reached >= dur - 4_000;
	}

	/**
	 * The room's queue, from a `queue` frame: its songs from someone else are added here, once each, in place. One the room's
	 * queue no longer has (it played, or was taken out) can be asked for again.
	 */
	private takeAdds(queue: SessionWire[]): void {
		const present = new Set<string>();
		queue.forEach((t, i) => {
			if (!t || typeof t.t !== 'string' || typeof t.a !== 'string' || !t.by || t.by === this.me) return;
			const key = `${t.t.toLowerCase()}\u0000${t.a.toLowerCase()}\u0000${t.by}`;
			present.add(key);
			if (this.added.has(key)) return;
			this.added.add(key);
			const item = itemFromWire(t);
			this.pending.push({ key: item.key, by: t.by });
			this.player.enqueue([item], { next: i === 0 });
		});
		for (const k of this.added) if (!present.has(k)) this.added.delete(k);
	}
}
