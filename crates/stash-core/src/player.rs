//! The player: one audio engine, a queue, and what is true about the sound right now.
//!
//! The engine joins one track to the next itself (that is what makes the joins gapless), so the next track is handed
//! to it before the current one ends. A file is handed over as soon as the track before it starts. An add-on's track
//! is different: where it plays from is a link the add-on makes on request, which costs the listener's allowance
//! there and stops working after a while. So it is asked for only when it is about to be needed: when the listener
//! starts it, or when the track before it has half a minute left.
//!
//! The player keeps the queue's words (which track is which), follows the engine's place in it, and answers "what is
//! playing, how far in, and what is being sent to the sound card" from the engine's own report.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::Result;
use crate::audio::{EndReason, Engine, Event, Output};

/// How many times an add-on that is in trouble (not: does not have the song) is asked for the next track.
const TRIES: u32 = 3;
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// A track as an add-on knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddonTrack {
    /// The add-on's id.
    pub addon: String,
    /// The add-on's own id for the track.
    pub track: String,
}

/// A track in the queue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueItem {
    /// The library's row, for a track of the library.
    pub id: Option<i64>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Set for a track an add-on serves; where it plays from is asked for when it is about to play.
    pub addon: Option<AddonTrack>,
    /// For a file: its path. Not shown to the window. Empty for a song the library knows by its description, which
    /// is asked for like an add-on's track: of each of the listener's add-ons in turn.
    #[serde(skip)]
    pub locator: String,
}

impl QueueItem {
    /// Where it plays from has to be asked for.
    fn asks(&self) -> bool {
        self.addon.is_some() || self.locator.is_empty()
    }
}

/// Where an add-on's track plays from right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    /// A URL the engine can open.
    pub locator: String,
    /// What the add-on says the stream is: "low", "high", "lossless", "hires".
    pub tier: Option<String>,
    /// The id of the add-on that serves it.
    pub addon: Option<String>,
}

/// Why an add-on's track could not be located.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub message: String,
    /// The add-on answered that it cannot give this track (the queue moves on), as opposed to trouble reaching or
    /// using the add-on (asking again may help).
    pub miss: bool,
}

/// Asks where a track plays from: its add-on, or for a song known by its description, the listener's add-ons in turn.
/// Called on a thread of its own; it may take as long as the network does.
pub type Resolver = Arc<dyn Fn(&QueueItem) -> std::result::Result<Located, Failure> + Send + Sync>;

/// Told of a play that counted: the track, and when it started (milliseconds since 1970).
pub type OnPlay = Arc<dyn Fn(&QueueItem, i64) + Send + Sync>;

pub struct Settings {
    /// Without one, the queue can hold files only.
    pub resolver: Option<Resolver>,
    /// An add-on's track is asked for when the track before it has this long left.
    pub ask_ahead_seconds: f64,
    /// Told when a play counts.
    pub on_play: Option<OnPlay>,
    /// A play counts once half the track was really played, but never before the first of these seconds and always
    /// by the second: the rule the other Stashes (and Last.fm) use, 30 s and 4 minutes.
    pub counts_between_seconds: (f64, f64),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resolver: None,
            ask_ahead_seconds: 30.0,
            on_play: None,
            counts_between_seconds: (30.0, 240.0),
        }
    }
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
    /// "idle", "loading" (an add-on is being asked for the track), "playing" or "paused".
    pub status: &'static str,
    /// The track's place in the queue.
    pub index: Option<usize>,
    pub queue_len: usize,
    pub track: Option<QueueItem>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// What the add-on said this stream is. The signal path says what it turned out to be.
    pub tier: Option<String>,
    /// The id of the add-on the stream comes from.
    pub served_by: Option<String>,
    pub signal: Option<Signal>,
    /// The last track that could not be played, and why.
    pub error: Option<String>,
}

/// The listening to the track the engine is on.
struct Heard {
    /// Which of the engine's tracks, in which run of the queue.
    pos: usize,
    generation: u64,
    started_at_ms: i64,
    /// Seconds really played: not paused, not waiting for data.
    listened: f64,
    counted: bool,
}

/// A track the engine has been handed.
struct Slot {
    /// Its place in the queue.
    queue: usize,
    tier: Option<String>,
    addon: Option<String>,
}

