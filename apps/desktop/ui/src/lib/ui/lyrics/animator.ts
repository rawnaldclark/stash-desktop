/**
 * The lyrics animator: Web Animations whose startTime is pinned to the engine clock (`now()` from the player API).
 * Compositor-only (transform and opacity), no animated filter or blur: the distance blur is made of static pre-blurred copies
 * that cross-fade by opacity. Animations exist only for the lines near the active one and are dropped when a line is two behind.
 *
 * Pinning: while playing, animation.startTime = timeline.now - (songTime - track.t0); while paused, currentTime = songTime - t0.
 * Re-pinned on play, pause, seek, resize, visibility change, and when the 4 Hz check finds more than 30 ms of drift.
 */
import { activeIndex, columnTrack, FADE_MS, glowFrames, layerTrack, letterTrack, liftFrames, spanTrack, sweepFrames, LAYERS, type LineModel, type Track } from './model';

export type AnimatorOptions = {
	/** The moving column (`.lc`): lines are absolutely positioned in it at top = i * pitch. */
	column: HTMLElement;
	/** The clipped viewport (`.lyr`). */
	viewport: HTMLElement;
	lines: LineModel[];
	/** Engine clock in ms. */
	now: () => number;
	/** Playback rate (default 1); animations run at this rate so song time and animation time stay the same. */
	rate?: () => number;
	reduced: boolean;
	/** The held-word glow (viewport >= 768 px, motion allowed). */
	glow: boolean;
};

type Pin = { a: Animation; t0: number };
type LineAnims = { j: number; layers: Pin[]; words: Pin[]; wordsBuilt: boolean; statics: HTMLElement[]; sung: boolean; el: HTMLElement };

export const BUDGET = 150;
const DRIFT_MS = 30;
/** Word animations exist only while their line is live: from LEAD ms before it starts to TAIL ms after its last word. Fewer animated
 *  elements = a cheaper style pass on every main-thread frame (the app has a few a second: clock, progress, line changes). */
const LEAD_MS = 1300;
const TAIL_MS = 100;
/** update() runs about once a second (in the same frame as the clock text), so everything it builds must cover the next LOOK_MS. */
const LOOK_MS = 1500;

export class LyricAnimator {
	private readonly o: AnimatorOptions;
	private readonly lines = new Map<number, LineAnims>();
	private col: Pin | null = null;
	private pitch = 0;
	private anchor = 0;
	/** A line's layers change when it is 2 lines ahead of the active one (far dim -> near dim) and stay put beyond that, so 3 lines ahead of
	 *  where the song will be (see LOOK_MS) is enough: lines further ahead hold their static far-dim copy (fewer running animations). */
	private readonly ahead = 3;
	private playing = false;
	private dead = false;

	constructor(opts: AnimatorOptions) {
		this.o = opts;
		this.ends = opts.lines.map((line) => Math.max(line.end, ...line.words.map((w) => w.b)));
		this.measure();
	}

	/** Animations alive right now (for tests and the budget). */
	get count(): number {
		let n = this.col ? 1 : 0;
		for (const l of this.lines.values()) n += l.layers.length + l.words.length;
		return n;
	}

	/** Reads pitch (1.7 em) and the anchor from the laid-out column and rebuilds the column animation. */
	measure(): void {
		const { column, viewport, lines } = this.o;
		const fs = parseFloat(getComputedStyle(column).fontSize) || 24;
		const cs = getComputedStyle(viewport);
		const h = viewport.clientHeight || 600;
		const pct = parseFloat(cs.getPropertyValue('--anchor-pct')) || 18;
		this.pitch = fs * 1.7;
		this.anchor = Math.max(1.2 * fs, (pct / 100) * h);
		if (lines.length) this.buildColumn();
	}

	/** The index range that gets animations: two lines behind to the last visible line ahead of where the song will be LOOK_MS from now (the DOM holds more). */
	windowFor(active: number, future = active): [number, number] {
		const n = this.o.lines.length;
		return [Math.max(0, active - 2), Math.min(n - 1, future + this.ahead)];
	}

	private lineEl(j: number): HTMLElement | null {
		return this.o.column.querySelector<HTMLElement>(`[data-i="${j}"]`);
	}

	/** The end of line j's last word, precomputed (the 4 Hz tick asks for it per line). */
	private readonly ends: number[];

