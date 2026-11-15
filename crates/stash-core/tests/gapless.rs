//! Gapless playback, measured: two files cut from one continuous signal must come out of the engine as that signal,
//! sample for sample, with nothing added or dropped at the join and the audio output opened once.
//!
//! Needs libmpv: `STASH_MPV_LIBRARY`, or `third_party/mpv/` in the repository. Without it the tests say so and pass,
//! so a checkout without the engine still builds and tests the rest.

use std::f64::consts::TAU;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stash_core::audio::{EndReason, Engine, Event, Output};

const RATE: u32 = 44_100;

fn library() -> Option<PathBuf> {
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

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stash-audio-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stereo signal with no period near the cut: 441 Hz on the left, 663 Hz on the right, so a swapped channel, a
/// dropped sample or a repeated one all show.
fn signal(samples: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(samples * 2);
    for n in 0..samples {
        let t = n as f64 / RATE as f64;
        out.push((0.6 * i16::MAX as f64 * (TAU * 441.0 * t).sin()) as i16);
        out.push((0.4 * i16::MAX as f64 * (TAU * 663.0 * t).sin()) as i16);
    }
    out
}

/// 16-bit stereo PCM in a plain 44-byte-header WAV.
fn write_wav(path: &Path, rate: u32, interleaved: &[i16]) {
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
    fs::write(path, bytes).unwrap();
}

struct Wav {
    format: u16,
    channels: u16,
    rate: u32,
    bits: u16,
    data: Vec<u8>,
}

/// Reads a WAV by its chunks (the engine may write an extensible header).
fn read_wav(path: &Path) -> Wav {
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

struct Played {
    ended: Vec<EndReason>,
    output_opened: usize,
    log: Vec<String>,
}

/// Plays `files` in order through an engine that writes its output to `out`, until all have ended.
fn play_to_file(library: &Path, files: &[&Path], out: &Path) -> Played {
    let engine = Engine::start(library, &Output::PcmFile(out.to_path_buf()), "v").unwrap();
    engine.play_now(&files[0].to_string_lossy()).unwrap();
    for file in &files[1..] {
        engine.play_next(&file.to_string_lossy()).unwrap();
    }
    let mut played = Played {
        ended: Vec::new(),
        output_opened: 0,
        log: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while played.ended.len() < files.len() {
        assert!(
            Instant::now() < deadline,
            "the engine did not finish in 30 s; its log:\n{}",
            played.log.join("\n")
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

#[test]
fn two_files_cut_from_one_signal_play_as_that_signal_sample_for_sample() {
    let Some(library) = library() else { return };
    let dir = scratch("gapless");
    // The cut is not on a zero crossing or a block boundary: an inserted gap or a lost frame cannot hide.
    let (first, second) = (30_011usize, 40_007usize);
    let whole = signal(first + second);
    let (a, b, out) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("out.wav"));
    write_wav(&a, RATE, &whole[..first * 2]);
    write_wav(&b, RATE, &whole[first * 2..]);

    let played = play_to_file(&library, &[&a, &b], &out);
    assert_eq!(
        played.ended,
        [EndReason::Eof, EndReason::Eof],
        "log:\n{}",
        played.log.join("\n")
    );
    assert_eq!(
        played.output_opened,
        1,
        "the audio output was opened once for both tracks; log:\n{}",
        played.log.join("\n")
    );

    let wav = read_wav(&out);
    assert_eq!(
        (wav.format, wav.channels, wav.rate, wav.bits),
        (1, 2, RATE, 16),
        "the output is the files' own format: nothing was converted"
    );
    let got: Vec<i16> = wav
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| i16::from_le_bytes(*s))
        .collect();
    assert_eq!(
        got.len(),
        whole.len(),
        "no sample added or dropped ({} frames in, {} out)",
        whole.len() / 2,
        got.len() / 2
    );
    if let Some(at) = got.iter().zip(&whole).position(|(g, w)| g != w) {
        panic!(
            "the output differs from the signal at sample {at} (frame {}; the join is at frame {first})",
            at / 2
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_new_sample_rate_reopens_the_output_instead_of_resampling() {
    let Some(library) = library() else { return };
    let dir = scratch("rate-change");
    let (a, b, out) = (dir.join("a.wav"), dir.join("b.wav"), dir.join("out.wav"));
    write_wav(&a, 44_100, &signal(22_050));
    write_wav(&b, 96_000, &signal(48_000));

    let played = play_to_file(&library, &[&a, &b], &out);
    assert_eq!(
        played.ended,
        [EndReason::Eof, EndReason::Eof],
        "log:\n{}",
        played.log.join("\n")
    );
    assert_eq!(
        played.output_opened,
        2,
        "one output per format; log:\n{}",
        played.log.join("\n")
    );
    // The file output starts over when it is reopened, so it holds the second track alone, at its own rate.
    let wav = read_wav(&out);
    assert_eq!((wav.rate, wav.data.len() / 4), (96_000, 48_000));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_cannot_be_opened_ends_with_an_error_and_the_next_one_plays() {
    let Some(library) = library() else { return };
    let dir = scratch("missing");
    let (missing, b, out) = (
        dir.join("not-there.wav"),
        dir.join("b.wav"),
        dir.join("out.wav"),
    );
    write_wav(&b, RATE, &signal(4_410));

    let played = play_to_file(&library, &[&missing, &b], &out);
    assert!(
        matches!(played.ended[0], EndReason::Error(_)),
        "{:?}",
        played.ended
    );
    assert_eq!(played.ended[1], EndReason::Eof);
    assert_eq!(read_wav(&out).data.len() / 4, 4_410);
    let _ = fs::remove_dir_all(&dir);
}
