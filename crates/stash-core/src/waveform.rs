//! A song's waveform: the loudest moment in each of a few hundred equal slices of a file, for a seek bar drawn in the
//! song's own shape. The file is decoded once (Symphonia: FLAC, WAV, Ogg Vorbis, MP3, AAC and ALAC in MP4); the caller
//! keeps the result.

use std::fs::File;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// How many slices a waveform has.
pub const SLICES: usize = 480;
/// Frames read into one block before it is reduced to its peak: a few thousand blocks for a song.
const BLOCK: usize = 2048;

/// The waveform of the file at `path`: [`SLICES`] values from 0 to 255, the loudest slice at 255 (a quiet recording
/// keeps its shape, not its quietness). `Err` when the file cannot be read or decoded (an Opus file, say).
pub fn peaks(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("not a file Stash can read: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("no audio in it")?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("its audio cannot be decoded here: {e}"))?;

    // The peak of every block of BLOCK frames, all channels together.
    let mut blocks: Vec<f32> = Vec::with_capacity(4096);
    let (mut peak, mut in_block) = (0f32, 0usize);
    let mut samples: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A damaged packet is skipped, as a player would.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let buffer = match &mut samples {
            Some(b) if b.capacity() >= decoded.capacity() * channels => b,
            _ => samples.insert(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec)),
        };
        buffer.copy_interleaved_ref(decoded);
        for frame in buffer.samples().chunks(channels) {
            for s in frame {
                peak = peak.max(s.abs());
            }
            in_block += 1;
            if in_block == BLOCK {
                blocks.push(peak);
                (peak, in_block) = (0.0, 0);
            }
        }
    }
    if in_block > 0 {
        blocks.push(peak);
    }
    if blocks.is_empty() {
        return Err("no audio in it".into());
    }
    Ok(slices(&blocks, SLICES))
}

/// `blocks` reduced to `n` slices, each the peak of the blocks it covers, scaled so the loudest is 255.
pub fn slices(blocks: &[f32], n: usize) -> Vec<u8> {
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let from = i * blocks.len() / n;
            let to = ((i + 1) * blocks.len() / n).max(from + 1).min(blocks.len());
            blocks.get(from..to).map_or(0.0, |b| b.iter().fold(0f32, |m, v| m.max(*v)))
        })
        .collect();
    let top = out.iter().fold(0f32, |m, v| m.max(*v));
    if top > 0.0 {
        for v in &mut out {
            *v /= top;
        }
    }
    out.into_iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV of 16-bit mono: `seconds` of a tone whose loudness rises from silence to full in its second half.
    fn rising_wav(path: &Path, seconds: u32) {
        let rate = 8_000u32;
        let frames = rate * seconds;
        let mut data = Vec::with_capacity(frames as usize * 2);
        for i in 0..frames {
            let loud = if i < frames / 2 { 0.0 } else { (i - frames / 2) as f32 / (frames / 2) as f32 };
            let v = (loud * 30_000.0 * ((i as f32) * 0.3).sin()) as i16;
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(path, out).unwrap();
    }

    #[test]
    fn a_file_is_decoded_into_its_shape() {
        let path = std::env::temp_dir().join(format!("stash-wave-{}.wav", std::process::id()));
        rising_wav(&path, 30);
        let wave = peaks(&path).unwrap();
        assert_eq!(wave.len(), SLICES);
        // Silent for its first half, rising through its second, loudest at the end.
        // (A block of the decoded audio straddles the middle, so the first 45% is what must be silent.)
        assert!(wave[..SLICES * 45 / 100].iter().all(|v| *v == 0), "{:?}", &wave[SLICES * 40 / 100..SLICES / 2]);
        let (a, b) = (wave[SLICES * 5 / 8] as u32, wave[SLICES * 7 / 8] as u32);
        assert!(a > 40 && b > a + 40, "{a} {b}");
        assert!(wave[SLICES - 3..].iter().any(|v| *v >= 250));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn what_cannot_be_decoded_says_so() {
        let path = std::env::temp_dir().join(format!("stash-wave-bad-{}.flac", std::process::id()));
        std::fs::write(&path, b"not audio at all").unwrap();
        assert!(peaks(&path).is_err());
        assert!(peaks(Path::new("Q:/nowhere/none.flac")).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn blocks_become_slices_scaled_to_the_loudest() {
        assert_eq!(slices(&[0.0, 0.25, 0.5, 0.5], 4), vec![0, 128, 255, 255]);
        // Fewer blocks than slices: each slice takes the block it falls in.
        assert_eq!(slices(&[0.2, 0.4], 4), vec![128, 128, 255, 255]);
        // Silence stays silent.
        assert_eq!(slices(&[0.0, 0.0], 3), vec![0, 0, 0]);
    }
}