#[derive(Default)]
struct Shared {
    queue: Vec<QueueItem>,
    /// The engine's playlist, entry for entry.
    slots: Vec<Slot>,
    /// The engine's place in its playlist.
    pos: Option<usize>,
    /// The queue place being started (its stream is being asked for, or the engine is opening it).
    starting: Option<usize>,
    /// The next queue place to hand to the engine.
    ahead: usize,
    /// Goes up whenever the listener changes what plays; work begun for an earlier number is dropped.
    generation: u64,
    /// The next track is being located.
    extending: bool,
    tries: u32,
    not_before: Option<Instant>,
    /// The add-on stayed in trouble: nothing more is asked until the listener does something.
    gave_up: bool,
    /// The output's own word on how it opened, e.g. "… @ 48000hz (shared)".
    accepted: Option<String>,
    error: Option<String>,
    /// A seek asked for while the next track was still loading; it is done when the track has loaded.
    pending_seek: Option<f64>,
    heard: Option<Heard>,
    /// The engine's last complaints about the track it is on: said with the failure when the track cannot be played.
    complaints: Vec<String>,
    /// Queue places whose stream failed in the engine and was asked for again, in this run of the queue.
    retried: std::collections::HashSet<usize>,
    /// A queue place to ask for again: its stream did not start.
    retry: Option<usize>,
}

impl Shared {
    fn slot(&self) -> Option<&Slot> {
        self.pos.and_then(|p| self.slots.get(p))
    }

    /// The queue place of the track the engine is on.
    fn index(&self) -> Option<usize> {
        self.slot().map(|s| s.queue)
    }
}

fn could_not(item: &QueueItem, why: &str) -> String {
    if why.is_empty() {
        format!("{} could not be played", item.title)
    } else {
        format!("{} could not be played: {why}", item.title)
    }
}

/// What the player's threads share.
#[derive(Clone)]
struct Ctx {
    engine: Arc<Engine>,
    shared: Arc<Mutex<Shared>>,
    resolver: Option<Resolver>,
    ask_ahead_seconds: f64,
    on_play: Option<OnPlay>,
    counts_between_seconds: (f64, f64),
}

impl Ctx {
    /// Called a few times a second: adds the time since `last` to the listening of the current track when the engine
    /// really played through it, and tells of the play once it counts.
    fn listen(&self, last: &mut Instant) {
        let now = Instant::now();
        let passed = now.duration_since(*last).as_secs_f64().min(1.0);
        *last = now;
        let Some(on_play) = &self.on_play else {
            return;
        };
        {
            let mut shared = self.shared.lock().unwrap();
            let Some(pos) = shared.pos.filter(|p| *p < shared.slots.len()) else {
                shared.heard = None;
                return;
            };
            let generation = shared.generation;
            let same = shared
                .heard
                .as_ref()
                .is_some_and(|h| h.pos == pos && h.generation == generation);
            if !same {
                let started_at_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                shared.heard = Some(Heard {
                    pos,
                    generation,
                    started_at_ms,
                    listened: 0.0,
                    counted: false,
                });
                return;
            }
            if shared.heard.as_ref().is_some_and(|h| h.counted) {
                return;
            }
        }
        let mpv = self.engine.mpv();
        if mpv.get("core-idle").as_deref() != Some("no") {
            return;
        }
        let (least, most) = self.counts_between_seconds;
        let needed = match mpv.get("duration").and_then(|v| v.parse::<f64>().ok()) {
            Some(length) if length > 0.0 => (length / 2.0).clamp(least, most),
            _ => most,
        };
        let counted = {
            let mut shared = self.shared.lock().unwrap();
            let Some(heard) = shared.heard.as_mut() else {
                return;
            };
            heard.listened += passed;
            if heard.listened < needed {
                return;
            }
            heard.counted = true;
            let (pos, started_at_ms) = (heard.pos, heard.started_at_ms);
            shared
                .slots
                .get(pos)
                .and_then(|slot| shared.queue.get(slot.queue))
                .cloned()
                .map(|item| (item, started_at_ms))
        };
        if let Some((item, started_at_ms)) = counted {
            on_play(&item, started_at_ms);
        }
    }

