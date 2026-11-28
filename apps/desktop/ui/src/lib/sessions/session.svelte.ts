/**
 * Sessions on this computer (ported from the web player's src/lib/sessions/session.svelte.ts; keep them in step): the directory
 * (Live now, Upcoming, followed stations) and the one session this window is tuned in to or hosts, as
 * reactive state the pages read (routes/sessions, the room in the side panel, full screen at /now). Plain classes with runes; the
 * logic that needs tests lives in room.ts, follow.ts and rules.ts.
 *
 * Tuning in needs this browser to have joined Community (its key is how a session knows a listener; nothing is sent before Join).
 * The player follows the DJ (engine `setListening`), so while tuned in, song rows request instead of play (TrackRow, ContextMenu).
 * Two minutes tuned in counts as heard (the station's vote needs it); that is sent once per station per tab.
 */
import { player } from '../app';
import { sessionActive } from './active';
import { community } from '../community/key';
import { wireTrackOf, itemFromWire } from '../share/links';
import type { WireTrack } from '../share/rules';
import type { LibraryItem } from '../identity';
import type { PlayerState } from '../api';
import { LS, ui } from '../ui/state.svelte';
import { createSessionsClient, sessionsMessage, SessionsProblem } from './client';
import { isFace } from './faces';
import { Follower, songKey } from './follow';
import { HostBridge } from './host';
import { RoomSocket, type Frame, type RoomStatus, type SocketLike } from './room';
import type { FollowedStation, LiveSession, StationSummary, UpcomingStation, Vote } from './rules';
import { endedText, isPhrase, personColor, phraseText, type PhraseId } from './words';

export const client = createSessionsClient();

/** The player, as the web's code reaches it (the web loads its app lazily; here it is always there). */
const loadApp = async () => ({ player });

/** Where a room's socket goes (the core says: `wss://…/v1/rooms`), asked once. */
let roomsAt: string | null = null;
async function rooms(): Promise<string | null> {
	if (!roomsAt) roomsAt = (await client.status().catch(() => ({ rooms: null }))).rooms;
	return roomsAt;
}

export type Member = { id: string; name: string; regular: boolean };
export type SessionInfo = { listeners: number; cohosts: string[]; presence: 'booth' | 'away'; autoAccept: boolean; lastSong: boolean };
/** A song in the session: a shared song, and who asked for it when it was a request (their member id). */
export type SessionTrack = WireTrack & { by?: string };
export type FeedLine = { key: string; text: string; who: string; color: string; at: number; count: number; phrase?: PhraseId };
export type Float = { id: number; face: string; color: string; x: number; big: boolean };
/** This listener's own requests: waiting for the DJ, or in the queue ("Up next"). */
export type MyRequest = { id: string | null; track: WireTrack; state: 'waiting' | 'queued' };
/** A request waiting for the DJ (the host sees everyone's). */
export type Request = { id: string; from: string; track: WireTrack };

const HEARD_MS = 120_000;
const NAME_KEY = 'stash.sessions.name';
/** The most a name in a room may be (the room's own cap). */
export const MAX_ROOM_NAME = 40;
const FEED_MAX = 5;
const FLOATS_MAX = 12;
/** A phrase said again within this long folds into the line before ("Sam and 3 others · Tune!"). */
const FOLD_MS = 10_000;

const str = (v: unknown, max = 200): string | null => (typeof v === 'string' && v.trim() ? v.trim().slice(0, max) : null);

class SessionState {
	/** idle: not in a session. joining: asked to tune in. live: in the room. retrying: back in a moment. ended: out, `endedLine` says why. */
	phase = $state<'idle' | 'joining' | 'live' | 'retrying' | 'ended'>('idle');
	station = $state<StationSummary | null>(null);
	code = $state<string | null>(null);
	me = $state<string | null>(null);
	hostId = $state<string | null>(null);
	members = $state<Member[]>([]);
	info = $state<SessionInfo | null>(null);
	track = $state<SessionTrack | null>(null);
	/** The room's song number (1, 2, …). */
	trackKey = $state<number | null>(null);
	stats = $state<{ reactions: number; saves: number; favourite: boolean } | null>(null);
	queue = $state<SessionTrack[]>([]);
	feed = $state<FeedLine[]>([]);
	floats = $state<Float[]>([]);
	waiting = $state<MyRequest[]>([]);
	endedLine = $state<string | null>(null);
	/** The song this listener saved (sent once a song). */
	savedKey = $state<number | null>(null);
	/** listener: tuned in. host: this tab is the DJ (it went live). */
	role = $state<'listener' | 'host'>('listener');
	/** The host's view: every request waiting, oldest first. */
	requests = $state<Request[]>([]);
	/** When this tab joined the session (the host: when it went live). */
	startedAt = $state(0);

