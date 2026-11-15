import type { Artwork, LibraryItem } from '../api';

/** m:ss (or h:mm:ss) from milliseconds. */
export function mmss(ms: number | undefined | null): string {
	if (ms === undefined || ms === null || !Number.isFinite(ms) || ms < 0) return '0:00';
	const total = Math.floor(ms / 1000);
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}` : `${m}:${String(s).padStart(2, '0')}`;
}

/** "-m:ss" remaining. */
export const remaining = (pos: number, dur: number) => '-' + mmss(Math.max(0, dur - pos));

/** The smallest artwork at least `min` px wide, else the largest, else the first. */
export function pickArt(art: Artwork[] | undefined, min = 0): string | undefined {
	if (!art?.length) return undefined;
	const sized = art.filter((a) => a.width).sort((a, b) => (a.width ?? 0) - (b.width ?? 0));
	if (!sized.length) return art[0].url;
	return (sized.find((a) => (a.width ?? 0) >= min) ?? sized[sized.length - 1]).url;
}

export const coverOf = (item: Pick<LibraryItem, 'artwork'> | null | undefined, min = 0) => pickArt(item?.artwork, min);

/** "3 songs" / "1 song". */
export const songs = (n: number) => `${n} ${n === 1 ? 'song' : 'songs'}`;

/** Total duration like "20 min" or "1 hr 5 min". */
export function totalLength(items: { durationMs?: number }[]): string {
	const min = Math.round(items.reduce((a, i) => a + (i.durationMs ?? 0), 0) / 60000);
	if (min < 60) return `${min} min`;
	return `${Math.floor(min / 60)} hr ${min % 60} min`;
}

/** Stable hue 0-359 from a string (for cover placeholders). */
export function hueOf(s: string): number {
	let h = 0;
	for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
	return h;
}

/** "Today", "Yesterday" or a short date, for History. */
export function dayLabel(ts: number, now = Date.now()): string {
	const d = new Date(ts);
	const n = new Date(now);
	const day = (x: Date) => Math.floor(new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime() / 86_400_000);
	const diff = day(n) - day(d);
	if (diff <= 0) return 'Today';
	if (diff === 1) return 'Yesterday';
	return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: d.getFullYear() === n.getFullYear() ? undefined : 'numeric' });
}