    fn locate(&self, item: &QueueItem) -> std::result::Result<Located, Failure> {
        if !item.asks() {
            return Ok(Located {
                locator: item.locator.clone(),
                tier: None,
                addon: None,
            });
        }
        match &self.resolver {
            Some(resolve) => resolve(item),
            None => Err(Failure {
                message: "this player has no add-ons".into(),
                miss: false,
            }),
        }
    }

    /// Runs `job` here when the track is a file (there is nothing to wait for), else on a thread of its own.
    fn run(&self, waits: bool, job: impl FnOnce(&Ctx) + Send + 'static) {
        if !waits {
            return job(self);
        }
        let ctx = self.clone();
        // The closure is kept in a cell so it can still be run here if no thread can be had.
        let job = Arc::new(Mutex::new(Some(job)));
        let theirs = Arc::clone(&job);
        let spawned = std::thread::Builder::new()
            .name("stash-locate".into())
            .spawn(move || {
                if let Some(job) = theirs.lock().unwrap().take() {
                    job(&ctx);
                }
            });
        if spawned.is_err()
            && let Some(job) = job.lock().unwrap().take()
        {
            job(self);
        }
    }

    /// Starts the queue place `ahead` points at, in place of whatever plays; a track the add-on does not have is
    /// passed over for the one after it.
    fn start(&self, generation: u64) {
        loop {
            let (item, place) = {
                let mut shared = self.shared.lock().unwrap();
                if shared.generation != generation {
                    return;
                }
                let place = shared.ahead;
                match shared.queue.get(place).cloned() {
                    Some(item) => {
                        shared.starting = Some(place);
                        (item, place)
                    }
                    None => {
                        shared.starting = None;
                        return;
                    }
                }
            };
            let located = self.locate(&item);
            let mut shared = self.shared.lock().unwrap();
            if shared.generation != generation {
                return;
            }
            match located {
                Ok(located) => {
                    let played = self
                        .engine
                        .play_now(&located.locator)
                        .and_then(|()| self.engine.set_paused(false));
                    match played {
                        Ok(()) => {
                            shared.slots = vec![Slot {
                                queue: place,
                                tier: located.tier,
                                addon: located.addon,
                            }];
                            shared.pos = None;
                            shared.ahead = place + 1;
                        }
                        Err(e) => {
                            shared.error = Some(could_not(&item, &e.to_string()));
                            shared.starting = None;
                        }
                    }
                    return;
                }
                Err(failure) => {
                    shared.error = Some(could_not(&item, &failure.message));
                    if !failure.miss {
                        shared.starting = None;
                        return;
                    }
                    shared.ahead = place + 1;
                }
            }
        }
    }

    /// Hands the engine the queue place `ahead` points at, to play after what it has.
    fn extend(&self, generation: u64) {
        let (item, place) = {
            let mut shared = self.shared.lock().unwrap();
            if shared.generation != generation {
                return;
            }
            let place = shared.ahead;
            match shared.queue.get(place).cloned() {
                Some(item) => (item, place),
                None => {
                    shared.extending = false;
                    return;
                }
            }
        };
        let located = self.locate(&item);
        let mut shared = self.shared.lock().unwrap();
        if shared.generation != generation {
            return;
        }
        shared.extending = false;
        match located {
            Ok(located) => {
                match self.engine.play_next(&located.locator) {
                    Ok(()) => shared.slots.push(Slot {
                        queue: place,
                        tier: located.tier,
                        addon: located.addon,
                    }),
                    Err(e) => shared.error = Some(could_not(&item, &e.to_string())),
                }
                shared.ahead = place + 1;
                shared.tries = 0;
                shared.not_before = None;
            }
            Err(failure) if failure.miss => {
                shared.error = Some(could_not(&item, &failure.message));
                shared.ahead = place + 1;
                shared.tries = 0;
                shared.not_before = None;
            }
            Err(failure) => {
                shared.tries += 1;
                if shared.tries >= TRIES {
                    shared.error = Some(could_not(&item, &failure.message));
                    shared.gave_up = true;
                } else {
                    shared.not_before = Some(Instant::now() + RETRY_AFTER);
                }
            }
        }
    }