	private room: RoomSocket | null = null;
	private bridge: HostBridge | null = null;
	/** A tune-in or Go live on its way. */
	private busy = false;
	private follower: Follower | null = null;
	private unsubscribe: (() => void) | null = null;
	private heardTimer: ReturnType<typeof setTimeout> | null = null;
	private heardSent = new Set<string>();
	private floatSeq = 0;
	private key: string | null = null;

	/**
	 * What rooms call this listener, as asked once before the first tune-in (kept in this browser). null: never asked; '' asked,
	 * and left blank (the room then shows them without a name).
	 */
	name = $state<string | null>(LS.get(NAME_KEY));

	setName(n: string): void {
		const v = n.replace(/\s+/g, ' ').trim().slice(0, MAX_ROOM_NAME);
		this.name = v;
		LS.set(NAME_KEY, v);
	}

	get host(): Member | null {
		return this.members.find((m) => m.id === this.hostId) ?? null;
	}
	get hostName(): string {
		return this.host?.name || this.station?.hostName || 'The DJ';
	}
	get inSession(): boolean {
		return this.phase === 'joining' || this.phase === 'live' || this.phase === 'retrying';
	}
	/** Up next in the queue, this listener's own requests. */
	get mineQueued(): SessionTrack[] {
		return this.me ? this.queue.filter((t) => t.by === this.me) : [];
	}
	/** Waiting and up next, in that order: what the "Your requests" list shows (3 waiting at most). */
	get mine(): MyRequest[] {
		return [...this.mineQueued.map((track): MyRequest => ({ id: null, track, state: 'queued' })), ...this.waiting];
	}
	nameOf(id: string | undefined | null): string {
		if (!id) return '';
		if (id === this.me) return 'You';
		return this.members.find((m) => m.id === id)?.name || 'Someone';
	}

	/** Tunes in to a station's session. Asks for Community first when this browser hasn't joined. */
	async tuneIn(stationId: string): Promise<'ok' | 'join' | 'off-air' | 'failed' | 'hosting'> {
		// One at a time: a click while one is on its way (a double-click, another station, Go live) is let go.
		if (this.busy) return 'ok';
		this.busy = true;
		try {
			return await this.tuneInNow(stationId);
		} finally {
			this.busy = false;
		}
	}

	private async tuneInNow(stationId: string): Promise<'ok' | 'join' | 'off-air' | 'failed' | 'hosting'> {
		if (this.inSession && this.station?.id === stationId) return 'ok';
		// A host tuning in elsewhere would end their own session: they end it first, on purpose.
		if (this.inSession && this.role === 'host') return 'hosting';
		const key = await community.key();
		if (!key) return 'join';
		if (this.inSession) this.leave();
		this.phase = 'joining';
		this.endedLine = null;
		let page;
		try {
			page = await client.station(stationId, key);
		} catch (e) {
			this.phase = 'idle';
			ui.toast(sessionsMessage(e));
			return 'failed';
		}
		if (!page.live) {
			this.phase = 'idle';
			ui.toast(`${page.station.name} isn’t on the air right now.`);
			return 'off-air';
		}
		if (page.live.full) {
			this.phase = 'idle';
			ui.toast(endedText('full', ''));
			return 'failed';
		}
		this.reset();
		ui.room = true;
		this.key = key;
		this.station = page.station;
		this.code = page.live.room;
		const app = await loadApp();
		const p = app.player;
		p.setListening({ station: page.station.name, host: page.station.hostName ?? 'The DJ' });
		const base = await rooms();
		if (!base) {
			p.setListening(null);
			this.phase = 'idle';
			this.station = null;
			this.code = null;
			this.closeRoomTab();
			ui.toast('Couldn’t reach Sessions. Try again.');
			return 'failed';
		}
		const room: RoomSocket = new RoomSocket(base, page.live.room, { communityKey: key, ...(this.name ? { name: this.name } : {}) }, (f) => this.room === room && this.frame(f), (s) => this.room === room && this.status(s), {
			open: (url) => new WebSocket(url) as unknown as SocketLike
		});
		this.room = room;
		const follower = new Follower(p, { roomNow: () => room.roomNow(), itemOf: (t) => itemFromWire(t) });
		this.follower = follower;
		this.unsubscribe = p.state.subscribe((st: PlayerState) =>
			follower.observe({ playing: st.playing, buffering: st.buffering, listenHeld: !!st.listenHeld, currentKey: st.listeningKey ?? st.current?.key ?? null })
		);
		room.connect();
		return 'ok';
	}

