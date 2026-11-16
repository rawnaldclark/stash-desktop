import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { writable } from 'svelte/store';
import type { Counts, CreditPart } from '../core';

// The core is a stand-in: each ask is held until the test answers it (or fails it).
type Ask = { names: string[]; answer: (credits: Record<string, CreditPart[]>) => void; fail: () => void };
const asks: Ask[] = [];
const counts = writable<Counts | null>(null);

vi.mock('../core', () => ({
	credits: (names: string[]) =>
		new Promise((resolve, reject) => {
			asks.push({ names, answer: (credits) => resolve({ credits }), fail: () => reject(new Error('down')) });
		})
}));
vi.mock('../app', () => ({ library: { counts: { subscribe: counts.subscribe } } }));

const { creditOf } = await import('./credits.svelte');

const split = (credit: string): CreditPart[] =>
	credit.split(' & ').map((name, i) => ({ name, before: i ? ' & ' : '', after: '', id: i + 1 }));
const answerAll = (ask: Ask) => ask.answer(Object.fromEntries(ask.names.map((n) => [n, split(n)])));
/** Lets the stand-in's answer reach the cache. */
const settle = () => vi.advanceTimersByTimeAsync(0);

let n = 0;
/** A credit no test has asked about yet (the cache is shared by the tests). */
const fresh = () => `Ada ${++n} & Ben ${n}`;

beforeEach(() => {
	vi.useFakeTimers();
	asks.length = 0;
	counts.set({ tracks: 10, albums: 2, artists: 3, liked: 0, playlists: 0 });
});
// Each test ends with nothing asked or waiting to be (a timer of the fake clock would never fire after it).
afterEach(async () => {
	for (let round = 0; round < 20; round++) {
		await vi.advanceTimersByTimeAsync(AGAIN);
		for (const ask of asks.splice(0)) answerAll(ask);
		await settle();
	}
	vi.useRealTimers();
});
/** Longer than the wait after a failure. */
const AGAIN = 6_000;

describe('the credits cache', () => {
	it('asks once for the credits shown together, and keeps what was said', async () => {
		const [a, b] = [fresh(), fresh()];
		expect(creditOf(a)).toEqual([]);
		expect(creditOf(b)).toEqual([]);
		expect(creditOf('Drake')).toEqual([]);
		await vi.advanceTimersByTimeAsync(30);
		expect(asks.map((x) => x.names)).toEqual([[a, b]]);
		answerAll(asks[0]);
		await settle();
		expect(creditOf(a).map((p) => p.name)).toEqual(a.split(' & '));
		await vi.advanceTimersByTimeAsync(10_000);
		expect(asks.length).toBe(1);
	});

	it('asks again 5 s after an ask failed, also for a credit wanted meanwhile', async () => {
		const [a, b] = [fresh(), fresh()];
		creditOf(a);
		await vi.advanceTimersByTimeAsync(30);
		// Wanted while the first ask is out.
		creditOf(b);
		asks[0].fail();
		await settle();
		await vi.advanceTimersByTimeAsync(4_900);
		expect(asks.length).toBe(1);
		await vi.advanceTimersByTimeAsync(200);
		expect(asks.length).toBe(2);
		expect([...asks[1].names].sort()).toEqual([a, b].sort());
		answerAll(asks[1]);
		await settle();
		expect(creditOf(b).length).toBe(2);
	});

	it('lets go of an answer asked for before the library changed, and asks again', async () => {
		const a = fresh();
		creditOf(a);
		await vi.advanceTimersByTimeAsync(30);
		counts.set({ tracks: 11, albums: 2, artists: 3, liked: 0, playlists: 0 });
		answerAll(asks[0]);
		await settle();
		expect(creditOf(a)).toEqual([]);
		await vi.advanceTimersByTimeAsync(30);
		expect(asks.length).toBe(2);
		answerAll(asks[1]);
		await settle();
		expect(creditOf(a).length).toBe(2);
		// What was said is let go when the library changes.
		counts.set({ tracks: 11, albums: 3, artists: 3, liked: 0, playlists: 0 });
		expect(creditOf(a)).toEqual([]);
	});

	it('keeps more credits than a long list shows', async () => {
		const many = Array.from({ length: 4_500 }, () => fresh());
		for (const credit of many) creditOf(credit);
		// 400 a time: twelve asks.
		for (let round = 0; round < 20; round++) {
			await vi.advanceTimersByTimeAsync(30);
			const open = asks.splice(0);
			for (const ask of open) answerAll(ask);
			await settle();
		}
		expect(many.every((credit) => creditOf(credit).length === 2)).toBe(true);
	});
});
