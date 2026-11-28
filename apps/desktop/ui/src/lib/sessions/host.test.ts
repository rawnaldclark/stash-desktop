import { describe, expect, it } from 'vitest';
import type { QueueItem } from '../api';
import type { LibraryItem } from '../identity';
import { itemFromWire } from '../share/links';
import { HostBridge, type HostView } from './host';
import type { Frame } from './room';

const song = (title: string, artist = 'Artist'): QueueItem => ({ key: `${title}|${artist}`.toLowerCase(), title, artist, refs: {}, artwork: [], qid: `${title}#q` }) as unknown as QueueItem;

function make() {
	const sent: Frame[] = [];
	let pos = 0;
	let t = 1_000_000;
	const enqueued: { title: string; next: boolean }[] = [];
	let skips = 0;
	const player = {
		position: () => pos,
		enqueue: (items: LibraryItem[], o: { next?: boolean } = {}) => items.forEach((i) => enqueued.push({ title: i.title, next: !!o.next })),
		next: () => skips++
	};
	const b = new HostBridge(player, (m) => (sent.push(m), true), () => t);
	return {
		b,
		sent,
		enqueued,
		skips: () => skips,
		at: (p: number, dt = 0) => ((pos = p), (t += dt)),
		view: (cur: LibraryItem | null, qid: string, o: Partial<HostView> = {}): HostView => ({ cur: cur ? { qid, item: cur } : null, playing: true, durationMs: 200_000, upcoming: [], stopped: null, ...o })
	};
}

