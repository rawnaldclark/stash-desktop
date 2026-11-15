//! What the audio tests share: a test signal, WAV reading and writing, and playing files or URLs through an engine
//! whose output is written to a file.
#![allow(dead_code)]

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stash_core::audio::{EndReason, Engine, Event, Output};

pub const RATE: u32 = 44_100;

/// The libmpv to test against: `STASH_MPV_LIBRARY`, one beside the test program, or `third_party/mpv/` in the
/// repository. `None` (after saying so) when there is none: the test then passes without having run.
pub fn library() -> Option<PathBuf> {
    if let Some(path) = stash_core::audio::find_library() {
        return Some(path);
    }
    let name = if cfg!(windows) {
        "libmpv-2.dll"
    } else if cfg!(target_os = "macos") {
        "libmpv.2.dylib"
    } else {
        "libmpv.so.2"
    };
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/mpv")
        .join(name);
    if bundled.is_file() {
        return Some(bundled);
    }
    eprintln!("SKIPPED: no libmpv found (set STASH_MPV_LIBRARY, or put it in third_party/mpv/)");
    None
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stash-audio-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stereo signal with no period near any cut: 441 Hz on the left, 663 Hz on the right, so a swapped channel, a
/// dropped sample or a repeated one all show. Interleaved, 16-bit.
pub fn signal(frames: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let t = n as f64 / RATE as f64;
        out.push((0.6 * i16::MAX as f64 * (TAU * 441.0 * t).sin()) as i16);
        out.push((0.4 * i16::MAX as f64 * (TAU * 663.0 * t).sin()) as i16);
    }
    out
}

/// 16-bit stereo PCM as a plain 44-byte-header WAV.
pub fn wav_bytes(rate: u32, interleaved: &[i16]) -> Vec<u8> {
    let data_bytes = (interleaved.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // channels
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes a second
    bytes.extend_from_slice(&4u16.to_le_bytes()); // bytes a frame
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in interleaved {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

pub fn write_wav(path: &Path, rate: u32, interleaved: &[i16]) {
    fs::write(path, wav_bytes(rate, interleaved)).unwrap();
}

pub struct Wav {
    pub format: u16,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub data: Vec<u8>,
}

impl Wav {
    pub fn samples(&self) -> Vec<i16> {
        self.data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| i16::from_le_bytes(*s))
            .collect()
    }
}

/// Reads a WAV by its chunks (the engine may write an extensible header).
pub fn read_wav(path: &Path) -> Wav {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let (mut format, mut channels, mut rate, mut bits, mut data) =
        (0u16, 0u16, 0u32, 0u16, Vec::new());
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            format = u16::from_le_bytes(body[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            // WAVE_FORMAT_EXTENSIBLE carries the real format in the first two bytes of its sub-format.
            if format == 0xFFFE && body.len() >= 26 {
                format = u16::from_le_bytes(body[24..26].try_into().unwrap());
            }
        } else if id == b"data" {
            data = body.to_vec();
        }
        at += 8 + size + (size & 1);
    }
    Wav {
        format,
        channels,
        rate,
        bits,
        data,
    }
}

pub struct Played {
    pub ended: Vec<EndReason>,
    /// How many times the engine opened an audio output.
    pub output_opened: usize,
    pub log: Vec<String>,
}

impl Played {
    pub fn log_text(&self) -> String {
        self.log.join("\n")
    }
}

/// Plays `locators` (paths or URLs) in order through an engine that writes its output to `out`, until all have ended.
pub fn play_to_file(library: &Path, locators: &[&str], out: &Path) -> Played {
    let engine = Engine::start(library, &Output::PcmFile(out.to_path_buf()), "v").unwrap();
    engine.play_now(locators[0]).unwrap();
    for locator in &locators[1..] {
        engine.play_next(locator).unwrap();
    }
    let mut played = Played {
        ended: Vec::new(),
        output_opened: 0,
        log: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while played.ended.len() < locators.len() {
        assert!(
            Instant::now() < deadline,
            "the engine did not finish in 30 s; its log:\n{}",
            played.log_text()
        );
        match engine.next_event(0.5) {
            Event::EndFile(reason) => played.ended.push(reason),
            Event::Log { prefix, text, .. } => {
                // mpv says "AO: [pcm] 44100Hz stereo 2ch s16" each time it opens an audio output.
                if text.starts_with("AO: [") {
                    played.output_opened += 1;
                }
                played.log.push(format!("[{prefix}] {text}"));
            }
            Event::Shutdown => break,
            _ => {}
        }
    }
    drop(engine); // closes the output: the WAV is complete from here on
    played
}

/// Panics unless `got` is `want`, sample for sample.
pub fn assert_same_samples(got: &[i16], want: &[i16], join_frame: usize) {
    assert_eq!(
        got.len(),
        want.len(),
        "no sample added or dropped ({} frames in, {} out)",
        want.len() / 2,
        got.len() / 2
    );
    if let Some(at) = got.iter().zip(want).position(|(g, w)| g != w) {
        panic!(
            "the output differs from the signal at sample {at} (frame {}; the join is at frame {join_frame})",
            at / 2
        );
    }
}