	/**
	 * Goes live with this computer's station: a public room for it, this window its host. The player plays on, untouched; the
	 * bridge tells the room what it plays (host.ts). Quitting Stash, or 15 minutes without playing, ends the session.
	 */
	async goLive(station: StationSummary): Promise<boolean> {
		if (this.busy) return false;
		this.busy = true;
		try {
			return await this.goLiveNow(station);
		} finally {
			this.busy = false;
		}
	}

	private async goLiveNow(station: StationSummary): Promise<boolean> {
		const key = await community.key();
		if (!key) {
			ui.toast('Join Community first: it’s how your station is yours.');
			return false;
		}
		if (this.inSession) this.leave();
		let opened;
		try {
			opened = await client.goLive(key, station.id, this.name || undefined);
		} catch (e) {
			ui.toast(sessionsMessage(e));
			return false;
		}
		this.reset();
		this.role = 'host';
		this.phase = 'joining';
		this.endedLine = null;
		this.key = key;
		this.station = station;
		this.code = opened.code;
		this.startedAt = Date.now();
		ui.room = true;
		ui.hosting = station.name;
		sessionActive.hosting = true;
		const app = await loadApp();
		const p = app.player;
		const base = await rooms();
		if (!base) {
			this.teardown();
			this.phase = 'idle';
			this.station = null;
			this.code = null;
			this.closeRoomTab();
			ui.toast('Couldn’t reach Sessions. Try again.');
			return false;
		}
		const room: RoomSocket = new RoomSocket(base, opened.code, { hostKey: opened.hostKey, ...(this.name ? { name: this.name } : {}) }, (f) => this.room === room && this.frame(f), (s) => this.room === room && this.status(s), {
			open: (url) => new WebSocket(url) as unknown as SocketLike
		});
		this.room = room;
		const bridge = new HostBridge(p, (m) => room.send(m));
		this.bridge = bridge;
		this.unsubscribe = p.state.subscribe((st: PlayerState) =>
			bridge.observe({
				cur: st.current ? { qid: st.current.qid, item: st.current } : null,
				playing: st.playing,
				durationMs: st.durationMs,
				// The window holds the queue from `queueStart` on: what follows the song playing starts one after it.
				upcoming: st.index >= 0 ? st.queue.slice(Math.max(0, st.index - st.queueStart + 1)) : [],
				stopped: null
			})
		);
		room.connect();
		return true;
	}

	/** The host's answer to a request: Play next, Add to queue, or pass. */
	answer(id: string, action: 'next' | 'queue' | 'pass'): void {
		if (this.role !== 'host' || !this.room?.send({ t: 'requestAction', id, action })) return;
		this.requests = this.requests.filter((r) => r.id !== id);
	}

	/** Auto-accept: requests go straight into the queue (as Add to queue). */
	autoAccept(on: boolean): void {
		if (this.role === 'host' && this.room?.send({ t: 'autoAccept', on }) && this.info) this.info = { ...this.info, autoAccept: on };
	}

	/** End after this song (on), or take it back (off). */
	lastSong(on: boolean): void {
		if (this.role === 'host' && this.room?.send({ t: 'lastSong', on }) && this.info) this.info = { ...this.info, lastSong: on };
	}

	/** End now: everyone's session ends. */
	endNow(): void {
		if (this.role === 'host') this.room?.send({ t: 'end' });
	}

	/** This tab's Leave: out of the room; the song stays, paused, as this player's own. */
	leave(): void {
		this.room?.leave();
		this.teardown();
		this.phase = 'idle';
		this.closeRoomTab();
	}

