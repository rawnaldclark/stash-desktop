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
use crate::eq;

mod eq_run;

use eq_run::EqRun;
pub use eq_run::{CorrectionShown, EqState};

/// How many times an add-on that is in trouble (not: does not have the song) is asked for the next track.
const TRIES: u32 = 3;
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// The slowest and fastest songs play (Android's limits).
pub const MIN_SPEED: f64 = 0.1;
pub const MAX_SPEED: f64 = 4.0;
/// Tuning: the A of a recording, and how far from it songs can be heard (Android's limits, about two semitones).
pub const STANDARD_HZ: f64 = 440.0;
pub const MIN_HZ: f64 = 392.0;
pub const MAX_HZ: f64 = 494.0;

/// A tuning within its limits, to a tenth of a hertz (anything that is not a number is the standard).
pub fn tuning_hz(hz: f64) -> f64 {
    if !hz.is_finite() {
        return STANDARD_HZ;
    }
    (hz.clamp(MIN_HZ, MAX_HZ) * 10.0).round() / 10.0
}

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
    /// A cover on the web, when one is known.
    pub art: Option<String>,
    /// Its length, when it is known before it plays.
    pub duration_ms: Option<u64>,
    /// This place in the queue, by a number of its own: the same song queued twice is two of these. The player
    /// gives it; whatever is handed in is replaced.
    pub qid: u64,
    /// For a file: its path. Not shown to the window. Empty for a song the library knows by its description, which
    /// is asked for like an add-on's track: of each of the listener's add-ons in turn.
    #[serde(skip)]
    pub locator: String,
}

impl QueueItem {
    /// Where it plays from has to be asked for: an add-on's track, a song of the library known by its description,
    /// and a song of the library whose file is not there (given back after it was saved, or on a drive that is not
    /// plugged in): the add-ons may have it.
    fn asks(&self) -> bool {
        self.addon.is_some()
            || self.locator.is_empty()
            || (self.id.is_some() && !std::path::Path::new(&self.locator).exists())
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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Failure {
    pub message: String,
    /// The add-on answered that it cannot give this track (the queue moves on), as opposed to trouble reaching or
    /// using the add-on (asking again may help).
    pub miss: bool,
    /// The add-on's trouble, when that is why: its code, and how long it asked to be left alone (in seconds).
    pub code: Option<crate::addons::Code>,
    pub retry_after: Option<u32>,
    /// A miss that may not be one: the add-ons that said "no" are not finding other songs either (their lookup may be
    /// broken, or their allowance spent).
    pub doubtful: bool,
    /// No add-on could be asked at all (none added, or none switched on that can).
    pub nothing_to_ask: bool,
    /// An add-on was asked for the song this time (not only "no"s remembered from before).
    pub asked: bool,
    /// An add-on answered this time, whatever it said (or one that said a little while ago it does not have the song
    /// has answered in the last few minutes, and was not just found out of reach): when another could not be reached,
    /// the line was not down, so that was the other add-on's trouble.
    pub answered: bool,
}

/// Asks where a track plays from: its add-on, or for a song known by its description, the listener's add-ons in turn.
/// Called on a thread of its own; it may take as long as the network does.
pub type Resolver = Arc<dyn Fn(&QueueItem) -> std::result::Result<Located, Failure> + Send + Sync>;

/// Told of a play that counted: the track, and when it started (milliseconds since 1970).
pub type OnPlay = Arc<dyn Fn(&QueueItem, i64) + Send + Sync>;

/// Told when the engine could not play a track, before the track is asked for once more: what it was given may be
/// what failed.
pub type OnFailed = Arc<dyn Fn(&QueueItem) + Send + Sync>;

/// The engine's end of a track with nothing to play (`MPV_ERROR_NOTHING_TO_PLAY`): what it says when the filters for a
/// track's audio cannot be built.
const NOTHING_TO_PLAY: i32 = -16;

/// Told when what each output keeps changed by itself (an output opened for the first time, or an equalizer the engine
/// could not use and turned off), to keep it. The book is read by calling the function given, once the place it is
/// kept in is held: a change the listener made meanwhile is never overwritten by an older book.
pub type OnEqualizers = Arc<dyn Fn(&dyn Fn() -> eq::Book) + Send + Sync>;

pub struct Settings {
    /// Without one, the queue can hold files only.
    pub resolver: Option<Resolver>,
    /// An add-on's track is asked for when the track before it has this long left,
    pub ask_ahead_seconds: f64,
    /// or once the track before it has played this long, so that skipping to it is at once (a track skipped sooner
    /// does not have the one after it asked for).
    pub ask_ahead_after_seconds: f64,
    /// Told when a play counts.
    pub on_play: Option<OnPlay>,
    /// Told when a track begins to be heard (the engine really plays it), with when it began: once for each play.
    pub on_start: Option<OnPlay>,
    /// Told when the engine could not play a track.
    pub on_failed: Option<OnFailed>,
    /// Told when what each output keeps changed by itself.
    pub on_equalizers: Option<OnEqualizers>,
    /// A play counts once half the track was really played, but never before the first of these seconds and always
    /// by the second: the rule the other Stashes (and Last.fm) use, 30 s and 4 minutes.
    pub counts_between_seconds: (f64, f64),
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resolver: None,
            ask_ahead_seconds: 30.0,
            ask_ahead_after_seconds: 5.0,
            on_play: None,
            on_start: None,
            on_failed: None,
            on_equalizers: None,
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
    /// The samples are made louder or softer on the way: the volume is not at 100, or ReplayGain evens this track
    /// out.
    pub scaled: bool,
    /// An equalizer changes the samples on the way: the engine's filter list holds Stash's equalizer, and it has not
    /// failed.
    pub equalized: bool,
}

/// What happens at the end of the queue, or of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    /// The queue starts over.
    All,
    /// The track starts over.
    One,
}

impl Repeat {
    pub fn as_str(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }

