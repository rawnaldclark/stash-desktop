//! What does this computer's sound card do with each format? `stashd output-check` plays a moment of digital
//! silence at several sample rates and bit depths, in shared or exclusive mode, and reports what the audio output
//! opened at for each. Nothing is audible. In exclusive mode other programs are silent while each file plays.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use stash_core::audio::{EndReason, Engine, Event, Output, find_library};

/// The formats tried: CD, the common video rate, and two hi-res ones.
const FORMATS: [(u32, u16); 4] = [(44_100, 16), (48_000, 16), (96_000, 24), (192_000, 24)];

/// Half a second of stereo digital silence as a WAV file.
fn silence(path: &Path, rate: u32, bits: u16) -> Result<()> {
    let frame = 2 * (bits as u32 / 8);
    let data = (rate / 2) * frame;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * frame).to_le_bytes());
    bytes.extend_from_slice(&(frame as u16).to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    bytes.resize(44 + data as usize, 0);
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

struct Opened {
    /// What the output said when it opened, e.g. "AO: [wasapi] 96000Hz stereo 2ch s32".
    output: Option<String>,
    /// The output's own log lines, for what mode it took.
    notes: Vec<String>,
    ended: Option<EndReason>,
}

fn play(library: &Path, file: &Path, exclusive: bool) -> Result<Opened> {
    let engine = Engine::start(library, &Output::Device { exclusive }, "v")?;
    engine.play_now(&file.to_string_lossy())?;
    let mut opened = Opened {
        output: None,
        notes: Vec::new(),
        ended: None,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while opened.ended.is_none() && Instant::now() < deadline {
        match engine.next_event(0.5) {
            Event::EndFile(reason) => opened.ended = Some(reason),
            Event::Log { prefix, text, .. } => {
                if text.starts_with("AO: [") {
                    opened.output = Some(text);
                } else if prefix.starts_with("ao") {
                    opened.notes.push(text);
                }
            }
            Event::Shutdown => break,
            _ => {}
        }
    }
    Ok(opened)
}

pub fn run(mpv: Option<PathBuf>, exclusive: bool, verbose: bool) -> Result<()> {
    let Some(library) = mpv.or_else(find_library) else {
        bail!(
            "no audio engine: pass --mpv <libmpv file>, or set {}",
            stash_core::audio::LIBRARY_ENV
        );
    };
    let dir = std::env::temp_dir().join(format!("stash-output-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    println!(
        "{} mode, half a second of silence per format:",
        if exclusive { "exclusive" } else { "shared" }
    );
    for (rate, bits) in FORMATS {
        let file = dir.join(format!("silence-{rate}-{bits}.wav"));
        silence(&file, rate, bits)?;
        let opened = play(&library, &file, exclusive)?;
        let output = opened.output.as_deref().unwrap_or("no output opened");
        let same_rate = opened
            .output
            .as_deref()
            .is_some_and(|o| o.contains(&format!("{rate}Hz")));
        println!(
            "  {bits}-bit {rate} Hz file -> {output}{}{}",
            if same_rate {
                ""
            } else {
                "  (not the file's rate)"
            },
            match opened.ended {
                Some(EndReason::Eof) => String::new(),
                other => format!("  [ended: {other:?}]"),
            }
        );
        if let Some(accepted) = opened
            .notes
            .iter()
            .rev()
            .find(|n| n.starts_with("Accepted as"))
        {
            println!("      {accepted}");
        }
        if verbose {
            for note in opened
                .notes
                .iter()
                .filter(|n| !n.starts_with("Accepted as"))
            {
                println!("      {note}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