    /// Starts again the queue place whose stream did not start, asking for the stream anew.
    fn retry_failed(&self) {
        let generation = {
            let mut shared = self.shared.lock().unwrap();
            let Some(place) = shared.retry.take() else {
                return;
            };
            shared.generation += 1;
            shared.slots.clear();
            shared.pos = None;
            shared.starting = Some(place);
            shared.ahead = place;
            shared.extending = false;
            shared.tries = 0;
            shared.not_before = None;
            shared.gave_up = false;
            shared.pending_seek = None;
            shared.error = None;
            shared.generation
        };
        let _ = self.engine.stop();
        self.run(true, move |ctx| ctx.start(generation));
    }

    /// Called every time the engine reports something, and a few times a second besides: if the engine has nothing
    /// after the current track and the queue has, the next one is located and handed over.
    fn keep_ahead(&self) {
        let (generation, waits) = {
            let shared = self.shared.lock().unwrap();
            if shared.extending
                || shared.gave_up
                || shared.starting.is_some()
                || shared.ahead >= shared.queue.len()
                || shared.not_before.is_some_and(|t| Instant::now() < t)
            {
                return;
            }
            match shared.pos {
                None if shared.slots.is_empty() => return,
                // The engine ran out with tracks left in the queue (the next one was not ready in time).
                None => {}
                Some(pos) if pos + 1 < shared.slots.len() || pos >= shared.slots.len() => return,
                Some(_) => {}
            }
            (shared.generation, shared.queue[shared.ahead].asks())
        };
        if waits && self.shared.lock().unwrap().pos.is_some() {
            // An add-on's track is asked for late. With no end in sight (a stream of unknown length) it is asked
            // for when the engine runs out.
            let remaining = self
                .engine
                .mpv()
                .get("time-remaining")
                .and_then(|v| v.parse::<f64>().ok());
            if remaining.is_none_or(|r| r > self.ask_ahead_seconds) {
                return;
            }
        }
        {
            let mut shared = self.shared.lock().unwrap();
            if shared.generation != generation || shared.extending {
                return;
            }
            shared.extending = true;
        }
        self.run(waits, move |ctx| ctx.extend(generation));
    }
}

pub struct Player {
    ctx: Ctx,
    stop: Arc<AtomicBool>,
    events: Option<JoinHandle<()>>,
}

impl Player {
    /// Starts an engine on the libmpv at `library`, sending to `output`, with an empty queue, for files only.
    pub fn start(library: &Path, output: &Output) -> Result<Self> {
        Self::start_with(library, output, Settings::default())
    }

