//! The player: one audio engine, a queue, and what is true about the sound right now.
//!
//! The queue is handed to the engine whole, so the engine joins one track to the next itself (that is what makes the
//! joins gapless). The player keeps the queue's words (which track is which), follows the engine's place in it, and
//! answers "what is playing, how far in, and what is being sent to the sound card" from the engine's own report.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::Serialize;

use crate::Result;
use crate::audio::{EndReason, Engine, Event, Output};

/// A track in the queue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueItem {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// A path or a URL the engine can open. Not shown to the window.
    #[serde(skip)]
    pub locator: String,
}

/// One end of the signal path: what the samples are at that point.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Format {
    pub sample_rate: Option<u32>,
    /// The sample format as the engine names it: "s16", "s32", "float".
    pub format: Option<String>,
}

/// What is happening to the sound between the file and the sound card, as the engine reports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Signal {
    /// The codec being decoded: "flac", "mp3", "pcm_s16le".
    pub codec: Option<String>,
    pub decoded: Format,
    pub output: Format,
    /// The audio output in use: "wasapi", "coreaudio", "pipewire", "null".
    pub driver: Option<String>,
    /// "exclusive" or "shared", when the output said.
    pub mode: Option<String>,
    /// The sample rate sent to the output is not the file's.
    pub resampled: bool,
    /// The sample format sent to the output is not the decoder's.
    pub converted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerState {
    /// "idle", "playing" or "paused".
    pub status: &'static str,
    /// The playing track's place in the queue.
    pub index: Option<usize>,
    pub queue_len: usize,
    pub track: Option<QueueItem>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub signal: Option<Signal>,
    /// The last track that could not be played, and why.
    pub error: Option<String>,
}

#[derive(Default)]
struct Shared {
    queue: Vec<QueueItem>,
    index: Option<usize>,
    /// The output's own word on how it opened, e.g. "… @ 48000hz (shared)".
    accepted: Option<String>,
    error: Option<String>,
    /// A seek asked for while the next track was still loading; it is done when the track has loaded.
    pending_seek: Option<f64>,
}

