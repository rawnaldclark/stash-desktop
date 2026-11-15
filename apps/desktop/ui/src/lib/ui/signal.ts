/**
 * Format and signal path: what the song playing is, and what happens to it on the way to the sound card, in the words and colours
 * of the Stash Android app. Everything here is read from what the core's audio engine reports (`signal`), not from a file's label.
 */
import type { Signal, Tier } from '../api';

const CODECS: Record<string, string> = { flac: 'FLAC', alac: 'ALAC', mp3: 'MP3', aac: 'AAC', opus: 'Opus', vorbis: 'Vorbis', wavpack: 'WavPack', ape: 'APE' };
const LOSSLESS = ['flac', 'alac', 'wavpack', 'ape'];
const LOSSY = ['mp3', 'mp2', 'aac', 'opus', 'vorbis', 'wma', 'ac3', 'eac3'];

const isPcm = (codec: string) => codec.startsWith('pcm');
export const isLossless = (codec: string | null | undefined): boolean => !!codec && (isPcm(codec) || LOSSLESS.includes(codec));
export const isLossy = (codec: string | null | undefined): boolean => !!codec && LOSSY.includes(codec);

/** "flac" → "FLAC", "pcm_s16le" → "PCM"; an unknown codec in capitals. */
export function codecName(codec: string | null | undefined): string {
	if (!codec) return '';
	if (isPcm(codec)) return 'PCM';
	return CODECS[codec] ?? codec.toUpperCase();
}

/** "44.1 kHz", "96 kHz". */
export const kHz = (rate: number | null | undefined): string => (rate ? `${Number((rate / 1000).toFixed(1))} kHz` : '');

/**
 * The bit depth, from the format the engine decodes to: s16 is 16-bit; s32 carries a 24-bit file as well as a 32-bit one, so it is
 * 24-bit when the file is known to be (the library says so: `sourceBitDepth`, or the codec's own name does: `pcm_s24le`) and 32-bit
 * otherwise; float is 32-bit float.
 */
export function bitDepth(format: string | null | undefined, sourceBitDepth?: number | null, codec?: string | null): string {
	const plain = (format ?? '').replace(/p$/, '');
	if (plain === 's32') return sourceBitDepth === 24 || /^pcm_[su]24/.test(codec ?? '') ? '24-bit' : '32-bit';
	if (plain === 'float' || plain === 'flt') return '32-bit float';
	if (plain === 'double' || plain === 'dbl') return '64-bit float';
	const bits = /^[su](\d+)$/.exec(plain);
	return bits ? `${bits[1]}-bit` : '';
}

/**
 * The format badge: "FLAC · 24-bit/44.1 kHz". A lossy codec has no bit depth of its own (the engine decodes it to whatever it
 * likes), so it shows the sample rate alone: "MP3 · 44.1 kHz". Null when the codec is not known (no badge).
 */
export function formatLabel(signal: Signal | null | undefined, sourceBitDepth?: number | null): string | null {
	const codec = codecName(signal?.codec);
	if (!signal || !codec) return null;
	const rate = kHz(signal.decoded.sample_rate);
	const bits = isLossy(signal.codec) ? '' : bitDepth(signal.decoded.format, sourceBitDepth, signal.codec);
	const detail = bits && rate ? `${bits}/${rate}` : bits || rate;
	return detail ? `${codec} · ${detail}` : codec;
}

export type Verdict = { word: 'Bit-perfect' | 'Lossless' | 'Resampled' | 'Lossy'; color: string };

export const VERDICTS = {
	bitPerfect: { word: 'Bit-perfect', color: '#6EE7B7' },
	lossless: { word: 'Lossless', color: '#93C5FD' },
	resampled: { word: 'Resampled', color: '#FCD34D' },
	lossy: { word: 'Lossy', color: '#9CA3AF' }
} as const satisfies Record<string, Verdict>;

/**
 * The verdict beside the badge. A lossy codec is Lossy whatever happens after it. Otherwise: Resampled when the engine changes the
 * sample rate; Bit-perfect when Stash has the sound card alone and the samples go out untouched (same format, volume at 100, no
 * ReplayGain); Lossless for a lossless codec that is not bit-perfect (the system's mixer is in the way, the sample format was
 * changed, or the samples were made louder or softer). Null when the codec is not known.
 */
export function verdict(signal: Signal | null | undefined): Verdict | null {
	if (!signal) return null;
	if (isLossy(signal.codec)) return VERDICTS.lossy;
	if (signal.resampled) return VERDICTS.resampled;
	if (signal.mode === 'exclusive' && !signal.converted && !signal.scaled) return VERDICTS.bitPerfect;
	if (isLossless(signal.codec)) return VERDICTS.lossless;
	return null;
}

const TIERS: Record<Tier, string> = { low: 'low', high: 'high', lossless: 'lossless', hires: 'hi-res' };

/**
 * Where an add-on's stream comes from and what the add-on said it is ("Demo folder · lossless"), and whether the sound agrees: an
 * add-on that said lossless or hi-res while the engine decodes a lossy codec, or said hi-res while the samples are CD's, is told so.
 */
export function origin(addonName: string | null, tier: Tier | null, signal: Signal | null | undefined, sourceBitDepth?: number | null): { text: string; disagrees: boolean } | null {
	if (!addonName) return null;
	if (!tier) return { text: addonName, disagrees: false };
	const said = TIERS[tier] ?? tier;
	const codec = signal?.codec ?? '';
	if ((tier === 'lossless' || tier === 'hires') && isLossy(codec)) return { text: `${addonName} said ${said}; this is ${codecName(codec)}`, disagrees: true };
	const rate = signal?.decoded.sample_rate ?? null;
	const format = (signal?.decoded.format ?? '').replace(/p$/, '');
	if (tier === 'hires' && rate !== null && rate <= 48_000 && format === 's16') return { text: `${addonName} said ${said}; this is ${[bitDepth(format, sourceBitDepth), kHz(rate)].filter(Boolean).join(' ')}`, disagrees: true };
	return { text: `${addonName} · ${said}`, disagrees: false };
}

/** What the sound card is being sent right now, in a sentence (Settings › Sound). Null when nothing is open. */
export function outputSentence(signal: Signal | null | undefined): string | null {
	if (!signal || !signal.output.sample_rate) return null;
	const how = signal.mode === 'exclusive' ? 'for Stash alone' : signal.mode === 'shared' ? 'shared with other programs' : '';
	const kept = !signal.resampled && !signal.converted && !signal.scaled;
	return `Right now the sound card is open at ${kHz(signal.output.sample_rate)}${how ? `, ${how}` : ''}. The track is ${kept ? 'going out as it is' : 'being changed on the way'} (${kHz(signal.decoded.sample_rate)} in).`;
}