    pub fn start_with(library: &Path, output: &Output, settings: Settings) -> Result<Self> {
        let engine = Arc::new(Engine::start(library, output, "v")?);
        engine.mpv().observe("playlist-pos")?;
        let ctx = Ctx {
            engine,
            shared: Arc::new(Mutex::new(Shared::default())),
            resolver: settings.resolver,
            ask_ahead_seconds: settings.ask_ahead_seconds,
            on_play: settings.on_play,
            counts_between_seconds: settings.counts_between_seconds,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let events = {
            let (ctx, stop) = (ctx.clone(), Arc::clone(&stop));
            // The one place that waits for engine events (the engine allows one waiter).
            std::thread::Builder::new()
                .name("stash-player".into())
                .spawn(move || {
                    let mut last = Instant::now();
                    while !stop.load(Ordering::Relaxed) {
                        match ctx.engine.next_event(0.25) {
                            Event::Property { name, value } if name == "playlist-pos" => {
                                let pos = value
                                    .and_then(|v| v.parse::<i64>().ok())
                                    .filter(|i| *i >= 0)
                                    .map(|i| i as usize);
                                ctx.shared.lock().unwrap().pos = pos;
                            }
                            Event::Log { prefix, text, .. }
                                if prefix.starts_with("ao") && text.starts_with("Accepted as") =>
                            {
                                ctx.shared.lock().unwrap().accepted = Some(text);
                            }
                            Event::Log {
                                prefix,
                                level,
                                text,
                            } if matches!(level.as_str(), "error" | "fatal") => {
                                let mut shared = ctx.shared.lock().unwrap();
                                if shared.complaints.len() < 6 {
                                    shared.complaints.push(format!("{prefix}: {}", text.trim()));
                                }
                            }
                            Event::EndFile(EndReason::Error(code)) => {
                                let reason = ctx.engine.mpv().error_text(code);
                                let mut shared = ctx.shared.lock().unwrap();
                                let what = shared
                                    .index()
                                    .or(shared.starting)
                                    .and_then(|i| shared.queue.get(i))
                                    .map(|t| t.title.clone())
                                    .unwrap_or_else(|| "A track".into());
                                // The engine's own words, without any address in them: a stream's address is the
                                // listener's key to it.
                                let said: Vec<String> = std::iter::once(reason)
                                    .chain(shared.complaints.drain(..))
                                    .map(|line| {
                                        line.split(' ')
                                            .map(|word| {
                                                if word.contains("://") {
                                                    "(the stream)"
                                                } else {
                                                    word
                                                }
                                            })
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                    })
                                    .filter(|line| !line.is_empty())
                                    .collect();
                                shared.error = Some(format!(
                                    "{what} could not be played: {}",
                                    said.join("; ")
                                ));
                                // A stream that did not start is asked for once more: its link may have run out, or
                                // the first try at its server failed. A file that cannot be played stays that way.
                                let place = shared.index().or(shared.starting);
                                if let Some(place) = place
                                    && shared.queue.get(place).is_some_and(|t| t.asks())
                                    && shared.retried.insert(place)
                                {
                                    shared.retry = Some(place);
                                }
                            }
                            // The engine only says its place when the number changes. A new playlist that starts on
                            // the same place as the old one (first track to first track) changes nothing, so the
                            // place is read whenever a track starts.
                            Event::StartFile => {
                                let pos = ctx
                                    .engine
                                    .mpv()
                                    .get("playlist-pos")
                                    .and_then(|v| v.parse::<i64>().ok())
                                    .filter(|i| *i >= 0)
                                    .map(|i| i as usize);
                                let mut shared = ctx.shared.lock().unwrap();
                                shared.pos = pos;
                                shared.complaints.clear();
                                if shared.slot().is_some() {
                                    shared.starting = None;
                                }
                            }
                            Event::FileLoaded => {
                                let pending = ctx.shared.lock().unwrap().pending_seek.take();
                                if let Some(seconds) = pending {
                                    let _ = ctx.engine.seek_to(seconds);
                                }
                            }
                            Event::Shutdown => break,
                            _ => {}
                        }
                        ctx.retry_failed();
                        ctx.keep_ahead();
                        ctx.listen(&mut last);
                    }
                })
                .map_err(|e| crate::Error::Audio(format!("starting the player's thread: {e}")))?
        };
        Ok(Self {
            ctx,
            stop,
            events: Some(events),
        })
    }

    /// Replaces the queue with `items` and plays from the first.
    pub fn play(&self, items: Vec<QueueItem>) -> Result<()> {
        if items.is_empty() {
            return self.stop();
        }
        self.ctx.shared.lock().unwrap().queue = items;
        self.start_at(0);
        Ok(())
    }

    /// Plays the queue from `place` on, in place of whatever plays.
    fn start_at(&self, place: usize) {
        let (generation, waits) = {
            let mut shared = self.ctx.shared.lock().unwrap();
            if place >= shared.queue.len() {
                return;
            }
            shared.generation += 1;
            shared.slots.clear();
            shared.pos = None;
            shared.starting = Some(place);
            shared.ahead = place;
            shared.extending = false;
            shared.tries = 0;
            shared.not_before = None;
            shared.gave_up = false;
            shared.error = None;
            shared.pending_seek = None;
            shared.retried.clear();
            shared.retry = None;
            (shared.generation, shared.queue[place].asks())
        };
        if waits {
            // What was playing stops now, not when the add-on has answered.
            let _ = self.ctx.engine.stop();
        }
        self.ctx.run(waits, move |ctx| ctx.start(generation));
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.ctx.engine.set_paused(paused)
    }

    /// On to the next track; at the end of the queue, playback ends.
    pub fn next(&self) -> Result<()> {
        let target = {
            let shared = self.ctx.shared.lock().unwrap();
            match (shared.pos, shared.slot().is_some(), shared.starting) {
                // The engine has the next track already.
                (Some(pos), true, _) if pos + 1 < shared.slots.len() => None,
                (_, true, _) => Some(shared.ahead),
                (_, false, Some(starting)) => Some(starting + 1),
                (_, false, None) => return Ok(()),
            }
        };
        match target {
            None => self.ctx.engine.mpv().command(&["playlist-next", "weak"]),
            Some(place) if place < self.ctx.shared.lock().unwrap().queue.len() => {
                self.start_at(place);
                Ok(())
            }
            // Past the end: the queue stays, playback ends.
            Some(_) => {
                {
                    let mut shared = self.ctx.shared.lock().unwrap();
                    shared.generation += 1;
                    shared.starting = None;
                }
                self.ctx.engine.stop()
            }
        }
    }

    /// Back to the previous track; on the first, it starts over.
    pub fn previous(&self) -> Result<()> {
        enum Back {
            Restart,
            Engine,
            To(usize),
        }
        let back = {
            let shared = self.ctx.shared.lock().unwrap();
            match (shared.index(), shared.starting) {
                (None, None) => return Ok(()),
                (Some(0), _) | (None, Some(0)) => Back::Restart,
                (None, Some(starting)) => Back::To(starting - 1),
                (Some(current), _) => {
                    let before = shared
                        .pos
                        .filter(|p| *p > 0)
                        .and_then(|p| shared.slots.get(p - 1))
                        .map(|s| s.queue);
                    match before {
                        // The engine still has the file before this one.
                        Some(place) if !shared.queue[place].asks() => Back::Engine,
                        // An add-on's track is asked for again: the link it gave may no longer work.
                        Some(place) => Back::To(place),
                        None => Back::To(current - 1),
                    }
                }
            }
        };
        match back {
            Back::Restart => self.seek_to(0.0),
            Back::Engine => self.ctx.engine.mpv().command(&["playlist-prev", "weak"]),
            Back::To(place) => {
                self.start_at(place);
                Ok(())
            }
        }
    }

    /// Jumps to `seconds` in the current track. Between two tracks there is nothing to seek in yet: the seek is
    /// kept and done when the track that is loading has loaded.
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        let seconds = seconds.max(0.0);
        if self.ctx.engine.seek_to(seconds).is_err() {
            self.ctx.shared.lock().unwrap().pending_seek = Some(seconds);
        }
        Ok(())
    }