	/** Back to the directory after an ending. */
	dismiss(): void {
		if (this.phase === 'ended') this.phase = 'idle';
		this.endedLine = null;
		this.closeRoomTab();
	}

	/** The Room tab goes with the session: the panel shows Lyrics again. */
	private closeRoomTab(): void {
		ui.room = false;
		if (ui.panelTab === 'room') ui.panelTab = 'lyrics';
	}

	react(face: string): boolean {
		if (!isFace(face)) return false;
		return !!this.room?.send({ t: 'react', emoji: face });
	}

	say(phrase: PhraseId): boolean {
		return !!this.room?.send({ t: 'say', phrase });
	}

	/** Asks the DJ for a song. False (with a line) when it can't go. */
	request(item: LibraryItem): boolean {
		if (!this.room || this.phase !== 'live') return false;
		if (this.waiting.length >= 3) {
			ui.toast('You have 3 requests waiting. Take one back to ask for another.');
			return false;
		}
		const track = wireTrackOf(item, true);
		if (this.track && this.track.t === track.t && this.track.a === track.a) {
			ui.toast('That song is playing now.');
			return false;
		}
		if (this.mine.some((r) => r.track.t === track.t && r.track.a === track.a)) {
			ui.toast('You asked for that one already.');
			return false;
		}
		if (!this.room.send({ t: 'request', track })) return false;
		ui.toast(`Asked ${this.hostName} for ${item.title}.`);
		return true;
	}

	cancelRequest(id: string): void {
		this.room?.send({ t: 'requestCancel', id });
	}

	/** This listener saved the playing song (their own Like): counted once a song for the room's "saved". */
	saved(): void {
		if (!this.trackKey || this.savedKey === this.trackKey) return;
		if (this.room?.send({ t: 'saved', trackKey: this.trackKey })) this.savedKey = this.trackKey;
	}

	async follow(on: boolean): Promise<void> {
		const s = this.station;
		if (!s || !this.key) return;
		try {
			const r = await client.follow(this.key, s.id, on);
			if (this.station?.id === s.id) this.station = { ...this.station, following: r.following, followers: r.followers };
		} catch (e) {
			ui.toast(sessionsMessage(e));
		}
	}

	async vote(value: Vote): Promise<void> {
		const s = this.station;
		if (!s || !this.key) return;
		try {
			const r = await client.vote(this.key, s.id, value);
			if (this.station?.id === s.id) this.station = { ...this.station, up: r.up, down: r.down, myVote: r.myVote };
		} catch (e) {
			ui.toast(sessionsMessage(e));
		}
	}

	/** Where the room's song is now (the progress bar while tuned in). */
	position(): number | null {
		return this.follower?.position() ?? null;
	}

	// ------------------------------------------------------------------------------------------------ the room's messages

	private status(s: RoomStatus): void {
		if (s.kind === 'open') {
			this.phase = 'live';
			this.me = s.memberId;
			this.startHeard();
		} else if (s.kind === 'retrying') {
			if (this.phase === 'live') this.phase = 'retrying';
		} else if (s.kind === 'closed') {
			if (s.reason === 'left') return;
			this.end(s.reason === 'ended' ? (this.endedLine ?? endedText('', this.hostName)) : endedText(s.reason, this.hostName));
		}
	}