	private pinOne(p: Pin, t: number) {
		if (this.playing) {
			const r = this.o.rate?.() ?? 1;
			p.a.playbackRate = r;
			p.a.startTime = (document.timeline.currentTime as number) - (t - p.t0) / r;
		} else {
			p.a.pause();
			p.a.currentTime = t - p.t0;
		}
	}

	private make(el: Element, tr: Track, extra: KeyframeAnimationOptions = {}): Pin {
		const a = el.animate(tr.frames, { duration: tr.duration, fill: 'both', easing: 'linear', ...extra });
		const p = { a, t0: tr.t0 };
		this.pinOne(p, this.o.now());
		return p;
	}

	private buildColumn() {
		this.col?.a.cancel();
		const tr = columnTrack(this.o.lines, this.anchor, this.pitch, this.o.reduced);
		this.col = this.make(this.o.column, tr);
	}

	/**
	 * A layer whose opacity never changes across its track is NOT animated: the compositor refuses an animation with no visible change
	 * (compositeFailed 131072), and one animation that is not composited makes every frame a main-thread frame. It holds a static opacity.
	 */
	private buildLayers(j: number): { pins: Pin[]; statics: HTMLElement[] } {
		const el = this.lineEl(j);
		const pins: Pin[] = [];
		const statics: HTMLElement[] = [];
		if (!el) return { pins, statics };
		for (const layer of LAYERS) {
			const target = el.querySelector<HTMLElement>(`[data-layer="${layer}"]`);
			if (!target) continue;
			const tr = layerTrack(this.o.lines, layer, j, this.o.reduced);
			const first = tr.frames[0].opacity;
			if (tr.frames.every((f) => f.opacity === first)) {
				target.style.opacity = String(first);
				statics.push(target);
			} else pins.push(this.make(target, tr));
		}
		return { pins, statics };
	}

	private buildWords(j: number): Pin[] {
		if (this.o.reduced) return [];
		const el = this.lineEl(j);
		const line = this.o.lines[j];
		if (!el || !line) return [];
		el.querySelectorAll('.w.lit').forEach((w) => w.classList.remove('lit'));
		const out: Pin[] = [];
		// every word animation spans the whole time its line can be near the active one (see spanTrack), so none starts or ends mid-song
		const n = this.o.lines.length;
		const from = this.o.lines[Math.max(0, j - 3)].start;
		const to = Math.max(this.o.lines[Math.min(n - 1, j + 3)].start + 1500, line.end + 1500, ...line.words.map((w) => w.b + 1500));
		const wEls = el.querySelectorAll<HTMLElement>('[data-layer="ls"] .w');
		const room = () => BUDGET - this.count - out.length;
		// 1. the essentials: the sweep (two copies) or the held word's letters. A word that does not fit stays lit (no animation).
		const extras: (() => void)[] = [];
		line.words.forEach((w, wi) => {
			const we = wEls[wi];
			if (!we) return;
			const dur = Math.max(40, w.b - w.a);
			const win = { t0: w.a, duration: dur };
			const letters = w.held ? we.querySelectorAll<HTMLElement>('.lt') : null;
			const need = letters ? letters.length : 2;
			if (need > room()) {
				we.classList.add('lit');
				return;
			}
			if (letters) letters.forEach((le, k) => out.push(this.make(le, spanTrack(letterTrack(w, k, letters.length, true), from, to))));
			else {
				const wl = we.querySelector('.wl');
				const wlt = we.querySelector('.wlt');
				if (wl) out.push(this.make(wl, spanTrack({ ...win, frames: sweepFrames(w.rtl ? 1 : -1) }, from, to)));
				if (wlt) out.push(this.make(wlt, spanTrack({ ...win, frames: sweepFrames(w.rtl ? -1 : 1) }, from, to)));
			}
			// 2. the extras (lift, glow), only if there is room left afterwards
			extras.push(() => {
				if (room() < 1) return;
				out.push(this.make(we, spanTrack({ ...win, frames: liftFrames() }, from, to, 'ease-out')));
				const g = we.querySelector('.wg');
				if (w.held && g && this.o.glow && room() >= 1) out.push(this.make(g, spanTrack({ ...win, frames: glowFrames() }, from, to)));
			});
		});
		for (const f of extras) f();
		return out;
	}