    /// Stops and empties the queue.
    pub fn stop(&self) -> Result<()> {
        {
            let mut shared = self.ctx.shared.lock().unwrap();
            shared.generation += 1;
            shared.queue.clear();
            shared.slots.clear();
            shared.pos = None;
            shared.starting = None;
            shared.ahead = 0;
            shared.extending = false;
        }
        self.ctx.engine.stop()
    }

    pub fn state(&self) -> PlayerState {
        let (queue_len, index, starting, asking, track, tier, served_by, accepted, error) = {
            let shared = self.ctx.shared.lock().unwrap();
            let index = shared.index();
            let track = index
                .or(shared.starting)
                .and_then(|i| shared.queue.get(i))
                .cloned();
            (
                shared.queue.len(),
                index,
                shared.starting,
                // A file that is starting is as good as playing; a track that has to be asked for is not yet.
                track.as_ref().is_some_and(|t| t.asks()),
                track,
                shared.slot().and_then(|s| s.tier.clone()),
                shared.slot().and_then(|s| s.addon.clone()),
                shared.accepted.clone(),
                shared.error.clone(),
            )
        };
        let mpv = self.ctx.engine.mpv();
        let seconds = |name: &str| {
            mpv.get(name)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|s| (s.max(0.0) * 1000.0) as u64)
                .unwrap_or(0)
        };
        let number = |name: &str| mpv.get(name).and_then(|v| v.parse::<u32>().ok());
        let playing = index.is_some();
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
                match starting {
                    Some(_) if asking => "loading",
                    Some(_) => "playing",
                    None => "idle",
                }
            } else if mpv.get("pause").as_deref() == Some("yes") {
                "paused"
            } else {
                "playing"
            },
            index: index.or(starting),
            queue_len,
            track,
            position_ms: if playing { seconds("time-pos") } else { 0 },
            duration_ms: if playing { seconds("duration") } else { 0 },
            tier,
            served_by,
            signal,
            error,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.ctx.shared.lock().unwrap().generation += 1;
        if let Some(events) = self.events.take() {
            let _ = events.join();
        }
    }
}