	private frame(f: Frame): void {
		if (this.bridge && f.t !== 'welcome') this.bridge.frame(f);
		switch (f.t) {
			case 'welcome': {
				const st = (f.state ?? {}) as Record<string, unknown>;
				if (this.bridge && typeof f.memberId === 'string') this.bridge.welcome(f.memberId, st);
				this.hostId = str(st.host, 16);
				this.members = this.cleanMembers(st.members);
				this.info = this.cleanInfo(st.session) ?? this.info;
				this.setTrack(st.track as SessionTrack | null, songKey(st.trackKey));
				this.queue = this.cleanQueue(st.queue);
				this.stats = this.cleanStats(st.songStats);
				this.follower?.welcome(st);
				return;
			}
			case 'members':
				this.members = this.cleanMembers(f.members);
				return;
			case 'session': {
				const info = this.cleanInfo(f);
				if (info) this.info = info;
				return;
			}
			case 'prepare': {
				const key = songKey(f.trackKey);
				const changed = key !== this.trackKey;
				this.setTrack(f.track as SessionTrack, key);
				if (changed) {
					this.stats = null;
					const by = this.track?.by;
					if (by && by !== this.hostId) this.line(`pick:${key}`, by, `${by === this.me ? 'Your' : `${this.nameOf(by)}’s`} pick is on`);
				}
				this.follower?.prepare(f);
				return;
			}
			case 'timeline':
				this.follower?.timelineMsg(f);
				return;
			case 'queue':
				this.queue = this.cleanQueue(f.queue);
				return;
			case 'songStats':
				if (f.trackKey === this.trackKey) this.stats = this.cleanStats(f);
				return;
			case 'reaction':
				if (typeof f.from === 'string' && typeof f.emoji === 'string' && isFace(f.emoji)) this.float(f.emoji, f.from);
				return;
			case 'said':
				if (typeof f.from === 'string' && isPhrase(f.phrase)) this.said(f.from, f.phrase);
				return;
			case 'requests': {
				const list = Array.isArray(f.requests) ? f.requests : [];
				if (this.role === 'host') {
					this.requests = list.filter((r): r is Request => !!r && typeof r.id === 'string' && typeof r.from === 'string' && !!r.track && typeof r.track.t === 'string').slice(0, 30);
					return;
				}
				this.waiting = list
					.filter((r): r is { id: string; from: string; track: WireTrack } => !!r && typeof r.id === 'string' && (r.from === this.me || !this.me) && !!r.track)
					.slice(0, 3)
					.map((r) => ({ id: r.id, track: r.track, state: 'waiting' }));
				return;
			}
			case 'requestAnswer':
				if (f.state === 'full') ui.toast('Requests are full right now. Try again in a bit.');
				else if (f.state === 'queued') ui.toast('It’s in the queue.');
				return;
			case 'requestPassed': {
				const t = f.track as WireTrack | undefined;
				if (t?.t) ui.toast(`${this.hostName} passed on ${t.t}.`);
				return;
			}
			case 'ended':
				this.endedLine = endedText(typeof f.reason === 'string' ? f.reason : '', this.hostName);
				return;
		}
	}

	private setTrack(t: SessionTrack | null | undefined, key: number | null): void {
		this.trackKey = key;
		if (!t || typeof t.t !== 'string' || typeof t.a !== 'string') {
			this.track = null;
			return;
		}
		this.track = { ...t, ...(typeof t.by === 'string' ? { by: t.by } : {}) };
	}

	private cleanMembers(v: unknown): Member[] {
		if (!Array.isArray(v)) return [];
		return v
			.filter((m): m is Record<string, unknown> => !!m && typeof m === 'object' && typeof (m as Member).id === 'string')
			.slice(0, 60)
			.map((m) => ({ id: m.id as string, name: str(m.name, 40) ?? '', regular: m.regular === true }));
	}

	private cleanInfo(v: unknown): SessionInfo | null {
		if (!v || typeof v !== 'object') return null;
		const o = v as Record<string, unknown>;
		return {
			listeners: typeof o.listeners === 'number' ? o.listeners : 0,
			cohosts: Array.isArray(o.cohosts) ? o.cohosts.filter((x): x is string => typeof x === 'string') : [],
			presence: o.presence === 'away' ? 'away' : 'booth',
			autoAccept: o.autoAccept === true,
			lastSong: o.lastSong === true
		};
	}

	private cleanQueue(v: unknown): SessionTrack[] {
		if (!Array.isArray(v)) return [];
		return v.filter((t): t is SessionTrack => !!t && typeof t === 'object' && typeof t.t === 'string' && typeof t.a === 'string').slice(0, 200);
	}

	private cleanStats(v: unknown) {
		if (!v || typeof v !== 'object') return null;
		const o = v as Record<string, unknown>;
		return { reactions: Number(o.reactions) || 0, saves: Number(o.saves) || 0, favourite: o.favourite === true };
	}

	private float(face: string, from: string): void {
		const id = ++this.floatSeq;
		const big = face === 'k:tableflip' || face === 'k:unflip' || face === 'k:dance' || face === 'k:rockon' || face === 'k:sparkle';
		this.floats = [...this.floats, { id, face, color: personColor(from), x: 8 + Math.round(Math.random() * 68), big }].slice(-FLOATS_MAX);
		setTimeout(() => (this.floats = this.floats.filter((x) => x.id !== id)), big ? 3_800 : 3_300);
	}

