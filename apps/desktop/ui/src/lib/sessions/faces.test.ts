import { describe, expect, it } from 'vitest';
import { FACES, faceFrames, faceFromCommand, faceName, faceText, isBigFace, isFace } from './faces';

describe('text faces', () => {
	it("are the share Worker's 17, each drawn differently and named differently, the last five big", () => {
		expect(FACES).toHaveLength(17);
		expect(FACES.every((f) => f.startsWith('k:'))).toBe(true);
		expect(new Set(FACES.map(faceText)).size).toBe(17);
		expect(new Set(FACES.map(faceName)).size).toBe(17);
		expect(FACES.filter(isBigFace)).toEqual(['k:tableflip', 'k:unflip', 'k:dance', 'k:rockon', 'k:sparkle']);
		expect(FACES.filter(isBigFace).every((f) => faceFrames(f)!.length === 2)).toBe(true);
	});

	it('draw as their text, named as their command; an emoji or an unknown id is nothing', () => {
		expect(faceText('k:shrug')).toBe('¯\\_(ツ)_/¯');
		expect(faceText('k:tableflip')).toBe('(╯°□°)╯︵ ┻━┻');
		expect(faceName('k:shrug')).toBe('/shrug');
		expect(faceFrames('🔥')).toBeNull();
		expect(faceText('k:from-next-year')).toBeNull();
		expect(isFace('toString')).toBe(false);
	});

	it('a typed command picks a face: whole, without the slash, or a start only one face has', () => {
		expect(faceFromCommand('/shrug')).toBe('k:shrug');
		expect(faceFromCommand(' Shrug ')).toBe('k:shrug');
		expect(faceFromCommand('/tab')).toBe('k:tableflip');
		expect(faceFromCommand('/c')).toBeNull(); // cool and cry
		expect(faceFromCommand('/nope')).toBeNull();
		expect(faceFromCommand('/')).toBeNull();
	});
});