	private drop(l: LineAnims, which: 'layers' | 'words' | 'all') {
		if (which === 'all') for (const e of l.statics) e.style.removeProperty('opacity');
		if (which !== 'words') for (const p of l.layers) p.a.cancel();
		if (which !== 'layers') for (const p of l.words) p.a.cancel();
		if (which !== 'words') l.layers = [];
		if (which !== 'layers') {
			l.words = [];
			l.wordsBuilt = false;
			l.sung = false;
			l.el.querySelectorAll('.w.lit').forEach((w) => w.classList.remove('lit'));
		}
	}

	private lastB(j: number): number {
		return this.ends[j];
	}

	private live(j: number, t: number): boolean {
		return t >= this.o.lines[j].start - LEAD_MS && t <= this.lastB(j) + TAIL_MS;
	}

	private markSung(l: LineAnims) {
		l.sung = true;
		l.el.querySelectorAll('.w').forEach((w) => w.classList.add('lit'));
	}

	/** A line left its live window: past its end its words are cancelled and left lit (no animation to resample); before its start (a seek back) they just drop. */
	private finishWords(l: LineAnims, j: number, t: number) {
		this.drop(l, 'words');
		if (t > this.o.lines[j].start) this.markSung(l);
	}

	/** Builds what the active line needs, drops what it no longer does. Call after DOM updates and on every tick. */
	update(playing: boolean): void {
		if (this.dead || !this.o.lines.length) return;
		const wasPlaying = this.playing;
		this.playing = playing;
		const { lines } = this.o;
		const t = this.o.now();
		const act = activeIndex(lines, t);
		const fut = activeIndex(lines, t + LOOK_MS);
		// a line two behind keeps its layers until its last cross-fade (150 + 150 ms after the line that makes it far) is over
		const back = activeIndex(lines, t - 2 * FADE_MS - 50);
		const [from, to] = this.windowFor(back, fut);
		for (const [j, l] of this.lines) {
			const gone = j < from || j > to || !l.el.isConnected;
			if (gone) {
				this.drop(l, 'all');
				this.lines.delete(j);
				continue;
			}
			if (l.wordsBuilt && !this.live(j, t)) this.finishWords(l, j, t);
		}
		for (let j = from; j <= to; j++) {
			if (this.lines.has(j)) continue;
			const { pins, statics } = this.buildLayers(j);
			if (pins.length || statics.length) this.lines.set(j, { j, layers: pins, words: [], wordsBuilt: false, statics, sung: false, el: this.lineEl(j)! });
		}
		// words: only for lines that are live now or within LEAD_MS (the active line first, then the ones ahead, then the one just sung)
		const order: number[] = [act];
		for (let j = act + 1; j <= fut + 2; j++) order.push(j);
		order.push(act - 1);
		for (const j of order) {
			const l = this.lines.get(j);
			if (l && !l.wordsBuilt && this.live(j, t) && this.count < BUDGET) {
				l.sung = false;
				l.words = this.buildWords(j);
				l.wordsBuilt = true;
			}
		}
		// lines already sung that hold no animation (after a seek, or just finished) show their words lit
		for (const [j, l] of this.lines) {
			if (l.wordsBuilt) continue;
			if (!l.sung && t > this.lastB(j) + TAIL_MS) this.markSung(l);
			else if (l.sung && t < lines[j].start - LEAD_MS) this.drop(l, 'words'); // a seek back past it
		}
		if (wasPlaying !== playing) this.repin();
		else if (playing && this.col && this.drift() > DRIFT_MS) this.repin();
	}

	/** Absolute drift of the column animation from the engine clock, in ms. */
	drift(): number {
		if (!this.col || !this.playing) return 0;
		const ct = Number(this.col.a.currentTime ?? 0);
		return Math.abs(ct - (this.o.now() - this.col.t0));
	}

	/** Re-pins everything to the engine clock (play, pause, seek, resize, visibility). */
	repin(): void {
		if (this.dead) return;
		const t = this.o.now();
		if (this.col) this.pinOne(this.col, t);
		for (const l of this.lines.values()) {
			for (const p of l.layers) this.pinOne(p, t);
			for (const p of l.words) this.pinOne(p, t);
		}
	}

	/** After a seek the active line changes a lot: drop everything and rebuild from scratch. */
	reset(playing: boolean): void {
		for (const l of this.lines.values()) this.drop(l, 'all');
		this.lines.clear();
		this.update(playing);
		this.repin();
	}

	destroy(): void {
		this.dead = true;
		for (const l of this.lines.values()) this.drop(l, 'all');
		this.lines.clear();
		this.col?.a.cancel();
		this.col = null;
	}
}
