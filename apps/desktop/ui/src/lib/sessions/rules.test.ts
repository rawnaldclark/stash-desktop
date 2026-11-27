import { describe, expect, it } from 'vitest';
import { cleanOwnStation, cleanStation, cleanStationInput, cleanStationPage, isRoomCode, isStationId } from './rules';

const STATION = { id: 'St12ab34', name: 'Desk jazz', hostName: null, vibe: 'quiet, for working', up: 7, down: 0, followers: 3 };

describe('stations', () => {
	it('ids and room codes are the service’s alphabets', () => {
		expect(isStationId('St12ab34')).toBe(true);
		expect(isStationId('St12ab3')).toBe(false);
		expect(isRoomCode('ABCDEFGH')).toBe(true);
		expect(isRoomCode('ABCDEFG0')).toBe(false);
		expect(isRoomCode('abcdefgh')).toBe(false);
	});

	it('a summary keeps known fields, drops control characters, and fails without a name or counts', () => {
		expect(cleanStation({ ...STATION, owner: 'x', name: ' Desk‮jazz ' })).toEqual({ ...STATION, name: 'Deskjazz' });
		expect(cleanStation({ ...STATION, name: '' })).toBeNull();
		expect(cleanStation({ ...STATION, up: -1 })).toBeNull();
		expect(cleanStation({ ...STATION, myVote: 1, following: true, mine: 'yes' })).toEqual({ ...STATION, myVote: 1, following: true });
	});

	it("the owner's view needs its settings and standing", () => {
		const own = { ...STATION, autoLive: 'schedule', schedule: { tz: 'Europe/London', windows: [{ day: 1, start: '22:00', end: '01:00' }] }, hidden: false, removed: false, lastLiveAt: null, deleteAt: 1_800_000_000_000 };
		expect(cleanOwnStation(own)).toEqual(own);
		expect(cleanOwnStation({ ...own, autoLive: 'sometimes' })).toBeNull();
	});

	it('a page keeps its hours and its session; a live track keeps only a shared song’s fields', () => {
		const page = cleanStationPage({ station: { ...STATION, schedule: null, next: null }, live: { room: 'ABCDEFGH', track: { t: 'Blue in Green', a: 'Miles Davis', by: 'M2345678', x: 1 }, listeners: 4, paused: true, full: false } });
		expect(page).toEqual({ station: { ...STATION, schedule: null, next: null }, live: { room: 'ABCDEFGH', track: { t: 'Blue in Green', a: 'Miles Davis' }, listeners: 4, paused: true, full: false } });
	});
});

describe('a station change from the browser', () => {
	it('is rebuilt from known fields; blank clears; a bad field refuses it all', () => {
		expect(cleanStationInput({ name: ' Basement ', vibe: '  ', hostName: null, extra: 1 }, true)).toEqual({ name: 'Basement', vibe: null, hostName: null });
		expect(cleanStationInput({ vibe: 'v' }, true)).toBeNull();
		expect(cleanStationInput({ vibe: 'v' }, false)).toEqual({ vibe: 'v' });
		expect(cleanStationInput({ name: 'x', autoLive: 'never' }, true)).toBeNull();
		expect(cleanStationInput({ name: 'x', vibe: 'v'.repeat(61) }, true)).toBeNull();
		expect(cleanStationInput({}, false)).toBeNull();
	});
});