    pub fn from_word(word: &str) -> Self {
        match word {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerState {
    /// "idle", "loading" (an add-on is being asked for the track), "playing" or "paused".
    pub status: &'static str,
    /// The track's place in the queue.
    pub index: Option<usize>,
    pub queue_len: usize,
    /// Goes up whenever the queue's tracks or their order change: a reader of the queue reads it again then.
    pub queue_version: u64,
    /// Goes up only when the whole queue is replaced (a new queue, a stop, a kept queue brought back).
    pub queue_id: u64,
    pub shuffle: bool,
    /// "off", "all" or "one".
    pub repeat: &'static str,
    /// What the queue was started from, in the listener's words ("Liked songs", a playlist's name).
    pub from: Option<String>,
    pub track: Option<QueueItem>,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// What the add-on said this stream is. The signal path says what it turned out to be.
    pub tier: Option<String>,
    /// The id of the add-on the stream comes from.
    pub served_by: Option<String>,
    /// The volume, 0 to 100.
    pub volume: u32,
    /// Stash has the sound card to itself: each track goes out at its own format and other programs are silent.
    pub exclusive: bool,
    /// How loudness is evened out between songs from the gain in their tags: "off", "track" or "album".
    pub replaygain: &'static str,
    /// How fast songs play (1 as recorded); the pitch is not changed by it.
    pub speed: f64,
    /// Tuning: the A songs are heard at, in Hz (440 as recorded); the speed is not changed by it.
    pub tuning: f64,
    /// The equalizer of the output in use.
    pub eq: EqState,
    pub signal: Option<Signal>,
    /// The last track that could not be played, and why.
    pub error: Option<String>,
    /// That track's row in the library, when it is one.
    pub error_id: Option<i64>,
    /// How many times a track could not be played: the window tells of each, the same one twice too.
    pub errors: u64,
    /// Nothing plays and the player rests on `track` (`status` is "paused"): the queue ended there, the track
    /// could not be played, or the queue was brought back from last time. Play starts it.
    pub resting: bool,
    /// Nothing comes after the track playing: the engine has nothing after it, and the queue has nothing left to hand
    /// it (what was left could not be played). Not for the page.
    #[serde(skip)]
    pub nothing_after: bool,
}

/// The listening to the track the engine is on.
struct Heard {
    /// Which of the engine's tracks, in which run of the queue.
    pos: usize,
    generation: u64,
    started_at_ms: i64,
    /// Seconds really played: not paused, not waiting for data.
    listened: f64,
    /// It was heard (and told of as begun).
    told: bool,
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
    error_id: Option<i64>,
    errors: u64,
    /// A seek asked for while the next track was still loading; it is done when the track has loaded.
    pending_seek: Option<f64>,
    heard: Option<Heard>,
    /// The engine's last complaints about the track it is on: said with the failure when the track cannot be played.
    complaints: Vec<String>,
    /// Queue places whose stream failed in the engine and was asked for again, in this run of the queue.
    retried: std::collections::HashSet<usize>,
    /// A queue place to ask for again: its stream did not start.
    retry: Option<usize>,
    /// The queue place that could not be opened with the equalizer on, played again without it: if it plays, the
    /// equalizer was at fault.
    eq_suspect: Option<usize>,
    /// What the engine said when it could not build the equalizer (it may say it only after the track has ended).
    eq_said: Option<String>,
    /// The track opened has started to play (the engine said so).
    started: bool,
    /// Goes up whenever the queue is edited while it plays; a track located for an earlier number is dropped.
    edits: u64,
    next_qid: u64,
    version: u64,
    /// Goes up only when the whole queue is replaced (a new queue, a stop, a kept queue brought back): what was begun
    /// for one queue (a radio's next songs) is not added to another.
    queue_id: u64,
    shuffle: bool,
    /// While shuffled: the queue's qids in the order they had before, which turning shuffle off restores.
    original: Option<Vec<u64>>,
    repeat: Repeat,
    from: Option<String>,
    /// The track (by its queue number) the player rests on while nothing plays, and where in it, in seconds: the
    /// last track when the queue has ended, or the place a kept queue was at. Play goes on from there.
    rest: Option<(u64, f64)>,
    /// The queue number of the track that last played or was last started.
    last: Option<u64>,
}

/// A sound output of the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    /// The engine's name for it (`"wasapi/{…}"`, `"pulse/…"`), or `"auto"` for the system's own choice.
    pub name: String,
    /// What the system calls it ("Speakers (USB Sound Device)").
    pub description: String,
}

/// A gain as tags write it ("-6.50 dB", "+1.2 dB"; R128's whole numbers in 1/256 dB are told apart only by being
/// non-zero, which is all that is asked of them here).
fn gain_db(text: &str) -> Option<f64> {
    let number = text.split_whitespace().next()?;
    let number = number
        .strip_suffix("dB")
        .or_else(|| number.strip_suffix("db"))
        .unwrap_or(number);
    number.trim_start_matches('+').parse::<f64>().ok()
}

/// A system's name for an output, without the padding some drivers leave in it.
fn tidy(description: &str) -> String {
    let words = description.split_whitespace().collect::<Vec<_>>().join(" ");
    words.replace("( ", "(").replace(" )", ")")
}

/// A queue [`Player::play_from_at`] put in place: what it is known by in the player's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    /// Its id ([`PlayerState::queue_id`]): another id means it was replaced since.
    pub queue_id: u64,
    /// Its tracks' queue numbers ([`QueueItem::qid`]), in its order.
    pub qids: Vec<u64>,
    /// The queue number of the track it starts (or rests) on; `None` for an empty queue.
    pub start: Option<u64>,
}

/// A queue to bring back as it was left: nothing plays until the listener says so.
#[derive(Debug, Clone, Default)]
pub struct Kept {
    pub items: Vec<QueueItem>,
    /// The place in `items` to rest on, and where in that track, in seconds.
    pub place: usize,
    pub seconds: f64,
    pub from: Option<String>,
    pub shuffle: bool,
    /// While shuffled: the places of `items` in the order they had before.
    pub original: Option<Vec<usize>>,
    pub repeat: Repeat,
}

impl Shared {
    /// Notes that `item` could not be played, and why.
    fn fail(&mut self, item: &QueueItem, why: &str) {
        self.error = Some(could_not(item, why));
        self.error_id = item.id;
        self.errors += 1;
    }

    /// Where the player rests while nothing plays and nothing is starting: the queue place, and the seconds into it.
    fn resting(&self) -> Option<(usize, f64)> {
        if self.slot().is_some() || self.starting.is_some() {
            return None;
        }
        let (qid, seconds) = self.rest?;
        let place = self.queue.iter().position(|t| t.qid == qid)?;
        Some((place, seconds))
    }

    fn slot(&self) -> Option<&Slot> {
        self.pos.and_then(|p| self.slots.get(p))
    }

    /// The queue place of the track the engine is on.
    fn index(&self) -> Option<usize> {
        self.slot().map(|s| s.queue)
    }

    /// Gives each of `items` a queue number of its own.
    fn number(&mut self, items: &mut [QueueItem]) {
        for item in items {
            self.next_qid += 1;
            item.qid = self.next_qid;
        }
    }

    /// Puts `items[from..]` in a new order by chance, each artist's songs spread out (`shuffle.rs`).
    fn shuffle_from(items: &mut Vec<QueueItem>, from: usize) {
        crate::shuffle::spread_from(
            items,
            from,
            |t| (t.artist.as_str(), t.album.as_deref()),
            &mut crate::shuffle::Chance::from_clock(),
        );
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
    ask_ahead_after_seconds: f64,
    on_play: Option<OnPlay>,
    on_start: Option<OnPlay>,
    on_failed: Option<OnFailed>,
    on_equalizers: Option<OnEqualizers>,
    counts_between_seconds: (f64, f64),
    eq: Arc<Mutex<EqRun>>,
}

impl Ctx {
    /// The output chosen: `"auto"` for the system's own choice.
    fn chosen(&self) -> String {
        self.engine
            .mpv()
            .get("audio-device")
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "auto".into())
    }

    /// The engine opens a file (its `on_preloaded` hook): its equalizer goes in from its first sample. The engine
    /// waits until this is done, so it is quick: a property read and one write (and, for the system's choice before
    /// the engine said which output it opened, a question to the system). The engine is let go whatever happens here.
    fn preloaded(&self, id: u64) {
        struct Continue<'a>(&'a Engine, u64);
        impl Drop for Continue<'_> {
            fn drop(&mut self) {
                let _ = self.0.mpv().hook_continue(self.1);
            }
        }
        let _continue = Continue(&self.engine, id);
        let rate = self
            .engine
            .mpv()
            .get("track-list")
            .and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
            .and_then(|tracks| {
                tracks
                    .iter()
                    .find(|t| t["type"] == "audio")
                    .and_then(|t| t["demux-samplerate"].as_u64())
            })
            .and_then(|rate| u32::try_from(rate).ok());
        let chosen = self.chosen();
        let mut run = self
            .eq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if run.asks_system(&chosen) && run.opened.is_none() {
            run.system_said(crate::audio::system_output());
        }
        run.preloaded(&self.engine, &chosen, rate);
    }

    /// Asks the system which output it would choose, when that is what is chosen and nothing plays (the engine's
    /// output is closed then, and the next song opens what the system chooses at that moment). While a song plays,
    /// the output the engine said it opened is the one in use.
    fn ask_system(&self, run: &mut EqRun, chosen: &str, playing: bool) {
        if !run.asks_system(chosen) || playing && run.opened.is_some() {
            return;
        }
        if !playing {
            run.opened = None;
        }
        if run.system_due() {
            run.system_said(crate::audio::system_output());
        }
    }

    /// The queue place being opened, when its failure may be the equalizer's: it had not started, the equalizer's
    /// filters are in the engine's list, it is not being tried without them already, and it was not tried again
    /// before.
    fn equalizer_suspect(&self) -> Option<usize> {
        let place = {
            let shared = self.shared.lock().unwrap();
            if shared.started || shared.eq_suspect.is_some() {
                return None;
            }
            let place = shared.index().or(shared.starting)?;
            (!shared.retried.contains(&place)).then_some(place)
        }?;
        self.engine.filters().contains("@eq:").then_some(place)
    }

