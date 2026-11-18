import { describe, expect, it } from 'vitest';
import type { Signal } from '../api';
import { formatLabel, origin, outputSentence, verdict } from './signal';

const sig = (over: Partial<Signal> & { decoded?: Signal['decoded'] } = {}): Signal => ({
	codec: 'flac',
	decoded: { sample_rate: 44100, format: 's16' },
	output: { sample_rate: 44100, format: 's16' },
	driver: 'wasapi',
	mode: 'shared',
	resampled: false,
	converted: false,
	scaled: false,
	equalized: false,
	...over
});

describe('formatLabel (the format badge)', () => {
	it('lossless: codec, then bit depth and sample rate', () => {
		expect(formatLabel(sig())).toBe('FLAC · 16-bit/44.1 kHz');
		expect(formatLabel(sig({ decoded: { sample_rate: 96000, format: 's32' } }), 24)).toBe('FLAC · 24-bit/96 kHz');
		expect(formatLabel(sig({ decoded: { sample_rate: 96000, format: 's32' } }), 32)).toBe('FLAC · 32-bit/96 kHz');
		expect(formatLabel(sig({ decoded: { sample_rate: 96000, format: 's32' } }))).toBe('FLAC · 32-bit/96 kHz');
		expect(formatLabel(sig({ codec: 'pcm_s16le' }))).toBe('PCM · 16-bit/44.1 kHz');
		expect(formatLabel(sig({ codec: 'pcm_s24le', decoded: { sample_rate: 96000, format: 's32' } }))).toBe('PCM · 24-bit/96 kHz');
		expect(formatLabel(sig({ codec: 'pcm_f32le', decoded: { sample_rate: 48000, format: 'float' } }))).toBe('PCM · 32-bit float/48 kHz');
		expect(formatLabel(sig({ codec: 'alac', decoded: { sample_rate: null, format: 's16p' } }))).toBe('ALAC · 16-bit');
	});
	it('lossy: codec and sample rate, never the decoder’s own sample format', () => {
		expect(formatLabel(sig({ codec: 'mp3', decoded: { sample_rate: 44100, format: 'floatp' } }))).toBe('MP3 · 44.1 kHz');
		expect(formatLabel(sig({ codec: 'opus', decoded: { sample_rate: 48000, format: 'floatp' } }))).toBe('Opus · 48 kHz');
	});
	it('hidden when the codec is not known', () => {
		expect(formatLabel(null)).toBeNull();
		expect(formatLabel(sig({ codec: null }))).toBeNull();
	});
});

describe('verdict', () => {
	it('Bit-perfect: the sound card for Stash alone, nothing changed', () => {
		expect(verdict(sig({ mode: 'exclusive' }))).toEqual({ word: 'Bit-perfect', color: '#6EE7B7' });
	});
	it('Lossless: a lossless codec that is not bit-perfect and not resampled', () => {
		expect(verdict(sig())).toEqual({ word: 'Lossless', color: '#93C5FD' });
		expect(verdict(sig({ mode: 'exclusive', converted: true }))?.word).toBe('Lossless');
		// The volume below 100, or ReplayGain, makes the samples louder or softer: not bit-perfect.
		expect(verdict(sig({ mode: 'exclusive', scaled: true }))?.word).toBe('Lossless');
		expect(verdict(sig({ codec: 'pcm_s24le' }))?.word).toBe('Lossless');
	});
	it('Resampled: the sample rate is changed on the way', () => {
		expect(verdict(sig({ resampled: true }))).toEqual({ word: 'Resampled', color: '#FCD34D' });
		expect(verdict(sig({ mode: 'exclusive', resampled: true }))?.word).toBe('Resampled');
	});
	it('Lossy: a lossy codec, whatever happens after it', () => {
		expect(verdict(sig({ codec: 'mp3' }))).toEqual({ word: 'Lossy', color: '#9CA3AF' });
		expect(verdict(sig({ codec: 'aac', mode: 'exclusive' }))?.word).toBe('Lossy');
		expect(verdict(sig({ codec: 'opus', resampled: true }))?.word).toBe('Lossy');
	});
	it('nothing when there is no signal', () => {
		expect(verdict(null)).toBeNull();
	});
	it('Lossless + your EQ: the equalizer changes the samples, so never Bit-perfect', () => {
		expect(verdict(sig({ mode: 'exclusive', equalized: true }))).toEqual({ word: 'Lossless + your EQ', color: '#C4B5FD' });
		expect(verdict(sig({ equalized: true }))?.word).toBe('Lossless + your EQ');
		expect(verdict(sig({ mode: 'exclusive', scaled: true, equalized: true }))?.word).toBe('Lossless + your EQ');
	});
	it('the equalizer does not hide a lossy codec or a resampled track', () => {
		expect(verdict(sig({ codec: 'mp3', equalized: true }))?.word).toBe('Lossy');
		expect(verdict(sig({ codec: 'mp3', mode: 'exclusive', equalized: true }))?.word).toBe('Lossy');
		expect(verdict(sig({ resampled: true, equalized: true }))?.word).toBe('Resampled');
		expect(verdict(sig({ mode: 'exclusive', resampled: true, equalized: true }))?.word).toBe('Resampled');
	});
});

describe('origin (an add-on’s stream)', () => {
	it('names the add-on and the tier it claimed', () => {
		expect(origin('Demo folder', 'lossless', sig())).toEqual({ text: 'Demo folder · lossless', disagrees: false });
		expect(origin('Demo folder', 'hires', sig({ decoded: { sample_rate: 96000, format: 's32' } }))).toEqual({ text: 'Demo folder · hi-res', disagrees: false });
		expect(origin('Demo folder', null, sig())).toEqual({ text: 'Demo folder', disagrees: false });
		expect(origin(null, 'lossless', sig())).toBeNull();
	});
	it('says so when the sound is not what the add-on claimed', () => {
		expect(origin('Demo folder', 'lossless', sig({ codec: 'mp3' }))).toEqual({ text: 'Demo folder said lossless; this is MP3', disagrees: true });
		expect(origin('Demo folder', 'hires', sig())).toEqual({ text: 'Demo folder said hi-res; this is 16-bit 44.1 kHz', disagrees: true });
	});
});

describe('outputSentence', () => {
	it('says what the sound card gets', () => {
		expect(outputSentence(sig({ mode: 'exclusive' }))).toBe('Now: 44.1 kHz · unchanged');
		expect(outputSentence(sig({ resampled: true, output: { sample_rate: 48000, format: 'float' } }))).toBe('Now: 44.1 kHz in, 48 kHz out · resampled');
		expect(outputSentence(null)).toBeNull();
	});
	it('names the equalizer when it is the only change', () => {
		expect(outputSentence(sig({ mode: 'exclusive', equalized: true }))).toBe('Now: 44.1 kHz · your equalizer');
		expect(outputSentence(sig({ scaled: true, equalized: true }))).toBe('Now: 44.1 kHz · changed by volume or format');
	});
});