	private said(from: string, phrase: PhraseId): void {
		const now = Date.now();
		const last = this.feed.at(-1);
		const text = phraseText(phrase)!;
		if (last && last.phrase === phrase && now - last.at < FOLD_MS && last.who !== from) {
			this.feed = [...this.feed.slice(0, -1), { ...last, count: last.count + 1, at: now }];
			return;
		}
		this.feed = [...this.feed, { key: `say:${now}:${from}`, who: from, color: personColor(from), text, at: now, count: 1, phrase }].slice(-FEED_MAX);
	}

	private line(key: string, who: string, text: string): void {
		this.feed = [...this.feed, { key, who, color: personColor(who), text, at: Date.now(), count: 1 }].slice(-FEED_MAX);
	}

	private startHeard(): void {
		const id = this.station?.id;
		if (!id || this.heardSent.has(id) || this.station?.heard || this.heardTimer) return;
		this.heardTimer = setTimeout(() => {
			this.heardTimer = null;
			if (!this.key || this.station?.id !== id || this.phase !== 'live') return;
			this.heardSent.add(id);
			void client.heard(this.key, id).then(
				() => this.station?.id === id && (this.station = { ...this.station, heard: true }),
				(e) => {
					if (!(e instanceof SessionsProblem) || e.code === 'unavailable') this.heardSent.delete(id);
				}
			);
		}, HEARD_MS);
	}

	private end(line: string): void {
		const host = this.role === 'host';
		this.teardown();
		this.endedLine = host && /ended the session|last song/.test(line) ? 'Your session has ended. Thanks for playing.' : line;
		this.phase = 'ended';
	}

	private teardown(): void {
		this.follower?.dispose();
		this.follower = null;
		this.bridge = null;
		ui.hosting = null;
		sessionActive.hosting = false;
		this.unsubscribe?.();
		this.unsubscribe = null;
		this.room = null;
		if (this.heardTimer) clearTimeout(this.heardTimer);
		this.heardTimer = null;
		if (this.role === 'listener') void loadApp().then((a) => a.player.setListening(null));
	}

	private reset(): void {
		this.me = null;
		this.hostId = null;
		this.members = [];
		this.info = null;
		this.track = null;
		this.trackKey = null;
		this.stats = null;
		this.queue = [];
		this.feed = [];
		this.floats = [];
		this.waiting = [];
		this.savedKey = null;
		this.role = 'listener';
		this.requests = [];
		this.startedAt = Date.now();
	}
}

export const session = new SessionState();

/** Live now, Upcoming and the stations this browser follows, for routes/sessions. */
class SessionsDirectory {
	live = $state<LiveSession[] | null>(null);
	upcoming = $state<UpcomingStation[] | null>(null);
	following = $state<FollowedStation[] | null>(null);
	/** null until asked; false when Sessions aren't on in this player yet. */
	on = $state<boolean | null>(null);
	/** Off because Stash is offline here (not because Sessions aren't on): it is asked again each time. */
	offline = $state(false);
	joined = $state<boolean | null>(null);
	loading = $state(false);
	failed = $state<string | null>(null);
	private at = 0;

	async load(opts: { ifStale?: boolean } = {}): Promise<void> {
		if (this.loading || (opts.ifStale && Date.now() - this.at < 30_000)) return;
		this.loading = true;
		try {
			if (this.on !== true) {
				const st = await client.status();
				this.on = st.on;
				this.offline = st.offline;
			}
			if (!this.on) return;
			const key = await community.key();
			this.joined = !!key;
			const [live, upcoming, following] = await Promise.all([client.live(key), client.upcoming(key), key ? client.following(key) : Promise.resolve(null)]);
			this.live = live;
			this.upcoming = upcoming;
			this.following = following;
			this.failed = null;
			this.at = Date.now();
		} catch (e) {
			if (e instanceof SessionsProblem && e.code === 'off') this.on = false;
			else this.failed = sessionsMessage(e);
		} finally {
			this.loading = false;
		}
	}
}

export const directory = new SessionsDirectory();
