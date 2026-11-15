import { describe, expect, it } from 'vitest';
import { removes } from './swipe';

describe('swipe to remove', () => {
	it('by distance only: past half the row, either way', () => {
		expect(removes(181, 360)).toBe(true);
		expect(removes(-181, 360)).toBe(true);
		expect(removes(180, 360)).toBe(false);
		expect(removes(90, 360)).toBe(false);
		expect(removes(500, 0)).toBe(false);
	});
});