    /// A track is playing, or being opened.
    fn playing(&self) -> bool {
        let shared = self.shared.lock().unwrap();
        shared.slot().is_some() || shared.starting.is_some()
    }

    /// Hands the book to be kept, read when the keeper is ready for it.
    fn keep_equalizers(&self) {
        if let Some(on_equalizers) = &self.on_equalizers {
            on_equalizers(&|| self.eq.lock().unwrap().book.clone());
        }
    }

    /// The engine says it opened `device`: its equalizer goes in, and a new output opened last is kept.
    fn opened_output(&self, device: Device) {
        let chosen = self.chosen();
        let changed = self
            .eq
            .lock()
            .unwrap()
            .opened_output(&self.engine, &chosen, device);
        if changed {
            self.keep_equalizers();
        }
    }

    /// The engine could not use the equalizer: the list is cleared, the output's equalizer turned off and kept so.
    fn equalizer_failed(&self, why: &str) {
        let chosen = self.chosen();
        {
            let mut run = self.eq.lock().unwrap();
            let key = run.output_key(&chosen);
            let description = self.description(&key, &run.book);
            run.fail(&self.engine, &chosen, &description, why);
        }
        self.keep_equalizers();
    }

    /// What the system calls the output `name`; for one it lists no longer (unplugged), what was kept of it.
    fn description(&self, name: &str, book: &eq::Book) -> String {
        self.engine
            .mpv()
            .get("audio-device-list")
            .and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
            .and_then(|list| {
                list.iter()
                    .find(|d| d["name"] == name)
                    .and_then(|d| d["description"].as_str().map(tidy))
            })
            .or_else(|| {
                book.outputs
                    .get(name)
                    .map(|kept| kept.description.clone())
                    .filter(|d| !d.is_empty())
            })
            .unwrap_or_else(|| {
                if name == "auto" {
                    "Autoselect device".into()
                } else {
                    name.to_string()
                }
            })
    }