describe('hosting from this tab', () => {
	it('sends what plays at the welcome, and each new song after, with Up next and why it changed', () => {
		const h = make();
		const a = song('Redbone');
		h.at(30_000);
		h.b.observe(h.view(a, 'q1', { upcoming: [song('Ivy')] }));
		expect(h.sent).toEqual([]); // nothing before the welcome
		h.b.welcome('HOST2345', {});
		expect(h.sent[0]).toMatchObject({ t: 'load', track: { t: 'Redbone', a: 'Artist' }, positionMs: 30_000, queue: [{ t: 'Ivy' }], why: 'pick' });
		h.at(199_000, 169_000);
		h.b.observe(h.view(a, 'q1', { upcoming: [song('Ivy')] }));
		h.at(0, 1_000);
		h.b.observe(h.view(song('Ivy'), 'q2'));
		expect(h.sent.at(-1)).toMatchObject({ t: 'load', track: { t: 'Ivy' }, why: 'end' });
		h.at(5_000, 5_000);
		h.b.observe(h.view(song('Ivy'), 'q2'));
		h.at(0, 100);
		h.b.observe(h.view(song('Lost'), 'q3'));
		expect(h.sent.at(-1)).toMatchObject({ t: 'load', track: { t: 'Lost' }, why: 'skip' });
	});

	it('a reconnect to the song the room still has puts the position right instead of loading it again', () => {
		const h = make();
		const a = song('Redbone');
		h.at(40_000);
		h.b.observe(h.view(a, 'q1', { upcoming: [song('Ivy')] }));
		h.b.welcome('HOST2345', { trackKey: 3, track: { t: 'redbone', a: 'artist' }, timeline: { playing: true } });
		expect(h.sent.some((m) => m.t === 'load')).toBe(false);
		expect(h.sent).toContainEqual({ t: 'seek', positionMs: 40_000 });
		expect(h.sent).toContainEqual(expect.objectContaining({ t: 'queue' }));
		// Paused here while the room thought it played: the room hears pause.
		const p = make();
		p.at(40_000);
		p.b.observe(p.view(a, 'q1', { playing: false }));
		p.b.welcome('HOST2345', { trackKey: 3, track: { t: 'Redbone', a: 'Artist' }, timeline: { playing: true } });
		expect(p.sent[0]).toEqual({ t: 'pause' });
		expect(p.sent.some((m) => m.t === 'load' || m.t === 'seek')).toBe(false);
		// A different song in the room: this one is loaded.
		const d = make();
		d.b.observe(d.view(song('Ivy'), 'q2'));
		d.b.welcome('HOST2345', { trackKey: 3, track: { t: 'Redbone', a: 'Artist' }, timeline: { playing: true } });
		expect(d.sent[0]).toMatchObject({ t: 'load', track: { t: 'Ivy' } });
	});

	it('answers every prepare ready, at once', () => {
		const h = make();
		h.b.welcome('HOST2345', {});
		h.b.frame({ t: 'prepare', trackKey: 4 });
		expect(h.sent).toEqual([{ t: 'status', status: 'ready', trackKey: 4 }]);
	});

	it('passes on play, pause and a seek, and Up next when it changes', () => {
		const h = make();
		h.b.welcome('HOST2345', {});
		const a = song('Redbone');
		h.at(10_000);
		h.b.observe(h.view(a, 'q1'));
		h.at(12_000, 2_000);
		h.b.observe(h.view(a, 'q1', { playing: false }));
		expect(h.sent.at(-1)).toEqual({ t: 'pause' });
		h.at(12_000, 1_000);
		h.b.observe(h.view(a, 'q1'));
		expect(h.sent.at(-1)).toEqual({ t: 'play' });
		h.at(90_000, 1_000); // a jump the music didn't play through
		h.b.observe(h.view(a, 'q1'));
		expect(h.sent.at(-1)).toEqual({ t: 'seek', positionMs: 90_000 });
		h.at(91_000, 1_000);
		h.b.observe(h.view(a, 'q1', { upcoming: [song('Ivy')] }));
		expect(h.sent.at(-1)).toMatchObject({ t: 'queue', queue: [{ t: 'Ivy' }] });
		const n = h.sent.length;
		h.at(92_000, 1_000);
		h.b.observe(h.view(a, 'q1', { upcoming: [song('Ivy')] }));
		expect(h.sent.length).toBe(n); // nothing changed, nothing sent
	});

	it("adds the room's songs from someone else to its queue once; that entry, not every copy, is theirs, until it plays", () => {
		const h = make();
		h.b.welcome('HOST2345', {});
		const req = { t: 'Electric Relaxation', a: 'A Tribe Called Quest', by: 'LEE23456' };
		// The host's own copy of the same song, further down their queue.
		const copy = { ...itemFromWire(req), qid: 'own' } as QueueItem;
		h.b.observe(h.view(song('Redbone'), 'q1', { upcoming: [copy] }));
		h.b.frame({ t: 'queue', queue: [req, { t: 'Ivy', a: 'Frank Ocean', by: 'HOST2345' }] });
		h.b.frame({ t: 'queue', queue: [req] });
		expect(h.enqueued).toEqual([{ title: 'Electric Relaxation', next: true }]);
		// The player puts it next: that entry is Lee's; the host's own copy isn't.
		const asked = { ...itemFromWire(req), qid: 'req' } as QueueItem;
		h.b.observe(h.view(song('Redbone'), 'q1', { upcoming: [asked, copy] }));
		const q = h.sent.at(-1)!;
		expect(q).toMatchObject({ t: 'queue', queue: [{ t: 'Electric Relaxation', by: 'LEE23456' }, { t: 'Electric Relaxation' }] });
		expect((q.queue as { by?: string }[])[1].by).toBeUndefined();
		// It plays as Lee's pick; once it has, the later copy is nobody's.
		h.at(0, 1_000);
		h.b.observe(h.view(asked, 'req', { upcoming: [copy] }));
		expect(h.sent.findLast((m) => m.t === 'load')?.track).toMatchObject({ t: 'Electric Relaxation', by: 'LEE23456' });
		h.at(0, 1_000);
		h.b.observe(h.view(copy, 'own'));
		expect((h.sent.findLast((m) => m.t === 'load')?.track as { by?: string }).by).toBeUndefined();
		// Asked for again after it played (gone from the room's queue, then back): added again.
		h.b.frame({ t: 'queue', queue: [] });
		h.b.frame({ t: 'queue', queue: [req] });
		expect(h.enqueued).toHaveLength(2);
	});

	it("a co-host's skip skips the song it was for, not the next one", () => {
		const h = make();
		h.b.welcome('HOST2345', {});
		h.b.frame({ t: 'prepare', trackKey: 7 });
		h.b.frame({ t: 'command', action: 'skip', trackKey: 6 });
		expect(h.skips()).toBe(0);
		h.b.frame({ t: 'command', action: 'skip', trackKey: 7 });
		expect(h.skips()).toBe(1);
	});
});