pub struct Player {
    engine: Arc<Engine>,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    events: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts an engine on the libmpv at `library`, sending to `output`, with an empty queue.
    pub fn start(library: &Path, output: &Output) -> Result<Self> {
        let engine = Arc::new(Engine::start(library, output, "v")?);
        engine.mpv().observe("playlist-pos")?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let events = {
            let (engine, shared, stop) =
                (Arc::clone(&engine), Arc::clone(&shared), Arc::clone(&stop));
            // The one place that waits for engine events (the engine allows one waiter).
            std::thread::Builder::new()
                .name("stash-player".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match engine.next_event(0.25) {
                            Event::Property { name, value } if name == "playlist-pos" => {
                                let index = value
                                    .and_then(|v| v.parse::<i64>().ok())
                                    .filter(|i| *i >= 0)
                                    .map(|i| i as usize);
                                shared.lock().unwrap().index = index;
                            }
                            Event::Log { prefix, text, .. }
                                if prefix.starts_with("ao") && text.starts_with("Accepted as") =>
                            {
                                shared.lock().unwrap().accepted = Some(text);
                            }
                            Event::EndFile(EndReason::Error(_)) => {
                                let mut shared = shared.lock().unwrap();
                                let what = shared
                                    .index
                                    .and_then(|i| shared.queue.get(i))
                                    .map(|t| t.title.clone())
                                    .unwrap_or_else(|| "a track".into());
                                shared.error = Some(format!("{what} could not be played"));
                            }
                            // The engine only says its place in the queue when the number changes. A new queue that starts on
                            // the same place as the old one (first track to first track) changes nothing, so the place is
                            // read whenever a track starts.
                            Event::StartFile => {
                                let index = engine
                                    .mpv()
                                    .get("playlist-pos")
                                    .and_then(|v| v.parse::<i64>().ok())
                                    .filter(|i| *i >= 0)
                                    .map(|i| i as usize);
                                shared.lock().unwrap().index = index;
                            }
                            Event::FileLoaded => {
                                let pending = shared.lock().unwrap().pending_seek.take();
                                if let Some(seconds) = pending {
                                    let _ = engine.seek_to(seconds);
                                }
                            }
                            Event::Shutdown => break,
                            _ => {}
                        }
                    }
                })
                .map_err(|e| crate::Error::Audio(format!("starting the player's thread: {e}")))?
        };
        Ok(Self {
            engine,
            shared,
            stop,
            events: Some(events),
        })
    }

    /// Replaces the queue with `items` and plays from the first.
    pub fn play(&self, items: Vec<QueueItem>) -> Result<()> {
        let Some(first) = items.first() else {
            return self.stop();
        };
        {
            let mut shared = self.shared.lock().unwrap();
            shared.queue = items.clone();
            shared.index = None;
            shared.error = None;
            shared.pending_seek = None;
        }
        self.engine.play_now(&first.locator)?;
        for item in &items[1..] {
            self.engine.play_next(&item.locator)?;
        }
        self.engine.set_paused(false)
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.engine.set_paused(paused)
    }

    /// On to the next track; at the end of the queue, playback ends.
    pub fn next(&self) -> Result<()> {
        self.engine.mpv().command(&["playlist-next", "force"])
    }

    /// Back to the previous track; on the first, it starts over.
    pub fn previous(&self) -> Result<()> {
        if self.shared.lock().unwrap().index == Some(0) {
            return self.seek_to(0.0);
        }
        self.engine.mpv().command(&["playlist-prev", "weak"])
    }

    /// Jumps to `seconds` in the current track. Between two tracks there is nothing to seek in yet: the seek is
    /// kept and done when the track that is loading has loaded.
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        let seconds = seconds.max(0.0);
        if self.engine.seek_to(seconds).is_err() {
            self.shared.lock().unwrap().pending_seek = Some(seconds);
        }
        Ok(())
    }

    /// Stops and empties the queue.
    pub fn stop(&self) -> Result<()> {
        {
            let mut shared = self.shared.lock().unwrap();
            shared.queue.clear();
            shared.index = None;
        }
        self.engine.stop()
    }

    pub fn state(&self) -> PlayerState {
        let (queue_len, index, track, accepted, error) = {
            let shared = self.shared.lock().unwrap();
            let track = shared.index.and_then(|i| shared.queue.get(i)).cloned();
            (
                shared.queue.len(),
                shared.index,
                track,
                shared.accepted.clone(),
                shared.error.clone(),
            )
        };
        let mpv = self.engine.mpv();
        let seconds = |name: &str| {
            mpv.get(name)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|s| (s.max(0.0) * 1000.0) as u64)
                .unwrap_or(0)
        };
        let number = |name: &str| mpv.get(name).and_then(|v| v.parse::<u32>().ok());
        let playing = track.is_some();
        let signal = playing.then(|| {
            let decoded = Format {
                sample_rate: number("audio-params/samplerate"),
                format: mpv.get("audio-params/format"),
            };
            let output = Format {
                sample_rate: number("audio-out-params/samplerate"),
                format: mpv.get("audio-out-params/format"),
            };
            // "s16" and "s16p" are the same samples laid out differently.
            let plain =
                |f: &Option<String>| f.as_deref().map(|f| f.trim_end_matches('p').to_string());
            Signal {
                codec: mpv.get("audio-codec-name"),
                resampled: decoded.sample_rate.is_some()
                    && output.sample_rate.is_some()
                    && decoded.sample_rate != output.sample_rate,
                converted: decoded.format.is_some()
                    && output.format.is_some()
                    && plain(&decoded.format) != plain(&output.format),
                decoded,
                output,
                driver: mpv.get("current-ao"),
                mode: accepted
                    .as_deref()
                    .and_then(|a| {
                        if a.contains("(exclusive)") {
                            Some("exclusive")
                        } else if a.contains("(shared)") {
                            Some("shared")
                        } else {
                            None
                        }
                    })
                    .map(str::to_string),
            }
        });
        PlayerState {
            status: if !playing {
                "idle"
            } else if mpv.get("pause").as_deref() == Some("yes") {
                "paused"
            } else {
                "playing"
            },
            index,
            queue_len,
            track,
            position_ms: if playing { seconds("time-pos") } else { 0 },
            duration_ms: if playing { seconds("duration") } else { 0 },
            signal,
            error,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(events) = self.events.take() {
            let _ = events.join();
        }
    }
}
