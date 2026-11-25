/** Library › Statistics: the wording and the scales of its lists and charts, apart from the screen so they are tested. */
import type { StatBar } from '../../core';

/** A clean top for an axis that reaches `max`: 1, 2 or 5 times a power of ten, at least `max`. */
export function niceTop(max: number): number {
	if (max <= 0) return 1;
	const power = 10 ** Math.floor(Math.log10(max));
	for (const step of [1, 2, 5, 10]) if (step * power >= max) return step * power;
	return 10 * power;
}

/** An axis tick in full under ten thousand, short past it: 0, 2,000, 15K. */
export function tick(n: number): string {
	if (n >= 10_000) return `${Math.round(n / 1000).toLocaleString()}K`;
	return n.toLocaleString();
}

/** A share as a whole percent, never "100%" while something is missing, nor "0%" while something is there. */
export function percent(have: number, of: number): string {
	if (of <= 0) return '0%';
	const p = (have / of) * 100;
	if (have > 0 && p < 1) return '<1%';
	if (have < of && p > 99) return '>99%';
	return `${Math.round(p)}%`;
}

/** "2026-11" as a column's short label and its full name: "Nov", "November 2026". */
export function monthNames(key: string): { short: string; full: string } {
	const [y, m] = key.split('-').map(Number);
	if (!y || !m) return { short: key, full: key };
	const at = new Date(y, m - 1, 1);
	return {
		short: at.toLocaleString(undefined, { month: 'short' }),
		full: at.toLocaleString(undefined, { month: 'long', year: 'numeric' })
	};
}

/** Where a row of a list leads: a genre, decade or artist opens its songs; a song plays where it is. */
export function rowHref(list: 'genres' | 'decades' | 'artists', bar: StatBar): string | undefined {
	if (bar.key === undefined || bar.key === null) return undefined;
	if (list === 'genres') return `/library?tab=songs&genre=${encodeURIComponent(bar.key)}`;
	if (list === 'decades') return bar.key ? `/library?tab=songs&decade=${bar.key}` : undefined;
	return `/artist/library/${encodeURIComponent(bar.key)}`;
}

/** A long time in whole hours, as a tile shows it: "27 hours", "under an hour". */
export function hours(ms: number): string {
	const h = Math.round(ms / 3_600_000);
	if (h < 1) return 'under an hour';
	return `${h.toLocaleString()} ${h === 1 ? 'hour' : 'hours'}`;
}

/** Whole hours as a big number shows them, its word beside it ("hours of music"): "27", "1,204", "<1" (some, under an
 * hour), "0" (none). */
export function wholeHours(ms: number): string {
	if (ms <= 0) return '0';
	const h = Math.round(ms / 3_600_000);
	return h < 1 ? '<1' : h.toLocaleString();
}

/** The months from the first one with plays on: the ones before it are from before any listening here. */
export function fromFirstPlay<T extends { plays: number }>(months: T[]): T[] {
	const first = months.findIndex((m) => m.plays > 0);
	return first < 0 ? [] : months.slice(first);
}

const count = (n: number, one: string, many: string) => `${n.toLocaleString()} ${n === 1 ? one : many}`;
export const songsWord = (n: number) => count(n, 'song', 'songs');
export const playsWord = (n: number) => count(n, 'play', 'plays');