    /// The output in use (`run` held): the one the engine said it opened, else the one chosen, else what the system
    /// would choose.
    fn output_in_use(&self, run: &mut EqRun, playing: bool) -> Device {
        let chosen = self.chosen();
        self.ask_system(run, &chosen, playing);
        let name = run.output_key(&chosen);
        run.opened
            .clone()
            .filter(|d| d.name == name)
            .unwrap_or_else(|| Device {
                description: self.description(&name, &run.book),
                name,
            })
    }
    /// Called a few times a second: adds the time since `last` to the listening of the current track when the engine
    /// really played through it, tells of the track once it is heard, and of the play once it counts.
    fn listen(&self, last: &mut Instant) {
        let now = Instant::now();
        let passed = now.duration_since(*last).as_secs_f64().min(1.0);
        *last = now;
        if self.on_play.is_none() && self.on_start.is_none() {
            return;
        }
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
                    told: false,
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
        let (began, counted, started_at_ms) = {
            let mut shared = self.shared.lock().unwrap();
            let Some(heard) = shared.heard.as_mut() else {
                return;
            };
            heard.listened += passed;
            let began = !heard.told;
            heard.told = true;
            let counts = heard.listened >= needed;
            heard.counted = counts;
            if !began && !counts {
                return;
            }
            let (pos, started_at_ms) = (heard.pos, heard.started_at_ms);
            let Some(item) = shared
                .slots
                .get(pos)
                .and_then(|slot| shared.queue.get(slot.queue))
                .cloned()
            else {
                return;
            };
            (
                began.then(|| item.clone()),
                counts.then_some(item),
                started_at_ms,
            )
        };
        if let (Some(on_start), Some(item)) = (&self.on_start, began) {
            on_start(&item, started_at_ms);
        }
        if let (Some(on_play), Some(item)) = (&self.on_play, counted) {
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
            // With no add-ons to ask, a file that is not there is handed to the engine, which says so in its words.
            None if !item.locator.is_empty() => Ok(Located {
                locator: item.locator.clone(),
                tier: None,
                addon: None,
            }),
            None => Err(Failure {
                message: "this player has no add-ons".into(),
                ..Default::default()
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
    ///
    /// With `stop_first`, what plays stops at once (not when the add-on has answered), and only while this start is
    /// still the newest: the check and the stop are under the same hold of the player's state, so a start that began
    /// meanwhile is left playing.
    fn start(&self, generation: u64, mut stop_first: bool) {
        loop {
            let (item, place) = {
                let mut shared = self.shared.lock().unwrap();
                if shared.generation != generation {
                    return;
                }
                if std::mem::take(&mut stop_first) {
                    let _ = self.engine.stop();
                }
                let place = shared.ahead;
                match shared.queue.get(place).cloned() {
                    Some(item) => {
                        shared.starting = Some(place);
                        shared.last = Some(item.qid);
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
                            shared.fail(&item, &e.to_string());
                            shared.starting = None;
                        }
                    }
                    return;
                }
                Err(failure) => {
                    shared.fail(&item, &failure.message);
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
    fn extend(&self, token: (u64, u64)) {
        let (item, place) = {
            let mut shared = self.shared.lock().unwrap();
            if (shared.generation, shared.edits) != token {
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
        if (shared.generation, shared.edits) != token {
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
                    Err(e) => shared.fail(&item, &e.to_string()),
                }
                shared.ahead = place + 1;
                shared.tries = 0;
                shared.not_before = None;
            }
            Err(failure) if failure.miss => {
                shared.fail(&item, &failure.message);
                shared.ahead = place + 1;
                shared.tries = 0;
                shared.not_before = None;
            }
            Err(failure) => {
                shared.tries += 1;
                if shared.tries >= TRIES {
                    shared.fail(&item, &failure.message);
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
            shared.error_id = None;
            shared.generation
        };
        self.run(true, move |ctx| ctx.start(generation, true));
    }

    /// Called every time the engine reports something, and a few times a second besides. While a track plays it
    /// is noted as the last one. When nothing plays and nothing is coming (the queue ended, or its track could
    /// not be played), the player rests on the last track: the window keeps showing it, and play starts it again.
    fn settle(&self) {
        let mut shared = self.shared.lock().unwrap();
        if let Some(place) = shared.index() {
            shared.last = shared.queue.get(place).map(|t| t.qid);
            shared.rest = None;
            return;
        }
        if shared.starting.is_some() || shared.extending || shared.retry.is_some() {
            return;
        }
        let known = |shared: &Shared, qid: u64| shared.queue.iter().any(|t| t.qid == qid);
        if shared.rest.is_some_and(|(qid, _)| known(&shared, qid)) {
            return;
        }
        shared.rest = None;
        if shared.queue.is_empty() {
            return;
        }
        let nothing_coming =
            shared.slots.is_empty() || shared.ahead >= shared.queue.len() || shared.gave_up;
        if !nothing_coming {
            return;
        }
        let fallback = shared.ahead.min(shared.queue.len() - 1);
        let qid = shared
            .last
            .filter(|qid| known(&shared, *qid))
            .unwrap_or(shared.queue[fallback].qid);
        shared.rest = Some((qid, 0.0));
    }

    /// Called every time the engine reports something, and a few times a second besides: if the engine has nothing
    /// after the current track and the queue has, the next one is located and handed over.
    fn keep_ahead(&self) {
        let (token, waits) = {
            let mut shared = self.shared.lock().unwrap();
            // With the queue set to start over, the track after the last one is the first.
            if shared.repeat == Repeat::All
                && !shared.queue.is_empty()
                && shared.ahead >= shared.queue.len()
            {
                shared.ahead = 0;
            }
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
            (
                (shared.generation, shared.edits),
                shared.queue[shared.ahead].asks(),
            )
        };
        if waits && self.shared.lock().unwrap().pos.is_some() {
            // An add-on's track is asked for late. With no end in sight (a stream of unknown length) it is asked
            // for when the engine runs out.
            let remaining = self
                .engine
                .mpv()
                .get("time-remaining")
                .and_then(|v| v.parse::<f64>().ok());
            let played = self
                .engine
                .mpv()
                .get("time-pos")
                .and_then(|v| v.parse::<f64>().ok());
            let near_the_end = remaining.is_some_and(|r| r <= self.ask_ahead_seconds);
            let heard_enough = played.is_some_and(|p| p >= self.ask_ahead_after_seconds);
            if !near_the_end && !heard_enough {
                return;
            }
        }
        {
            let mut shared = self.shared.lock().unwrap();
            if (shared.generation, shared.edits) != token || shared.extending {
                return;
            }
            shared.extending = true;
        }
        self.run(waits, move |ctx| ctx.extend(token));
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
        // Every file's equalizer goes in as the engine opens it (hooks cannot be taken back: one for the player's life).
        engine.mpv().hook_add("on_preloaded", 0)?;
        let ctx = Ctx {
            engine,
            shared: Arc::new(Mutex::new(Shared::default())),
            resolver: settings.resolver,
            ask_ahead_seconds: settings.ask_ahead_seconds,
            ask_ahead_after_seconds: settings.ask_ahead_after_seconds,
            on_play: settings.on_play,
            on_start: settings.on_start,
            on_failed: settings.on_failed,
            on_equalizers: settings.on_equalizers,
            counts_between_seconds: settings.counts_between_seconds,
            eq: Arc::new(Mutex::new(EqRun::new(output))),
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
                        // Steps of the equalizer waiting to be sent are looked at more often.
                        let wait = if ctx.eq.lock().unwrap().busy() {
                            0.05
                        } else {
                            0.25
                        };
                        match ctx.engine.next_event(wait) {
                            Event::Hook { name, id } if name == "on_preloaded" => ctx.preloaded(id),
                            Event::Hook { id, .. } => {
                                let _ = ctx.engine.mpv().hook_continue(id);
                            }
                            // The output the engine opened ("the system's choice" may open another than last time).
                            Event::Log { prefix, text, .. }
                                if prefix.starts_with("ao")
                                    && eq::opened_output(&prefix, text.trim_end()).is_some() =>
                            {
                                if let Some((name, description)) =
                                    eq::opened_output(&prefix, text.trim_end())
                                {
                                    ctx.opened_output(Device {
                                        name,
                                        description: tidy(&description),
                                    });
                                }
                            }
                            // The engine could not build the filters, or one failed while running (it then passes the
                            // audio by it). The engine sends its log only when no event waits, so these words can come
                            // after the track they are about has ended: while a track opens they decide nothing (its end
                            // does, below) and are kept for the listener. Once a track plays, they are about a list
                            // written while it played, or a filter that failed: the equalizer is turned off.
                            Event::Log { text, .. }
                                if text.trim() == "Audio filter initialized failed!"
                                    || text.starts_with("Disabling filter")
                                        && text.trim_end().ends_with("because it has failed.") =>
                            {
                                let opening = {
                                    let mut shared = ctx.shared.lock().unwrap();
                                    shared.eq_said = Some(text.trim().to_string());
                                    !shared.started
                                };
                                if !opening && ctx.engine.filters().contains("@eq:") {
                                    ctx.equalizer_failed(text.trim());
                                }
                            }
                            // A seek the engine made itself (Repeat one's loop) built the graph again from the list
                            // as written: a seam, so the whole curve is written now. After Stash's own seeks and a
                            // file's start the list is in place already, and nothing is written.
                            Event::PlaybackRestart => {
                                // The track that could not be opened with the equalizer plays without it: the
                                // equalizer was at fault, and is turned off.
                                let blamed = {
                                    let mut shared = ctx.shared.lock().unwrap();
                                    shared.started = true;
                                    let place = shared.index().or(shared.starting);
                                    let suspect = shared.eq_suspect.take();
                                    let said = shared.eq_said.take();
                                    (suspect.is_some() && suspect == place).then(|| {
                                        said.unwrap_or_else(|| {
                                            "the engine could not build its filters".into()
                                        })
                                    })
                                };
                                if let Some(why) = blamed {
                                    ctx.equalizer_failed(&why);
                                }
                                let chosen = ctx.chosen();
                                ctx.eq.lock().unwrap().commit(&ctx.engine, &chosen);
                            }
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
                            // A track that could not be opened, with nothing to play, while the equalizer's filters
                            // were in the engine's list: the filters may be at fault, not the track. It is opened
                            // again once, without them, and nothing is said of it yet: if it plays, the equalizer was
                            // at fault (above); if not, the track is (below).
                            Event::EndFile(EndReason::Error(NOTHING_TO_PLAY))
                                if ctx.equalizer_suspect().is_some() =>
                            {
                                if let Some(place) = ctx.equalizer_suspect() {
                                    ctx.eq.lock().unwrap().hold(&ctx.engine);
                                    let mut shared = ctx.shared.lock().unwrap();
                                    shared.complaints.clear();
                                    shared.retried.insert(place);
                                    shared.retry = Some(place);
                                    shared.eq_suspect = Some(place);
                                }
                            }
                            Event::EndFile(EndReason::Error(code)) => {
                                let reason = ctx.engine.mpv().error_text(code);
                                let mut shared = ctx.shared.lock().unwrap();
                                let failed = shared
                                    .index()
                                    .or(shared.starting)
                                    .and_then(|i| shared.queue.get(i));
                                let failed_id = failed.and_then(|t| t.id);
                                let failed_item = failed.cloned();
                                let what = failed
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
                                shared.error_id = failed_id;
                                shared.errors += 1;
                                shared.eq_suspect = None;
                                shared.eq_said = None;
                                // A stream that did not start is asked for once more: its link may have run out, or
                                // the first try at its server failed. A file that cannot be played stays that way.
                                let place = shared.index().or(shared.starting);
                                if let Some(place) = place
                                    && shared.queue.get(place).is_some_and(|t| t.asks())
                                    && shared.retried.insert(place)
                                {
                                    shared.retry = Some(place);
                                }
                                // Told before it is asked for again: what it was given may be what failed.
                                drop(shared);
                                if let (Some(item), Some(on_failed)) = (failed_item, &ctx.on_failed)
                                {
                                    on_failed(&item);
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
                                shared.started = false;
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
                        ctx.settle();
                        {
                            let mut run = ctx.eq.lock().unwrap();
                            if run.busy() {
                                let position = ctx
                                    .engine
                                    .mpv()
                                    .get("time-pos")
                                    .and_then(|p| p.parse::<f64>().ok());
                                run.tick(&ctx.engine, position);
                            }
                        }
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
        self.play_from(items, 0, None)
    }

    /// Replaces the queue with `items` and plays from place `start`; `from` says what the queue is, in the
    /// listener's words. With shuffle on, the track at `start` plays first and the rest follow in a new order.
    pub fn play_from(&self, items: Vec<QueueItem>, start: usize, from: Option<String>) -> Result<()> {
        self.play_from_at(items, start, from, 0.0, true).map(|_| ())
    }

    /// Replaces the queue with `items`, as [`Player::play_from`] does, and starts the track at `start` from `seconds`
    /// in. The place is taken once the track has loaded, so it holds for a track an add-on is still being asked for (a
    /// seek asked right after a start could land on the track before it). With `play` false nothing plays: the player
    /// rests on that track, `seconds` in, and play starts it from there. Answers what was queued.
    pub fn play_from_at(
        &self,
        mut items: Vec<QueueItem>,
        start: usize,
        from: Option<String>,
        seconds: f64,
        play: bool,
    ) -> Result<Queued> {
        if items.is_empty() {
            self.stop()?;
            let queue_id = self.ctx.shared.lock().unwrap().queue_id;
            return Ok(Queued { queue_id, qids: Vec::new(), start: None });
        }
        let seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
        let mut start = start.min(items.len() - 1);
        // The queue is put in place and its start begun under one hold: nothing else starts in between.
        let (queued, begun) = {
            let mut shared = self.ctx.shared.lock().unwrap();
            shared.number(&mut items);
            shared.original = None;
            if shared.shuffle {
                shared.original = Some(items.iter().map(|t| t.qid).collect());
                items.swap(0, start);
                start = 0;
                Shared::shuffle_from(&mut items, 1);
            }
            shared.queue = items;
            shared.from = from;
            shared.version += 1;
            shared.queue_id += 1;
            let queued = Queued {
                queue_id: shared.queue_id,
                qids: shared.queue.iter().map(|t| t.qid).collect(),
                start: Some(shared.queue[start].qid),
            };
            let begun = Self::begin(&mut shared, start, Some(seconds));
            if !play {
                // Nothing starts: the player rests on the track, `seconds` in (what the engine plays is stopped below).
                shared.starting = None;
                shared.pending_seek = None;
                shared.rest = Some((shared.queue[start].qid, seconds));
            }
            (queued, begun)
        };
        if !play {
            self.ctx.engine.stop()?;
        } else if let Some((generation, waits)) = begun {
            self.go(generation, waits);
        }
        Ok(queued)
    }

    /// Replaces what comes after the playing track with `items`: the track goes on playing, as the first of a queue
    /// that is new (`from` says what it is), so what played before it is let go too. False when nothing plays.
    pub fn play_after_current(&self, mut items: Vec<QueueItem>, from: Option<String>) -> bool {
        let start = {
            let mut shared = self.ctx.shared.lock().unwrap();
            let Some(current) = shared.index().or(shared.starting) else {
                return false;
            };
            let Some(playing) = shared.queue.get(current).cloned() else {
                return false;
            };
            shared.number(&mut items);
            let mut queue = Vec::with_capacity(items.len() + 1);
            queue.push(playing);
            queue.extend(items);
            shared.queue = queue;
            shared.original = None;
            shared.from = from;
            shared.queue_id += 1;
            self.after_edit(&mut shared, Some(current), Some(0))
        };
        if let Some(place) = start {
            self.start_at(place);
        }
        true
    }

    /// After the queue was edited while it plays: what the engine holds beyond the current track is taken back
    /// (the next track is handed over again from the new order), and the current track's place is set to `now`.
    /// Answers a place to start playing from, when the track being started is no longer where it was.
    fn after_edit(
        &self,
        shared: &mut Shared,
        was: Option<usize>,
        now: Option<usize>,
    ) -> Option<usize> {
        shared.version += 1;
        shared.edits += 1;
        shared.extending = false;
        shared.tries = 0;
        shared.not_before = None;
        shared.gave_up = false;
        shared.retried.clear();
        let mpv = self.ctx.engine.mpv();
        match (shared.pos.filter(|p| *p < shared.slots.len()), now) {
            (Some(at), Some(now)) => {
                for entry in (at + 1..shared.slots.len()).rev() {
                    let _ = mpv.command(&["playlist-remove", &entry.to_string()]);
                }
                shared.slots.truncate(at + 1);
                // The places of tracks already played are no longer known.
                for slot in &mut shared.slots[..at] {
                    slot.queue = usize::MAX;
                }
                shared.slots[at].queue = now;
                shared.ahead = now + 1;
                None
            }
            // A track is being started: if it is no longer where it was, it is started again from its new place.
            _ if shared.starting.is_some() => (was != now).then_some(now).flatten(),
            // Nothing plays: the engine holds nothing of the queue any more.
            _ => {
                let _ = mpv.command(&["playlist-clear"]);
                shared.slots.clear();
                shared.pos = None;
                shared.ahead = shared.ahead.min(shared.queue.len());
                None
            }
        }
    }

    /// Adds `items` to the queue: right after the playing track (`next`), or at the end. With an empty queue they
    /// start to play; with the player resting on a track they are only added. Answers the queue numbers they got.
    pub fn enqueue(&self, items: Vec<QueueItem>, next: bool) -> Vec<u64> {
        self.enqueue_when(items, next, |_| true).unwrap_or_default()
    }

    /// Adds `items` at the end of the queue, as [`Player::enqueue`] does, only while the queue is the one `queue_id`
    /// names (see [`PlayerState::queue_id`]) and, with `after`, its last track is that one (by its queue number): songs
    /// found for one queue are not added to another. `None` when they were not added.
    pub fn enqueue_onto(
        &self,
        items: Vec<QueueItem>,
        queue_id: u64,
        after: Option<u64>,
    ) -> Option<Vec<u64>> {
        self.enqueue_when(items, false, |shared| {
            shared.queue_id == queue_id
                && after.is_none_or(|qid| shared.queue.last().map(|t| t.qid) == Some(qid))
        })
    }

    fn enqueue_when(
        &self,
        mut items: Vec<QueueItem>,
        next: bool,
        may: impl Fn(&Shared) -> bool,
    ) -> Option<Vec<u64>> {
        let qids: Vec<u64>;
        let start = {
            let mut shared = self.ctx.shared.lock().unwrap();
            if !may(&shared) {
                return None;
            }
            if items.is_empty() {
                return Some(Vec::new());
            }
            shared.number(&mut items);
            qids = items.iter().map(|t| t.qid).collect();
            let current = shared.index().or(shared.starting);
            let resting = shared.resting().map(|(place, _)| place);
            let at = match (current.or(resting), next) {
                (Some(current), true) => current + 1,
                _ => shared.queue.len(),
            };
            if let Some(original) = shared.original.as_mut() {
                original.extend(items.iter().map(|t| t.qid));
            }
            shared.queue.splice(at..at, items);
            let restart = self.after_edit(&mut shared, current, current);
            match (current, resting) {
                (Some(_), _) => restart,
                // Resting on a track (the queue had ended, or was brought back): the queue grows, nothing starts.
                (None, Some(_)) => None,
                (None, None) => Some(at),
            }
        };
        if let Some(place) = start {
            self.start_at(place);
        }
        Some(qids)
    }

    /// Takes the track at `place` out of the queue. The playing track taken out makes way for the one after it.
    pub fn remove_at(&self, place: usize) -> bool {
        enum Then {
            Nothing,
            Start(usize),
            End,
        }
        let then = {
            let mut shared = self.ctx.shared.lock().unwrap();
            if place >= shared.queue.len() {
                return false;
            }
            let current = shared.index().or(shared.starting);
            let removed = shared.queue.remove(place);
            if let Some(original) = shared.original.as_mut() {
                original.retain(|qid| *qid != removed.qid);
            }
            if current == Some(place) {
                shared.version += 1;
                if place < shared.queue.len() {
                    Then::Start(place)
                } else {
                    shared.generation += 1;
                    shared.slots.clear();
                    shared.pos = None;
                    shared.starting = None;
                    shared.ahead = shared.queue.len();
                    Then::End
                }
            } else {
                let now = current.map(|c| if place < c { c - 1 } else { c });
                match self.after_edit(&mut shared, current, now) {
                    Some(start) => Then::Start(start),
                    None => Then::Nothing,
                }
            }
        };
        match then {
            Then::Nothing => {}
            Then::Start(place) => self.start_at(place),
            Then::End => {
                let _ = self.ctx.engine.stop();
            }
        }
        true
    }

    /// Takes every track `keep` says no to out of the queue, wherever it is (before the playing track too: the queue may
    /// start over), in one go. The playing (or starting) track taken out makes way for the next track kept, or
    /// playback ends when none is; the track the player rests on, for the next one kept (or the last one left). Answers
    /// how many went.
    ///
    /// All of it happens under one hold of the player's state, a new start's own bookkeeping too: two calls at once
    /// (from two threads) each see a queue whose places hold.
    pub fn retain(&self, keep: impl Fn(&QueueItem) -> bool) -> usize {
        enum Then {
            Nothing,
            /// A start begun: its generation, and whether it waits for an add-on.
            Start(u64, bool),
        }
        let (gone, then) = {
            let mut shared = self.ctx.shared.lock().unwrap();
            let out: Vec<bool> = shared.queue.iter().map(|t| !keep(t)).collect();
            let gone = out.iter().filter(|o| **o).count();
            if gone == 0 {
                return 0;
            }
            let current = shared
                .index()
                .or(shared.starting)
                .filter(|&c| c < out.len());
            let resting = shared.resting().map(|(place, _)| place);
            let begin = |shared: &mut Shared, place: usize| match Self::begin(shared, place, None) {
                Some((generation, waits)) => Then::Start(generation, waits),
                None => Then::Nothing,
            };
            // Where a kept track ends up: after the kept tracks before it. And the first track kept from a place on.
            let kept_before = |place: usize| out[..place].iter().filter(|o| !**o).count();
            let next_kept = |place: usize| (place..out.len()).find(|&p| !out[p]).map(kept_before);
            let dropped: std::collections::HashSet<u64> = shared
                .queue
                .iter()
                .zip(&out)
                .filter(|(_, o)| **o)
                .map(|(t, _)| t.qid)
                .collect();
            shared.queue.retain(|t| !dropped.contains(&t.qid));
            if let Some(original) = shared.original.as_mut() {
                original.retain(|qid| !dropped.contains(qid));
            }
            let then = match current {
                Some(c) if out[c] => {
                    shared.version += 1;
                    match next_kept(c) {
                        Some(place) => begin(&mut shared, place),
                        None => {
                            shared.generation += 1;
                            shared.slots.clear();
                            shared.pos = None;
                            shared.starting = None;
                            shared.ahead = shared.queue.len();
                            // Under the same hold, so a start begun just after is not stopped.
                            let _ = self.ctx.engine.stop();
                            Then::Nothing
                        }
                    }
                }
                Some(c) => match self.after_edit(&mut shared, current, Some(kept_before(c))) {
                    Some(start) => begin(&mut shared, start),
                    None => Then::Nothing,
                },
                None => {
                    // Resting on a track that went: on the next one kept, else on the last one left (as a queue that
                    // played out rests on its last track).
                    if let Some(rest) = resting.filter(|r| out[*r]) {
                        let next = next_kept(rest)
                            .map(|place| shared.queue[place].qid)
                            .or_else(|| shared.queue.last().map(|t| t.qid));
                        shared.rest = next.map(|qid| (qid, 0.0));
                    }
                    let _ = self.after_edit(&mut shared, None, None);
                    Then::Nothing
                }
            };
            (gone, then)
        };
        if let Then::Start(generation, waits) = then {
            self.go(generation, waits);
        }
        gone
    }

    /// Takes back what the engine was handed beyond the track playing, so the tracks after it are asked for again: what
    /// may play has changed (Stash went offline). The track playing goes on.
    pub fn ask_again_ahead(&self) {
        let start = {
            let mut shared = self.ctx.shared.lock().unwrap();
            let current = shared.index().or(shared.starting);
            self.after_edit(&mut shared, current, current)
        };
        if let Some(place) = start {
            self.start_at(place);
        }
    }

    /// Moves the track at place `from` to place `to`. The playing track goes on playing wherever it ends up.
    pub fn move_item(&self, from: usize, to: usize) -> bool {
        let start = {
            let mut shared = self.ctx.shared.lock().unwrap();
            if from >= shared.queue.len() || to >= shared.queue.len() || from == to {
                return false;
            }
            let current = shared.index().or(shared.starting);
            let item = shared.queue.remove(from);
            shared.queue.insert(to, item);
            let now = current.map(|c| {
                if c == from {
                    to
                } else {
                    let after_removal = if from < c { c - 1 } else { c };
                    if to <= after_removal {
                        after_removal + 1
                    } else {
                        after_removal
                    }
                }
            });
            self.after_edit(&mut shared, current, now)
        };
        if let Some(place) = start {
            self.start_at(place);
        }
        true
    }

    /// Shuffle on: the tracks after the playing one follow in a new order. Off: the queue goes back to the order
    /// it had (tracks added meanwhile stay at its end). The playing track goes on playing.
    pub fn set_shuffle(&self, on: bool) {
        let start = {
            let mut shared = self.ctx.shared.lock().unwrap();
            if shared.shuffle == on {
                return;
            }
            shared.shuffle = on;
            if shared.queue.is_empty() {
                return;
            }
            let current = shared.index().or(shared.starting);
            let playing = current.and_then(|c| shared.queue.get(c)).map(|t| t.qid);
            if on {
                shared.original = Some(shared.queue.iter().map(|t| t.qid).collect());
                let from = current.map(|c| c + 1).unwrap_or(0);
                Shared::shuffle_from(&mut shared.queue, from);
            } else if let Some(original) = shared.original.take() {
                let place = |qid: u64| {
                    original
                        .iter()
                        .position(|q| *q == qid)
                        .unwrap_or(usize::MAX)
                };
                shared.queue.sort_by_key(|t| place(t.qid));
            }
            let now = playing.and_then(|qid| shared.queue.iter().position(|t| t.qid == qid));
            self.after_edit(&mut shared, current, now)
        };
        if let Some(place) = start {
            self.start_at(place);
        }
    }

    /// What happens at the end: nothing, the queue starts over, or the track starts over.
    pub fn set_repeat(&self, repeat: Repeat) {
        let _ = self.ctx.engine.mpv().set(
            "loop-file",
            if repeat == Repeat::One { "inf" } else { "no" },
        );
        let mut shared = self.ctx.shared.lock().unwrap();
        if shared.repeat == repeat {
            return;
        }
        shared.repeat = repeat;
        // What was handed over for the old setting (the first track after the last, or nothing) is taken back.
        let current = shared.index().or(shared.starting);
        self.after_edit(&mut shared, current, current);
    }

    /// Plays the queue from `place` on, in place of whatever plays.
    fn start_at(&self, place: usize) {
        self.start_at_from(place, None)
    }

    /// Plays the queue from `place` on, and that track from `seconds` in.
    fn start_at_from(&self, place: usize, seconds: Option<f64>) {
        let begun = Self::begin(&mut self.ctx.shared.lock().unwrap(), place, seconds);
        if let Some((generation, waits)) = begun {
            self.go(generation, waits);
        }
    }

    /// A start's bookkeeping, under the hold of the player's state: the engine's tracks are let go, and the track at
    /// `place` is the one starting. Answers the start's generation and whether it waits for an add-on; None when there
    /// is no such place. [`Player::go`] hands it to the engine, once the state is let go.
    fn begin(shared: &mut Shared, place: usize, seconds: Option<f64>) -> Option<(u64, bool)> {
        {
            if place >= shared.queue.len() {
                return None;
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
            shared.error_id = None;
            shared.pending_seek = seconds.filter(|s| *s > 0.05);
            shared.retried.clear();
            shared.retry = None;
            shared.rest = None;
            shared.last = Some(shared.queue[place].qid);
            Some((shared.generation, shared.queue[place].asks()))
        }
    }

    /// Hands a start begun (see [`Player::begin`]) to the engine.
    fn go(&self, generation: u64, waits: bool) {
        // A start that waits for an add-on stops what plays at once, if it is still the newest (see `Ctx::start`).
        self.ctx.run(waits, move |ctx| ctx.start(generation, waits));
    }

    /// Pauses, or plays on. Resting on a track, playing on starts that track from where it rests.
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        let resting = self.ctx.shared.lock().unwrap().resting();
        match resting {
            Some(_) if paused => Ok(()),
            Some((place, seconds)) => {
                self.start_at_from(place, Some(seconds));
                Ok(())
            }
            None => self.ctx.engine.set_paused(paused),
        }
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
                (_, false, None) => match shared.resting() {
                    // Resting on the last track with nothing after it: it stays.
                    Some((place, _))
                        if place + 1 >= shared.queue.len() && shared.repeat != Repeat::All =>
                    {
                        return Ok(());
                    }
                    Some((place, _)) => Some(place + 1),
                    None => return Ok(()),
                },
            }
        };
        match target {
            None => self.ctx.engine.mpv().command(&["playlist-next", "weak"]),
            Some(place) if place < self.ctx.shared.lock().unwrap().queue.len() => {
                self.start_at(place);
                Ok(())
            }
            // Past the end with the queue set to start over: the first track.
            Some(_) if self.ctx.shared.lock().unwrap().repeat == Repeat::All => {
                self.start_at(0);
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
                (None, None) => match shared.resting() {
                    Some((place, _)) if place > 0 => Back::To(place - 1),
                    Some((place, _)) => Back::To(place),
                    None => return Ok(()),
                },
                (Some(0), _) | (None, Some(0)) => Back::Restart,
                (None, Some(starting)) => Back::To(starting - 1),
                (Some(current), _) => {
                    let before = shared
                        .pos
                        .filter(|p| *p > 0)
                        .and_then(|p| shared.slots.get(p - 1))
                        .map(|s| s.queue);
                    match before.filter(|place| *place < current) {
                        // The engine still has the file before this one.
                        Some(place) if place + 1 == current && !shared.queue[place].asks() => {
                            Back::Engine
                        }
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
        {
            // Resting on a track: the place in it moves, and play goes on from there.
            let mut shared = self.ctx.shared.lock().unwrap();
            if shared.resting().is_some() {
                if let Some(rest) = shared.rest.as_mut() {
                    rest.1 = seconds;
                }
                return Ok(());
            }
        }
        self.commit_eq();
        if self.ctx.engine.seek_to(seconds).is_err() {
            self.ctx.shared.lock().unwrap().pending_seek = Some(seconds);
        }
        Ok(())
    }

    /// A seam: the engine is about to drop the audio it queued (a seek, the output opened again), so the list is
    /// written as planned now, and what is built again after is the equalizer as it should be.
    fn commit_eq(&self) {
        let chosen = self.ctx.chosen();
        self.ctx
            .eq
            .lock()
            .unwrap()
            .commit(&self.ctx.engine, &chosen);
    }

    /// Sets the volume, 0 to 100. It is the engine's own volume, so in exclusive mode anything but 100 changes the
    /// samples on their way out.
    pub fn set_volume(&self, percent: f64) -> Result<()> {
        self.ctx
            .engine
            .mpv()
            .set("volume", &format!("{:.0}", percent.clamp(0.0, 100.0)))
    }

    /// The sound outputs the system offers: the system's own choice (`"auto"`) first, then each output of the
    /// system's sound driver. (Outputs of the engine's fallback drivers are left out: they are the same outputs
    /// again, and exclusive mode does not reach through them.)
    pub fn devices(&self) -> Vec<Device> {
        let listed: Vec<serde_json::Value> = self
            .ctx
            .engine
            .mpv()
            .get("audio-device-list")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let all: Vec<Device> = listed
            .iter()
            .filter_map(|d| {
                Some(Device {
                    name: d["name"].as_str()?.to_string(),
                    description: tidy(d["description"].as_str().unwrap_or_default()),
                })
            })
            .collect();
        let driver = all
            .iter()
            .find(|d| d.name != "auto")
            .map(|d| d.name.split('/').next().unwrap_or_default().to_string());
        let mut devices: Vec<Device> = all
            .into_iter()
            .filter(|d| {
                d.name == "auto"
                    || driver
                        .as_deref()
                        .is_some_and(|driver| d.name.split('/').next() == Some(driver))
            })
            .collect();
        if !devices.iter().any(|d| d.name == "auto") {
            devices.insert(
                0,
                Device {
                    name: "auto".into(),
                    description: "Autoselect device".into(),
                },
            );
        }
        devices
    }

    /// The output the sound goes to: `"auto"` for the system's own choice.
    pub fn device(&self) -> String {
        self.ctx
            .engine
            .mpv()
            .get("audio-device")
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "auto".into())
    }

    /// Sends the sound to the output `name` (one of [`Player::devices`], or `"auto"`). The output is opened again
    /// at once, also in the middle of a track.
    pub fn set_device(&self, name: &str) -> Result<()> {
        let mpv = self.ctx.engine.mpv();
        mpv.set("audio-device", name)?;
        // That output's equalizer goes in as it opens (until the engine says which it opened).
        self.ctx.eq.lock().unwrap().opened = None;
        self.commit_eq();
        let _ = mpv.command(&["ao-reload"]);
        Ok(())
    }

    /// Takes the sound card for Stash alone, or gives it back. The output is opened again at once, also in the
    /// middle of a track.
    pub fn set_exclusive(&self, exclusive: bool) -> Result<()> {
        let mpv = self.ctx.engine.mpv();
        self.ctx.engine.set_exclusive(exclusive)?;
        self.commit_eq();
        // With nothing playing there is no output to open again: the setting holds for the next track.
        let _ = mpv.command(&["ao-reload"]);
        Ok(())
    }

    /// Evens out loudness between songs from the gain their tags carry (ReplayGain): `"track"` makes every song
    /// equally loud, `"album"` keeps the differences between the songs of an album, `"off"` plays them as they are.
    /// Heard at once, also in the middle of a song. A song without the tags plays as it is.
    pub fn set_replaygain(&self, mode: &str) -> Result<()> {
        let mode = match mode {
            "track" | "album" => mode,
            _ => "no",
        };
        self.ctx.engine.mpv().set("replaygain", mode)?;
        Ok(())
    }

    /// How fast songs play, `MIN_SPEED` to `MAX_SPEED` (1 as recorded); the pitch stays where it is. Heard at once.
    pub fn set_speed(&self, speed: f64) -> Result<()> {
        let speed = if speed.is_finite() {
            speed.clamp(MIN_SPEED, MAX_SPEED)
        } else {
            1.0
        };
        self.ctx.engine.mpv().set("speed", &format!("{speed:.3}"))?;
        Ok(())
    }

    /// Tuning: the A that songs are heard at, in Hz (`STANDARD_HZ` as recorded, 432 a little lower), within
    /// `MIN_HZ`..`MAX_HZ` (about two semitones either way) to a tenth. The speed stays where it is. Heard at once.
    pub fn set_tuning(&self, hz: f64) -> Result<()> {
        let hz = tuning_hz(hz);
        self.ctx
            .engine
            .mpv()
            .set("pitch", &format!("{:.6}", hz / STANDARD_HZ))?;
        Ok(())
    }

    /// What each output keeps, when the player starts: the equalizer of the output expected goes in.
    pub fn set_equalizers(&self, book: eq::Book) {
        let chosen = self.ctx.chosen();
        let mut run = self.ctx.eq.lock().unwrap();
        run.book = book;
        run.commit(&self.ctx.engine, &chosen);
    }

    /// What each output keeps.
    pub fn equalizers(&self) -> eq::Book {
        self.ctx.eq.lock().unwrap().book.clone()
    }

    /// The output in use: the one the engine said it opened, else the one chosen (for the system's choice, the one
    /// the system says it would choose).
    pub fn output_in_use(&self) -> Device {
        let playing = self.ctx.playing();
        let mut run = self.ctx.eq.lock().unwrap();
        self.ctx.output_in_use(&mut run, playing)
    }

    /// The equalizer of the output in use.
    pub fn eq(&self) -> eq::Eq {
        let playing = self.ctx.playing();
        let chosen = self.ctx.chosen();
        let mut run = self.ctx.eq.lock().unwrap();
        self.ctx.ask_system(&mut run, &chosen, playing);
        run.eq(&chosen)
    }

    /// Sets the equalizer of the output in use and keeps it in what each output keeps. While a song plays it glides
    /// there; when that starts with a new list and the engine refuses it, this answers the refusal and nothing changes.
    pub fn set_eq(&self, eq: eq::Eq) -> Result<()> {
        let playing = self.ctx.playing();
        let chosen = self.ctx.chosen();
        let mut run = self.ctx.eq.lock().unwrap();
        let output = self.ctx.output_in_use(&mut run, playing);
        let before = run.book.clone();
        run.book.put(&output.name, &output.description, eq);
        if let Err(e) = run.change(&self.ctx.engine, &chosen, playing) {
            run.book = before;
            return Err(e);
        }
        Ok(())
    }

    /// Tests only: `list` becomes the engine's filter list, as it is, for every file until the equalizer is changed or
    /// fails (the fail-safe's test).
    #[doc(hidden)]
    pub fn set_filters_unchecked(&self, list: &str) {
        let mut run = self.ctx.eq.lock().unwrap();
        run.forced = Some(list.to_string());
        let _ = self.ctx.engine.set_filters(Some(list));
    }

    /// Tests only: the engine's filter list as it reads it back ("" for none).
    #[doc(hidden)]
    pub fn filters(&self) -> String {
        self.ctx.engine.filters()
    }

    /// The queue from `offset` on.
    pub fn queue_at(&self, offset: usize, limit: usize) -> Vec<QueueItem> {
        let shared = self.ctx.shared.lock().unwrap();
        shared
            .queue
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Plays the queue from `place` on. False when the queue has no such place.
    pub fn jump(&self, place: usize) -> bool {
        if place >= self.ctx.shared.lock().unwrap().queue.len() {
            return false;
        }
        self.start_at(place);
        true
    }

    /// Brings a queue back as it was left: its tracks, the place in it, shuffle and repeat. Nothing plays; the
    /// player rests on the track at `place`, and play goes on from `seconds` in it.
    pub fn restore(&self, mut kept: Kept) {
        if kept.items.is_empty() {
            return;
        }
        self.set_repeat(kept.repeat);
        let mut shared = self.ctx.shared.lock().unwrap();
        shared.number(&mut kept.items);
        let place = kept.place.min(kept.items.len() - 1);
        let qid = kept.items[place].qid;
        shared.shuffle = kept.shuffle;
        shared.original = kept.original.filter(|_| kept.shuffle).map(|order| {
            order
                .into_iter()
                .filter_map(|i| kept.items.get(i).map(|t| t.qid))
                .collect()
        });
        shared.queue = kept.items;
        shared.from = kept.from;
        shared.generation += 1;
        shared.version += 1;
        shared.queue_id += 1;
        shared.slots.clear();
        shared.pos = None;
        shared.starting = None;
        shared.ahead = place;
        shared.rest = Some((qid, kept.seconds.max(0.0)));
        shared.last = Some(qid);
    }

    /// The queue as it is, to keep it: its tracks and, while shuffled, the order they had before. The place and
    /// the seconds are the state's (`index`, `position_ms`).
    pub fn keep(&self) -> (u64, Vec<QueueItem>, Option<Vec<usize>>) {
        let shared = self.ctx.shared.lock().unwrap();
        let original = shared.original.as_ref().map(|order| {
            let places: std::collections::HashMap<u64, usize> = shared
                .queue
                .iter()
                .enumerate()
                .map(|(place, t)| (t.qid, place))
                .collect();
            order
                .iter()
                .filter_map(|qid| places.get(qid).copied())
                .collect()
        });
        (shared.version, shared.queue.clone(), original)
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
            shared.original = None;
            shared.from = None;
            shared.rest = None;
            shared.last = None;
            shared.version += 1;
            shared.queue_id += 1;
        }
        self.ctx.engine.stop()
    }

    pub fn state(&self) -> PlayerState {
        // The engine moves to the track it has ready (`playlist-next`) before its event says so, and its position is
        // the new track's at once: its own word, read now, is newer than the one kept (else a skip would report the
        // track left as started over).
        let engine_pos = self
            .ctx
            .engine
            .mpv()
            .get("playlist-pos")
            .and_then(|v| v.parse::<i64>().ok())
            .and_then(|i| usize::try_from(i).ok());
        let (
            queue_len,
            index,
            starting,
            asking,
            track,
            tier,
            served_by,
            accepted,
            (error, error_id, errors),
            queue,
            resting,
            nothing_after,
        ) = {
            let shared = self.ctx.shared.lock().unwrap();
            // Only while a track plays: during a start the engine's list is being replaced, and its word is old.
            let pos = match engine_pos {
                Some(p) if shared.pos.is_some() && p < shared.slots.len() => Some(p),
                _ => shared.pos,
            };
            let slot = pos.and_then(|p| shared.slots.get(p));
            let index = slot.map(|s| s.queue);
            let resting = shared.resting();
            let track = index
                .or(shared.starting)
                .or(resting.map(|(place, _)| place))
                .and_then(|i| shared.queue.get(i))
                .cloned();
            (
                shared.queue.len(),
                index,
                shared.starting,
                // A file that is starting is as good as playing; a track that has to be asked for is not yet.
                track.as_ref().is_some_and(|t| t.asks()),
                track,
                slot.and_then(|s| s.tier.clone()),
                slot.and_then(|s| s.addon.clone()),
                shared.accepted.clone(),
                (shared.error.clone(), shared.error_id, shared.errors),
                (
                    shared.version,
                    shared.queue_id,
                    shared.shuffle,
                    shared.repeat,
                    shared.from.clone(),
                ),
                resting,
                shared.ahead >= shared.queue.len()
                    && pos.is_some_and(|pos| pos + 1 >= shared.slots.len()),
            )
        };
        let (queue_version, queue_id, shuffle, repeat, from) = queue;
        let mpv = self.ctx.engine.mpv();
        let seconds = |name: &str| {
            mpv.get(name)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|s| (s.max(0.0) * 1000.0) as u64)
                .unwrap_or(0)
        };
        let number = |name: &str| mpv.get(name).and_then(|v| v.parse::<u32>().ok());
        let playing = index.is_some();
        let volume = mpv
            .get("volume")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(100.0);
        let speed = mpv
            .get("speed")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|s| s.is_finite())
            .unwrap_or(1.0);
        let tuning = mpv
            .get("pitch")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|p| tuning_hz(p * STANDARD_HZ))
            .unwrap_or(STANDARD_HZ);
        let replaygain = match mpv.get("replaygain").as_deref() {
            Some("track") => "track",
            Some("album") => "album",
            _ => "off",
        };
        let resting_length = track.as_ref().and_then(|t| t.duration_ms).unwrap_or(0);
        let chosen = self.ctx.chosen();
        let (eq, failed) = {
            let mut run = self.ctx.eq.lock().unwrap();
            let output = self.ctx.output_in_use(&mut run, playing);
            (run.state(output, chosen == "auto"), run.failed)
        };
        let equalized = playing && !failed && self.ctx.engine.filters().contains("@eq:");
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
                scaled: (volume - 100.0).abs() > 0.01 || {
                    // The engine evens a track out when its tags carry a track gain; by album, the album's gain
                    // when there is one.
                    let gain = |key: &str| {
                        mpv.get(&format!("metadata/by-key/{key}"))
                            .and_then(|text| gain_db(&text))
                            .is_some_and(|db| db.abs() >= 0.005)
                    };
                    let track = gain("REPLAYGAIN_TRACK_GAIN") || gain("R128_TRACK_GAIN");
                    let album = gain("REPLAYGAIN_ALBUM_GAIN") || gain("R128_ALBUM_GAIN");
                    match replaygain {
                        "track" => track,
                        "album" => track || album,
                        _ => false,
                    }
                },
                equalized,
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
                    // Resting on a track is being paused in it: play goes on from there.
                    None if resting.is_some() => "paused",
                    None => "idle",
                }
            } else if mpv.get("pause").as_deref() == Some("yes") {
                "paused"
            } else {
                "playing"
            },
            index: index.or(starting).or(resting.map(|(place, _)| place)),
            queue_len,
            queue_version,
            queue_id,
            shuffle,
            repeat: repeat.as_str(),
            from,
            track,
            position_ms: match resting {
                _ if playing => seconds("time-pos"),
                Some((_, at)) => (at * 1000.0) as u64,
                None => 0,
            },
            duration_ms: match resting {
                _ if playing => seconds("duration"),
                Some(_) => resting_length,
                None => 0,
            },
            tier,
            served_by,
            volume: volume.round().clamp(0.0, 100.0) as u32,
            exclusive: self.ctx.engine.exclusive(),
            replaygain,
            speed: (speed * 1000.0).round() / 1000.0,
            tuning,
            eq,
            signal,
            error,
            error_id,
            errors,
            resting: resting.is_some(),
            nothing_after,
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

#[cfg(test)]
mod tuning_tests {
    use super::*;

    #[test]
    fn a_tuning_is_kept_within_two_semitones_to_a_tenth() {
        assert_eq!(tuning_hz(432.0), 432.0);
        assert_eq!(tuning_hz(431.54), 431.5);
        assert_eq!(tuning_hz(999.0), MAX_HZ);
        assert_eq!(tuning_hz(100.0), MIN_HZ);
        assert_eq!(tuning_hz(f64::NAN), STANDARD_HZ);
        assert_eq!(tuning_hz(f64::INFINITY), STANDARD_HZ);
    }
}
