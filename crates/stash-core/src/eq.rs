//! The equalizer: a correction from a file made for the listener's headphones or speakers (Equalizer APO's text, as
//! autoeq.app and Room EQ Wizard write it), and Bass and Treble, each Less, As is or More. Each output keeps its own,
//! in the book.
//!
//! Off, or nothing set, means no filter at all, so the music reaches the output as it was made. On, the engine's
//! filter list holds two entries: `@frames`, which hands the music on in frames of 256 samples and is never taken out
//! mid-song, and `@eq`: a preamp that glides, the correction's filters, the Bass and Treble shelves, and float out.
//! The preamp leaves room for the boosts, so nothing clips on the way.
//!
//! A change while a song plays glides in through the filters' own commands, in an order that never outruns that room;
//! another correction goes through flat, where the entry can be swapped without a sound. Everything here is worked out
//! on paper: the player carries the steps out.

use std::collections::BTreeMap;
use std::f64::consts::{LN_2, PI};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Bass: a low shelf at this many Hz, with this Q, raised (More) or lowered (Less) by this many dB. Tune by ear
/// before release.
pub const BASS: (f64, f64, f64) = (105.0, 0.71, 4.0);
/// Treble: a high shelf, the same way.
pub const TREBLE: (f64, f64, f64) = (6000.0, 0.71, 3.0);
/// The room kept above the curve's highest point, in dB (experiment X11 sets it).
pub const MARGIN_DB: f64 = 1.0;
/// The most filters a correction holds.
pub const MOST_FILTERS: usize = 31;
/// The largest equalizer file read, in bytes.
pub const FILE_MOST: u64 = 64 * 1024;
/// The name of the engine's filter entry the commands go to (`af-command eq …`).
pub const LABEL: &str = "eq";
/// The first entry of the filter list: fixed frames of 256 samples and no padding, so the glide is smooth and the same
/// for every codec. It is an entry of its own, so swapping `@eq` never frees the samples it holds.
pub const FRAMES: &str = "@frames:lavfi=%22%asetnsamples=n=256:p=0";

/// The most lines an equalizer file has.
const LINES_MOST: usize = 2000;
/// The most a correction may raise the sound anywhere, in dB.
const BOOST_MOST: f64 = 24.0;
/// The slope Equalizer APO gives a shelf that names neither a Q nor a slope.
const SHELF_SLOPE: f64 = 0.9;
/// The rates the room is worked out at; the largest need is taken, so the level is the same for every file.
const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];
/// A filter at or above this share of a file's rate is left out for that file.
const NYQUIST_SHARE: f64 = 0.49;
/// How fast the preamp glides, in linear gain per second (1024/rate for each frame of 256 samples).
const GLIDE_PER_SECOND: f64 = 4.0;
/// The format of the book as kept.
const BOOK_VERSION: u32 = 1;

const TOO_BIG: &str = "That file is too big to be an equalizer file";
const GRAPHIC: &str = "This file is a graphic EQ (GraphicEQ). Stash reads the parametric kind: on autoeq.app, download it for Equalizer APO as ParametricEQ.";
const NOTHING: &str = "This file has no filters Stash can use (Preamp and Filter lines, as Equalizer APO writes them).";

/// The kind of filter: a peak (a bell around its frequency), or a shelf that raises or lowers everything below (low)
/// or above (high) it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Peak,
    LowShelf,
    HighShelf,
}

/// How wide a filter is: by its Q, in octaves, or (a shelf) by its slope, where 1 is the steepest without a bump.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Width {
    Q(f64),
    Octaves(f64),
    Slope(f64),
}

/// One filter: its kind, its frequency in Hz, its width, and how many dB it raises (below 0, lowers) the sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub kind: Kind,
    pub hz: f64,
    pub width: Width,
    pub db: f64,
}

impl Band {
    /// A filter Stash plays exactly: 10 Hz to 24 kHz, at most 30 dB either way, and a Q of 0.05 to 30, 0.05 to 8
    /// octaves, or a slope of 0.05 to 2. Anything else is refused in the listener's words.
    pub fn new(kind: Kind, hz: f64, width: Width, db: f64) -> Result<Band> {
        let (Width::Q(w) | Width::Octaves(w) | Width::Slope(w)) = width;
        if !(hz.is_finite() && db.is_finite() && w.is_finite()) {
            return refused("a number in this filter cannot be used.");
        }
        let range = "(Stash takes 10 Hz to 24 kHz)";
        if hz > 24_000.0 {
            return refused(format!(
                "{} is above what music holds {range}.",
                hz_words(hz)
            ));
        }
        if hz < 10.0 {
            return refused(format!(
                "{} is below what music holds {range}.",
                hz_words(hz)
            ));
        }
        if db.abs() > 30.0 {
            return refused(format!(
                "a gain of {} dB is beyond what Stash takes (Stash takes −30 dB to +30 dB).",
                words_of(db)
            ));
        }
        match width {
            Width::Q(q) if !(0.05..=30.0).contains(&q) => refused(format!(
                "a Q of {} is outside what Stash takes (Stash takes Q 0.05 to 30).",
                words_of(q)
            )),
            Width::Octaves(o) if !(0.05..=8.0).contains(&o) => refused(format!(
                "a bandwidth of {} octaves is outside what Stash takes (Stash takes 0.05 to 8 octaves).",
                words_of(o)
            )),
            Width::Slope(s) if !(0.05..=2.0).contains(&s) => refused(format!(
                "a slope of {} dB is outside what Stash takes (Stash takes 0.6 dB to 24 dB).",
                words_of(s * 12.0)
            )),
            Width::Slope(s) if slope_root(s, db) < 0.0 => refused(format!(
                "a slope of {} dB is too steep for a gain of {} dB.",
                words_of(s * 12.0),
                words_of(db)
            )),
            _ => Ok(Band {
                kind,
                hz,
                width,
                db,
            }),
        }
    }

    /// How many dB this filter raises the sound at `at_hz` in a file of `rate` Hz: the RBJ cookbook's filters, whose
    /// formulas FFmpeg's af_biquads.c (which plays them) and Equalizer APO's BiQuad.cpp (which the files are written
    /// for) share.
    pub fn response_db(&self, at_hz: f64, rate: f64) -> f64 {
        self.biquad(rate).db_at(at_hz, rate)
    }

    /// The filter's coefficients at `rate`, worked out as FFmpeg works them out.
    fn biquad(&self, rate: f64) -> Biquad {
        let amp = 10f64.powf(self.db / 40.0);
        let w0 = 2.0 * PI * self.hz / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = match self.width {
            Width::Q(q) => sin / (2.0 * q),
            Width::Octaves(o) => sin * (LN_2 / 2.0 * o * w0 / sin).sinh(),
            Width::Slope(s) => sin / 2.0 * slope_root(s, self.db).sqrt(),
        };
        let root = 2.0 * amp.sqrt() * alpha;
        let (up, down) = (amp + 1.0, amp - 1.0);
        match self.kind {
            Kind::Peak => Biquad {
                num: [1.0 + alpha * amp, -2.0 * cos, 1.0 - alpha * amp],
                den: [1.0 + alpha / amp, -2.0 * cos, 1.0 - alpha / amp],
            },
            Kind::LowShelf => Biquad {
                num: [
                    amp * (up - down * cos + root),
                    2.0 * amp * (down - up * cos),
                    amp * (up - down * cos - root),
                ],
                den: [
                    up + down * cos + root,
                    -2.0 * (down + up * cos),
                    up + down * cos - root,
                ],
            },
            Kind::HighShelf => Biquad {
                num: [
                    amp * (up + down * cos + root),
                    -2.0 * amp * (down + up * cos),
                    amp * (up + down * cos - root),
                ],
                den: [
                    up - down * cos + root,
                    2.0 * (down - up * cos),
                    up - down * cos - root,
                ],
            },
        }
    }
}

/// What the cookbook takes the square root of for a shelf of slope `s` and gain `db`: below 0, no shelf has that slope
/// at that gain (the steepest, 2, holds up to about 22.9 dB).
fn slope_root(s: f64, db: f64) -> f64 {
    let amp = 10f64.powf(db / 40.0);
    (amp + 1.0 / amp) * (1.0 / s - 1.0) + 2.0
}

/// A filter's coefficients: its numerator's and its denominator's, as FFmpeg keeps them (not divided by the first).
struct Biquad {
    num: [f64; 3],
    den: [f64; 3],
}

impl Biquad {
    /// The filter's gain in dB at `hz` in a file of `rate` Hz.
    fn db_at(&self, hz: f64, rate: f64) -> f64 {
        let (sin, cos) = (2.0 * PI * hz / rate).sin_cos();
        // |c0 + c1·e^-jw + c2·e^-2jw| is |(c0 + c2)·cos w + c1 + j·(c0 − c2)·sin w| (the same, times e^jw).
        let power =
            |c: &[f64; 3]| ((c[0] + c[2]) * cos + c[1]).powi(2) + ((c[0] - c[2]) * sin).powi(2);
        10.0 * (power(&self.num) / power(&self.den)).log10()
    }
}

/// A correction brought in from a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Correction {
    /// The name the window shows ("Sennheiser HD 650"), from the file's name.
    pub name: String,
    /// The file's own name ("Sennheiser HD 650 ParametricEQ.txt").
    pub file: String,
    /// The file's text, kept and read again at every start, so a fix to the reader reaches kept corrections.
    pub source: String,
    /// The file's Preamp lines added up, in dB.
    pub preamp_db: f64,
    /// Its filters, in file order.
    pub bands: Vec<Band>,
    /// Filters the file itself switched off (OFF), left out.
    pub switched_off: usize,
    /// Lines Equalizer APO passes over too: notes, comments, empty lines, other programs' headers.
    pub passed_over: usize,
}

/// A tone choice: Less, As is or More.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tilt {
    Less,
    #[default]
    AsIs,
    More,
}

impl Tilt {
    /// "more bass", "less treble"; nothing for As is.
    fn words(self, what: &str) -> Option<String> {
        match self {
            Tilt::Less => Some(format!("less {what}")),
            Tilt::AsIs => None,
            Tilt::More => Some(format!("more {what}")),
        }
    }
}

/// One output's equalizer: the switch, Bass and Treble, and the correction.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Eq {
    pub on: bool,
    pub bass: Tilt,
    pub treble: Tilt,
    pub correction: Option<Correction>,
}

impl Eq {
    /// True when the equalizer changes nothing: it is off, or there is no correction and Bass and Treble are As is.
    pub fn is_flat(&self) -> bool {
        !self.on
            || (self.correction.is_none() && self.bass == Tilt::AsIs && self.treble == Tilt::AsIs)
    }

    /// The highest point of the whole curve, in dB (see `peak_of`); 0 when flat.
    pub fn peak_db(&self) -> f64 {
        if self.is_flat() {
            return 0.0;
        }
        let bands: Vec<Band> = self.slots().into_iter().map(|s| s.band).collect();
        peak_of(&bands)
    }

    /// How many dB softer Stash plays to leave room for the boosts: the peak and the margin, or the file's own preamp
    /// when that asks for more. 0 when flat or when nothing is raised; a positive preamp is not applied.
    pub fn room_db(&self) -> f64 {
        if self.is_flat() {
            return 0.0;
        }
        let peak = self.peak_db();
        let for_boosts = if peak > 0.05 { peak + MARGIN_DB } else { 0.0 };
        let preamp = self.correction.as_ref().map_or(0.0, |c| -c.preamp_db);
        for_boosts.max(preamp).max(0.0)
    }

    /// The equalizer in a few words, for the badge: "Sennheiser HD 650, more bass". None when flat.
    pub fn describe(&self) -> Option<String> {
        if self.is_flat() {
            return None;
        }
        let words: Vec<String> = self
            .correction
            .iter()
            .map(|c| c.name.clone())
            .chain(self.bass.words("bass"))
            .chain(self.treble.words("treble"))
            .collect();
        Some(words.join(", "))
    }

    /// Every filter the equalizer plays, named: the correction's in file order ("c0", "c1", …), then "bass" and
    /// "treble" (at 0 dB when As is).
    fn slots(&self) -> Vec<Slot> {
        let correction = self.correction.iter().flat_map(|c| &c.bands);
        let mut slots: Vec<Slot> = correction
            .enumerate()
            .map(|(i, band)| Slot {
                id: format!("c{i}"),
                band: *band,
            })
            .collect();
        slots.push(Slot {
            id: "bass".to_string(),
            band: tone(Kind::LowShelf, BASS, self.bass),
        });
        slots.push(Slot {
            id: "treble".to_string(),
            band: tone(Kind::HighShelf, TREBLE, self.treble),
        });
        slots
    }
}

/// Bass or Treble as a shelf: its frequency and Q, and its dB for the listener's choice.
fn tone(kind: Kind, (hz, q, more): (f64, f64, f64), tilt: Tilt) -> Band {
    let db = match tilt {
        Tilt::Less => -more,
        Tilt::AsIs => 0.0,
        Tilt::More => more,
    };
    Band {
        kind,
        hz,
        width: Width::Q(q),
        db,
    }
}

/// The highest point, in dB, of all these filters together, at each of `RATES`, the largest taken: at 0 Hz (where a
/// low shelf has its whole gain), on a grid of 1/48 octave from 20 Hz up to 20 kHz, and at each filter's own
/// frequency. A filter a rate cannot hold is left out there, as it is when played.
fn peak_of(bands: &[Band]) -> f64 {
    RATES
        .iter()
        .map(|&rate| peak_at(bands, rate))
        .fold(f64::NEG_INFINITY, f64::max)
}

fn peak_at(bands: &[Band], rate: f64) -> f64 {
    let held: Vec<&Band> = bands.iter().filter(|b| holds(rate, b.hz)).collect();
    let filters: Vec<Biquad> = held.iter().map(|b| b.biquad(rate)).collect();
    let top = 20_000f64.min(NYQUIST_SHARE * rate);
    let steps = (48.0 * (top / 20.0).log2()).floor() as i32;
    let grid = (0..=steps).map(|k| 20.0 * 2f64.powf(f64::from(k) / 48.0));
    std::iter::once(0.0)
        .chain(grid)
        .chain([top])
        .chain(held.iter().map(|b| b.hz))
        .map(|hz| filters.iter().map(|f| f.db_at(hz, rate)).sum::<f64>())
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Whether a file of `rate` Hz holds a filter at `hz`: it is below 0.49 of the rate. (FFmpeg passes over only a filter
/// above half the rate, and one exactly there silences the music.)
fn holds(rate: f64, hz: f64) -> bool {
    hz < NYQUIST_SHARE * rate
}

/// Reads an Equalizer APO file (AutoEq's ParametricEQ.txt is one) by APO's own rules. What Stash cannot play exactly
/// is refused in the listener's words, naming the line ("Line 7 has a high-pass filter …"); nothing is taken in part.
pub fn read_apo(bytes: &[u8], file_name: &str) -> Result<Correction> {
    if bytes.len() as u64 > FILE_MOST {
        return refused(TOO_BIG);
    }
    read_text(decode(bytes), file_name)
}

/// Reads a file's text, once decoded. A kept correction is read again from here: its file's size was checked when it
/// was brought in, and the same text can take more bytes as kept (a "€" of the Windows code page takes three), so a
/// correction that came in is never refused as too big later.
fn read_text(source: String, file_name: &str) -> Result<Correction> {
    let text = source.replace('\r', "");
    if text.lines().count() > LINES_MOST {
        return refused(TOO_BIG);
    }
    let mut reading = Reading::default();
    for (i, line) in text.lines().enumerate() {
        reading.line(i + 1, line)?;
    }
    reading.correction(file_name, source)
}

/// "Read 10 filters and a preamp of −6.4 dB from Sennheiser HD 650 ParametricEQ.txt."
pub fn summary(c: &Correction) -> String {
    let filters = match c.bands.len() {
        1 => "1 filter".to_string(),
        n => format!("{n} filters"),
    };
    let preamp = words_of(c.preamp_db);
    if preamp == "0" {
        format!("Read {filters} from {}.", c.file)
    } else {
        format!(
            "Read {filters} and a preamp of {preamp} dB from {}.",
            c.file
        )
    }
}

/// The output the engine opened, from its line "Selecting device '{id}' (Headphones (USB Audio))" (logged by
/// `ao/wasapi`): its name ("wasapi/{id}", as `audio-device-list` names it) and its name for the listener. None for any
/// other line, "Selecting device by id: …" among them.
pub fn opened_output(prefix: &str, text: &str) -> Option<(String, String)> {
    if prefix != "ao/wasapi" {
        return None;
    }
    let rest = text.trim_end().strip_prefix("Selecting device '")?;
    let (id, name) = rest.split_once("' (")?;
    let name = name.strip_suffix(')')?;
    (!id.is_empty()).then(|| (format!("wasapi/{id}"), name.to_string()))
}

/// One filter of the engine's chain, under its name there: "c0" to "c30" for the correction's, "bass", "treble".
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub id: String,
    pub band: Band,
}

impl Slot {
    /// The filter's name in the graph and in commands: "equalizer@c0", "lowshelf@bass".
    fn target(&self) -> String {
        let filter = match self.band.kind {
            Kind::Peak => "equalizer",
            Kind::LowShelf => "lowshelf",
            Kind::HighShelf => "highshelf",
        };
        format!("{filter}@{}", self.id)
    }

    /// The filter as the graph writes it, in 64-bit floats.
    fn filter(&self) -> String {
        let (t, w) = match self.band.width {
            Width::Q(q) => ("q", q),
            Width::Octaves(o) => ("o", o),
            Width::Slope(s) => ("s", s),
        };
        format!(
            "{}=f={}:t={t}:w={}:g={}:r=f64",
            self.target(),
            fixed(self.band.hz, 3),
            fixed(w, 4),
            fixed(self.band.db, 3)
        )
    }
}

/// What the engine plays for an equalizer in a file of a given rate: the preamp's gain (1 is untouched) and the
/// filters, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub gain: f64,
    pub slots: Vec<Slot>,
}

impl Chain {
    /// The chain for `eq` in a file of `rate` Hz; None when flat. A filter at or above 0.49 of the rate is left out
    /// (an unknown rate leaves none out); Bass and Treble are always there, at 0 dB when As is, unless the rate is too
    /// low to hold them.
    pub fn plan(eq: &Eq, rate: Option<u32>) -> Option<Chain> {
        if eq.is_flat() {
            return None;
        }
        let slots = eq
            .slots()
            .into_iter()
            .filter(|s| rate.is_none_or(|rate| holds(f64::from(rate), s.band.hz)))
            .collect();
        Some(Chain {
            gain: 10f64.powf(-eq.room_db() / 20.0),
            slots,
        })
    }

    /// The same filters at 0 dB and the preamp at 1: the music passes through as it came in.
    pub fn flat(&self) -> Chain {
        let slots = self
            .slots
            .iter()
            .map(|s| Slot {
                id: s.id.clone(),
                band: Band { db: 0.0, ..s.band },
            })
            .collect();
        Chain { gain: 1.0, slots }
    }

    /// The same filters (names, kinds, frequencies and widths), whatever their gains and the preamp's: one can become
    /// the other by commands alone.
    pub fn same_shape(&self, other: &Chain) -> bool {
        self.slots.len() == other.slots.len()
            && self.slots.iter().zip(&other.slots).all(|(a, b)| {
                a.id == b.id
                    && a.band.kind == b.band.kind
                    && a.band.hz == b.band.hz
                    && a.band.width == b.band.width
            })
    }

    /// The `@eq` entry: the gliding preamp, the filters, and float out, in mpv's length quoting ("%<bytes>%").
    pub fn entry(&self) -> String {
        let mut graph = format!(
            "volume@pre=volume='{}':eval=frame:precision=double",
            glide(self.gain)
        );
        for slot in &self.slots {
            graph.push(',');
            graph.push_str(&slot.filter());
        }
        graph.push_str(",aformat@out=sample_fmts=fltp");
        format!("@eq:lavfi=%{}%{graph}", graph.len())
    }

    /// The whole filter list: the frames, then the entry.
    pub fn af(&self) -> String {
        format!("{FRAMES},{}", self.entry())
    }
}

/// The preamp's expression for a gain `gain`. Built from a string, the chain starts at `gain` at once (`volume` is
/// not a number at the first frame); sent as a command, the gain moves from where it is toward `gain` by 1024/rate a
/// frame of 256 samples (4.0 a second at every rate) and ends exactly on it, so at 1 the music passes untouched.
pub fn glide(gain: f64) -> String {
    let g = fixed(gain, 9);
    format!(
        "if(isnan(volume)+lte(abs({g}-volume),1024/sample_rate),{g},volume+1024/sample_rate*sgn({g}-volume))"
    )
}

/// A command to one filter of the `@eq` entry: `["af-command", LABEL, command, argument, target]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cmd {
    pub command: &'static str,
    pub argument: String,
    pub target: String,
}

/// One step of a change, for the player to carry out in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Write the filter list (`["af", "set", …]`).
    Write(String),
    /// Send these commands.
    Send(Vec<Cmd>),
    /// Wait this many seconds of glide before the next step; the player adds the time the queued audio takes.
    Wait(f64),
}

/// The steps from the chain playing to another, so that no moment between is louder than the room allows:
/// - from none, the new one is written at flat (which passes the music through as it came in), then glided to;
/// - to none, the chain glides to flat and is then taken out, leaving the frames;
/// - to the same shape, by commands alone (see `steps`), sending only what changes;
/// - to another shape, through flat: down to it, the new one written at flat, then up to it.
pub fn transition(from: Option<&Chain>, to: Option<&Chain>) -> Vec<Step> {
    match (from, to) {
        (None, None) => Vec::new(),
        (None, Some(b)) => [vec![Step::Write(b.flat().af())], steps(&b.flat(), b)].concat(),
        (Some(a), None) => [to_flat(a), vec![Step::Write(FRAMES.to_string())]].concat(),
        (Some(a), Some(b)) if a.same_shape(b) => steps(a, b),
        (Some(a), Some(b)) => [
            to_flat(a),
            vec![Step::Write(b.flat().af())],
            steps(&b.flat(), b),
        ]
        .concat(),
    }
}

/// From `a` down to flat, then a wait until that is heard, so the entry passes the music unchanged when it goes.
fn to_flat(a: &Chain) -> Vec<Step> {
    let mut out = steps(a, &a.flat());
    if !out.is_empty() {
        out.push(Step::Wait(glide_seconds(a.gain, 1.0)));
    }
    out
}

/// From `a` to `b` of the same shape: a preamp that falls goes first and is waited for; then the filters, those whose
/// gain falls before those whose gain rises; then, after a wait, a preamp that rises. A filter's dB only grows with
/// its gain and the dBs add, so every moment between stays at or below the higher of the two curves while the preamp
/// is at the lower of the two gains.
fn steps(a: &Chain, b: &Chain) -> Vec<Step> {
    let moved = fixed(a.gain, 9) != fixed(b.gain, 9);
    let mut out = Vec::new();
    if moved && b.gain < a.gain {
        out.push(Step::Send(vec![preamp_to(b.gain)]));
        out.push(Step::Wait(glide_seconds(a.gain, b.gain)));
    }
    let filters = filter_changes(a, b);
    let sent = !filters.is_empty();
    if sent {
        out.push(Step::Send(filters));
    }
    if moved && b.gain > a.gain {
        if sent {
            out.push(Step::Wait(0.0));
        }
        out.push(Step::Send(vec![preamp_to(b.gain)]));
    }
    out
}

/// The commands for the filters whose gain changes from `a` to `b`: those that fall, then those that rise.
fn filter_changes(a: &Chain, b: &Chain) -> Vec<Cmd> {
    let changed: Vec<(&Slot, &Slot)> = a
        .slots
        .iter()
        .zip(&b.slots)
        .filter(|(x, y)| fixed(x.band.db, 3) != fixed(y.band.db, 3))
        .collect();
    let falling = changed.iter().filter(|(x, y)| y.band.db < x.band.db);
    let rising = changed.iter().filter(|(x, y)| y.band.db > x.band.db);
    falling.chain(rising).map(|(_, y)| gain_to(y)).collect()
}

fn preamp_to(gain: f64) -> Cmd {
    Cmd {
        command: "volume",
        argument: glide(gain),
        target: "volume@pre".to_string(),
    }
}

fn gain_to(slot: &Slot) -> Cmd {
    Cmd {
        command: "g",
        argument: fixed(slot.band.db, 3),
        target: slot.target(),
    }
}

/// How long the preamp takes to glide from one gain to another, in seconds.
fn glide_seconds(from: f64, to: f64) -> f64 {
    (to - from).abs() / GLIDE_PER_SECOND
}

/// The equalizer of each output, under the engine's own name for it ("wasapi/{endpoint id}", or "auto" where the
/// engine names none), and the output opened last: the guess at start, until the engine says which it opened.
#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub v: u32,
    pub last: Option<String>,
    pub outputs: BTreeMap<String, Kept>,
}

impl Default for Book {
    fn default() -> Book {
        Book {
            v: BOOK_VERSION,
            last: None,
            outputs: BTreeMap::new(),
        }
    }
}

/// One output's equalizer, with the output's name for the listener and, when it had to be turned off, why.
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub description: String,
    pub eq: Eq,
    /// Why this equalizer was turned off on its own; not kept across starts.
    pub trouble: Option<String>,
}

/// The book as kept: `{"v":1,"last":…,"outputs":{name:{…}}}`.
#[derive(Serialize, Deserialize)]
struct Stored<T> {
    #[serde(default)]
    v: u32,
    last: Option<String>,
    outputs: BTreeMap<String, T>,
}

/// One output as kept: the correction by its name, its file's name and its whole text, never its filters.
#[derive(Serialize, Deserialize)]
struct StoredOutput {
    #[serde(default)]
    description: String,
    #[serde(default)]
    on: bool,
    #[serde(default)]
    bass: Tilt,
    #[serde(default)]
    treble: Tilt,
    #[serde(default)]
    correction: Option<StoredCorrection>,
}

#[derive(Serialize, Deserialize)]
struct StoredCorrection {
    name: String,
    file: String,
    source: String,
}

impl Book {
    /// The book as kept. It never fails: a text that cannot be read is an empty book, an output that cannot be read is
    /// left out, and a correction whose text no longer reads turns that output off, its `trouble` saying why.
    pub fn from_json(text: &str) -> Book {
        let Ok(stored) = serde_json::from_str::<Stored<serde_json::Value>>(text) else {
            return Book::default();
        };
        let outputs = stored
            .outputs
            .into_iter()
            .filter_map(|(name, value)| {
                let output = serde_json::from_value(value).ok()?;
                Some((name, Kept::from_stored(output)))
            })
            .collect();
        Book {
            v: BOOK_VERSION,
            last: stored.last,
            outputs,
        }
    }

    /// The book to keep.
    pub fn to_json(&self) -> String {
        let outputs: BTreeMap<String, StoredOutput> = self
            .outputs
            .iter()
            .map(|(name, kept)| (name.clone(), kept.stored()))
            .collect();
        let stored = Stored {
            v: BOOK_VERSION,
            last: self.last.clone(),
            outputs,
        };
        serde_json::to_string(&stored).expect("a book is plain data")
    }

    /// The equalizer of `output`; off when it has none.
    pub fn get(&self, output: &str) -> Eq {
        self.outputs
            .get(output)
            .map(|kept| kept.eq.clone())
            .unwrap_or_default()
    }

    /// Keeps `eq` for `output` (`description` is its name for the listener). An equalizer that is off with nothing in
    /// it removes the entry, so the book stays small.
    pub fn put(&mut self, output: &str, description: &str, eq: Eq) {
        if eq == Eq::default() {
            self.outputs.remove(output);
            return;
        }
        let kept = Kept {
            description: description.to_string(),
            eq,
            trouble: None,
        };
        self.outputs.insert(output.to_string(), kept);
    }
}

impl Kept {
    fn from_stored(stored: StoredOutput) -> Kept {
        let mut eq = Eq {
            on: stored.on,
            bass: stored.bass,
            treble: stored.treble,
            correction: None,
        };
        let mut trouble = None;
        if let Some(kept) = stored.correction {
            match read_text(kept.source, &kept.file) {
                Ok(correction) => {
                    eq.correction = Some(Correction {
                        name: kept.name,
                        ..correction
                    })
                }
                Err(why) => {
                    eq.on = false;
                    let why = why.to_string();
                    trouble = Some(format!(
                        "Your correction “{}” could not be read again: {}.",
                        kept.name,
                        why.trim_end_matches('.')
                    ));
                }
            }
        }
        Kept {
            description: stored.description,
            eq,
            trouble,
        }
    }

    fn stored(&self) -> StoredOutput {
        StoredOutput {
            description: self.description.clone(),
            on: self.eq.on,
            bass: self.eq.bass,
            treble: self.eq.treble,
            correction: self.eq.correction.as_ref().map(|c| StoredCorrection {
                name: c.name.clone(),
                file: c.file.clone(),
                source: c.source.clone(),
            }),
        }
    }
}

/// What a file has said so far.
#[derive(Default)]
struct Reading {
    preamp_db: f64,
    bands: Vec<Band>,
    switched_off: usize,
    passed_over: usize,
}

impl Reading {
    /// Reads line `n` (counted from 1), split at its first ":" into a key and a value: a preamp or a filter is taken,
    /// a line Equalizer APO passes over is counted, and a line that changes what the file means is refused. Keys and
    /// filter types are told apart by case, as in APO. A line without ":", an empty one among them, is passed over.
    fn line(&mut self, n: usize, line: &str) -> Result<()> {
        let Some((key, value)) = line.split_once(':') else {
            self.passed_over += 1;
            return Ok(());
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            _ if key.starts_with('#') => self.passed_over += 1,
            // Several Preamp lines add up, as in APO 0.8 and later.
            "Preamp" => match preamp(value) {
                Some(db) => self.preamp_db += db,
                None => self.passed_over += 1,
            },
            "GraphicEQ" => return refused(GRAPHIC),
            // "Filter", "Filter 1", "Filter1": the number is not read.
            _ if key.starts_with("Filter") => match filter(n, value)? {
                FilterLine::Band(band) => self.bands.push(band),
                FilterLine::Off => self.switched_off += 1,
                FilterLine::Over => self.passed_over += 1,
            },
            "Channel" if value == "all" => {}
            _ => match command(key) {
                Some((does, why)) => {
                    let said = if value.is_empty() {
                        key.to_string()
                    } else {
                        format!("{key}: {value}")
                    };
                    return refused(format!(
                        "Line {n} {does} ({said}). {why}, so the file was not brought in."
                    ));
                }
                // Another program's line, such as Room EQ Wizard's "Dated:" and "Equaliser:".
                None => self.passed_over += 1,
            },
        }
        Ok(())
    }

    /// The correction the whole file makes, or why there is none.
    fn correction(self, file_name: &str, source: String) -> Result<Correction> {
        let count = self.bands.len();
        if count > MOST_FILTERS {
            return refused(format!(
                "This file has {count} filters; Stash takes up to {MOST_FILTERS}."
            ));
        }
        if count == 0 {
            // A preamp alone changes only the level, which is the volume's job.
            return refused(NOTHING);
        }
        let boost = peak_of(&self.bands);
        if boost > BOOST_MOST + 1e-9 {
            // Rounded up to a tenth, so it never reads as the limit itself.
            let shown = ((boost - 1e-9) * 10.0).ceil() / 10.0;
            return refused(format!(
                "This file boosts up to {} dB; Stash takes up to {}.",
                words_of(shown),
                words_of(BOOST_MOST)
            ));
        }
        Ok(Correction {
            name: name_of(file_name),
            file: file_name.to_string(),
            source,
            preamp_db: self.preamp_db,
            bands: self.bands,
            switched_off: self.switched_off,
            passed_over: self.passed_over,
        })
    }
}

/// What one Filter line comes to.
enum FilterLine {
    Band(Band),
    /// Switched off in the file (OFF).
    Off,
    /// Passed over, as APO passes it over: type "None", or no "ON <type>" at its start.
    Over,
}

/// A Filter line's value ("ON PK Fc 105 Hz Gain 6.5 dB Q 0.70" and its kin), commas read as decimal points.
fn filter(n: usize, value: &str) -> Result<FilterLine> {
    let text = value.replace(',', ".");
    let words: Vec<&str> = text.split_whitespace().collect();
    let (Some(&"ON"), Some(&kind)) = (words.first(), words.get(1)) else {
        return Ok(if words.first() == Some(&"OFF") {
            FilterLine::Off
        } else {
            FilterLine::Over
        });
    };
    let rest = &words[2..];
    match kind {
        "PK" | "PEQ" | "Modal" => peak(n, rest).map(FilterLine::Band),
        "LSC" | "HSC" | "LS" | "HS" => shelf(n, kind, rest).map(FilterLine::Band),
        _ => match unplayable(kind) {
            Some(what) => refused(format!(
                "Line {n} has {what} ({kind}). Stash's equalizer reads peaking (PK) and shelf (LS, HS, LSC, HSC) filters."
            )),
            None => Ok(FilterLine::Over),
        },
    }
}

/// A filter type Stash does not play, in words.
fn unplayable(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "HP" | "HPQ" => "a high-pass filter",
        "LP" | "LPQ" => "a low-pass filter",
        "BP" => "a band-pass filter",
        "NO" => "a notch filter",
        "AP" => "an all-pass filter",
        "IIR" => "a filter of its own coefficients",
        _ => return None,
    })
}

/// What a command Stash does not follow does, and why Stash does not follow it.
fn command(key: &str) -> Option<(&'static str, &'static str)> {
    const ONLY: &str = "Stash reads only the preamp and the filters";
    Some(match key {
        "Channel" => ("picks channels", "Stash puts one correction on both sides"),
        "Device" => (
            "picks a device",
            "Stash keeps a correction for each output itself",
        ),
        "Include" => ("brings in another file", ONLY),
        "Convolution" => ("uses a convolution", ONLY),
        "Delay" => ("delays the sound", ONLY),
        "Copy" => ("mixes channels", ONLY),
        "Eval" => ("works out values of its own", ONLY),
        "If" | "ElseIf" | "Else" | "EndIf" => ("has a condition", ONLY),
        "Stage" => ("picks a stage", ONLY),
        "LoudnessCorrection" => ("corrects loudness", ONLY),
        "VSTPlugin" => ("uses a plug-in", ONLY),
        _ => return None,
    })
}

/// A peak (PK, PEQ, Modal): a frequency, a gain, and a Q or a bandwidth in octaves.
fn peak(n: usize, words: &[&str]) -> Result<Band> {
    let said = Said::of(words);
    let (hz, db) = said.frequency_and_gain(n)?;
    let width = match (said.q, said.octaves) {
        (Some(q), _) => Width::Q(q),
        (None, Some(o)) => Width::Octaves(o),
        (None, None) => {
            return refused(format!("Line {n}: no Q, slope or bandwidth for this peak."));
        }
    };
    on_line(n, Band::new(Kind::Peak, hz, width, db))
}

/// A shelf (LSC, HSC, LS, HS): a frequency, a gain, and a slope ("<n> dB" right after the type, S = n/12), a Q, or
/// neither (APO's slope 0.9, at the frequency given). LS and HS with a slope or a Q name the corner: the shelf's
/// centre is 10^(|G|/(80·S)) above it for LS and below it for HS, S from the Q as 1/((1/Q² − 2)/(A + 1/A) + 1).
fn shelf(n: usize, name: &str, words: &[&str]) -> Result<Band> {
    let said = Said::of(words);
    let (hz, db) = said.frequency_and_gain(n)?;
    let kind = if name.starts_with('L') {
        Kind::LowShelf
    } else {
        Kind::HighShelf
    };
    let slope = with_unit(words, "dB").and_then(number).map(|x| x / 12.0);
    let (width, s) = match (slope, said.q) {
        (Some(s), _) => (Width::Slope(s), s),
        (None, Some(q)) => (Width::Q(q), slope_of_q(q, db)),
        (None, None) => return on_line(n, Band::new(kind, hz, Width::Slope(SHELF_SLOPE), db)),
    };
    let band = on_line(n, Band::new(kind, hz, width, db))?;
    if name == "LSC" || name == "HSC" {
        return Ok(band);
    }
    if !(s.is_finite() && s > 0.0) {
        return refused(format!("Line {n}: this shelf's slope cannot be used."));
    }
    let shift = 10f64.powf(db.abs() / (80.0 * s));
    let centre = if kind == Kind::LowShelf {
        hz * shift
    } else {
        hz / shift
    };
    on_line(n, Band::new(kind, centre, width, db))
}

/// A shelf's slope S from its Q and gain (the cookbook's 1/Q = √((A + 1/A)(1/S − 1) + 2), turned round).
fn slope_of_q(q: f64, db: f64) -> f64 {
    let amp = 10f64.powf(db / 40.0);
    1.0 / ((1.0 / (q * q) - 2.0) / (amp + 1.0 / amp) + 1.0)
}

/// The values a Filter line gives after its type: "Fc <n> Hz", "Gain <n> dB", "Q <n>", "BW Oct <n>". A word that is
/// none of these is passed over.
#[derive(Default)]
struct Said {
    hz: Option<f64>,
    db: Option<f64>,
    q: Option<f64>,
    octaves: Option<f64>,
}

impl Said {
    fn of(words: &[&str]) -> Said {
        let mut said = Said::default();
        for (i, word) in words.iter().enumerate() {
            let after = &words[i + 1..];
            match *word {
                "Fc" => said.hz = with_unit(after, "Hz").and_then(frequency).or(said.hz),
                "Gain" => said.db = with_unit(after, "dB").and_then(number).or(said.db),
                "Q" => said.q = after.first().and_then(|w| number(w)).or(said.q),
                "BW" if after.first() == Some(&"Oct") => {
                    said.octaves = after.get(1).and_then(|w| number(w)).or(said.octaves)
                }
                _ => {}
            }
        }
        said
    }

    /// The frequency and the gain every filter needs, or the line's refusal.
    fn frequency_and_gain(&self, n: usize) -> Result<(f64, f64)> {
        let Some(hz) = self.hz else {
            return refused(format!("Line {n}: no frequency (Fc … Hz)."));
        };
        let Some(db) = self.db else {
            return refused(format!("Line {n}: no gain (Gain … dB)."));
        };
        Ok((hz, db))
    }
}

/// A Preamp line's value: "<n> dB", commas read as decimal points.
fn preamp(value: &str) -> Option<f64> {
    let text = value.replace(',', ".");
    let words: Vec<&str> = text.split_whitespace().collect();
    with_unit(&words, "dB").and_then(number)
}

/// The number at the start of `words` that `unit` follows, as a word of its own or joined to it ("105 Hz", "105Hz").
fn with_unit<'a>(words: &[&'a str], unit: &str) -> Option<&'a str> {
    let first = *words.first()?;
    match first.strip_suffix(unit) {
        Some(number) if !number.is_empty() => Some(number),
        _ => (words.get(1).copied() == Some(unit)).then_some(first),
    }
}

/// A frequency by APO's thousands rule: a value of 5 or more characters, without "e", whose 4th character from the
/// end is "." is read in thousands, so "1.000" is 1000 Hz (and "105.000" is 105 kHz).
fn frequency(text: &str) -> Option<f64> {
    let hz = number(text)?;
    let chars: Vec<char> = text.chars().collect();
    let thousands = chars.len() >= 5 && !text.contains('e') && chars[chars.len() - 4] == '.';
    Some(if thousands { hz * 1000.0 } else { hz })
}

/// A plain decimal number ("-3.5", "1e3"); not "inf" or "NaN", and never one too large to hold.
fn number(text: &str) -> Option<f64> {
    let plain = text.bytes().any(|b| b.is_ascii_digit())
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || b"+-.eE".contains(&b));
    if !plain {
        return None;
    }
    text.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// A refusal of a filter, said of line `n`.
fn on_line(n: usize, band: Result<Band>) -> Result<Band> {
    band.map_err(|why| Error::Refused(format!("Line {n}: {why}")))
}

/// The correction's name from its file's: without ".txt" or a trailing "ParametricEQ" (AutoEq's ending, in any case,
/// as one word or two, after a space or not), trimmed, at most 60 characters; "Correction" when nothing is left.
fn name_of(file_name: &str) -> String {
    let mut name = file_name.trim();
    if let Some(stem) = strip_suffix_ignore_case(name, ".txt") {
        name = stem;
    }
    for ending in [" parametric eq", "parametriceq"] {
        if let Some(stem) = strip_suffix_ignore_case(name, ending) {
            name = stem;
            break;
        }
    }
    let name: String = name.trim().chars().take(60).collect();
    match name.trim_end() {
        "" => "Correction".to_string(),
        name => name.to_string(),
    }
}

fn strip_suffix_ignore_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let at = text.len().checked_sub(suffix.len())?;
    (text.is_char_boundary(at) && text[at..].eq_ignore_ascii_case(suffix)).then(|| &text[..at])
}

/// A file's text: UTF-16 after its byte-order mark, else UTF-8 (a mark stripped), else the Windows code page APO falls
/// back to.
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|pair| unit(*pair));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// A byte of the Windows code page 1252 as a character (its 0x80–0x9F differ from Latin-1).
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// `x` with this many decimals, as the engine reads numbers: never an exponent, always a point, no minus on a zero.
fn fixed(x: f64, decimals: usize) -> String {
    let text = format!("{x:.decimals$}");
    match text.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_string(),
        _ => text,
    }
}

/// A number as the listener reads it: at most three decimals, no trailing zeros, and a true minus sign ("−6.4").
fn words_of(x: f64) -> String {
    let text = fixed(x, 3);
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text.strip_prefix('-') {
        Some(rest) => format!("−{rest}"),
        None => text.to_string(),
    }
}

/// A frequency as the listener reads it: "25 kHz", "5 Hz".
fn hz_words(hz: f64) -> String {
    if hz.abs() >= 1000.0 {
        format!("{} kHz", words_of(hz / 1000.0))
    } else {
        format!("{} Hz", words_of(hz))
    }
}

fn refused<T>(words: impl Into<String>) -> Result<T> {
    Err(Error::Refused(words.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(hz: f64, db: f64, q: f64) -> Band {
        Band::new(Kind::Peak, hz, Width::Q(q), db).unwrap()
    }

    fn made_up(bands: Vec<Band>, preamp_db: f64) -> Correction {
        Correction {
            name: "Test Phones".into(),
            file: "Test Phones ParametricEQ.txt".into(),
            source: String::new(),
            preamp_db,
            bands,
            switched_off: 0,
            passed_over: 0,
        }
    }

    fn on(correction: Option<Correction>, bass: Tilt, treble: Tilt) -> Eq {
        Eq {
            on: true,
            bass,
            treble,
            correction,
        }
    }

    fn refusal<T: std::fmt::Debug>(result: Result<T>) -> String {
        match result {
            Err(Error::Refused(words)) => words,
            other => panic!("not refused: {other:?}"),
        }
    }

    /// A complex number, for the port below.
    #[derive(Clone, Copy)]
    struct Complex {
        re: f64,
        im: f64,
    }

    impl Complex {
        fn plus(self, o: Complex) -> Complex {
            Complex {
                re: self.re + o.re,
                im: self.im + o.im,
            }
        }
        fn times(self, o: Complex) -> Complex {
            Complex {
                re: self.re * o.re - self.im * o.im,
                im: self.re * o.im + self.im * o.re,
            }
        }
        fn scaled(self, k: f64) -> Complex {
            Complex {
                re: self.re * k,
                im: self.im * k,
            }
        }
        fn over(self, o: Complex) -> Complex {
            let d = o.re * o.re + o.im * o.im;
            Complex {
                re: (self.re * o.re + self.im * o.im) / d,
                im: (self.im * o.re - self.re * o.im) / d,
            }
        }
    }

    /// Equalizer APO's BiQuad.cpp, ported on its own: the RBJ cookbook's coefficients divided by a0 (as APO keeps
    /// them), then |H(e^jw)| by complex arithmetic, z^-2 as z^-1 squared.
    fn apo_db(kind: Kind, db: f64, hz: f64, width: Width, rate: f64, at_hz: f64) -> f64 {
        let a = 10f64.powf(db / 40.0);
        let omega = 2.0 * PI * hz / rate;
        let (sn, cs) = (omega.sin(), omega.cos());
        let alpha = match width {
            Width::Q(q) => sn / (2.0 * q),
            Width::Slope(s) => sn / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt(),
            Width::Octaves(bw) => sn * (LN_2 / 2.0 * bw * omega / sn).sinh(),
        };
        let beta = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Peak => (
                1.0 + alpha * a,
                -2.0 * cs,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cs,
                1.0 - alpha / a,
            ),
            Kind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cs + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                a * ((a + 1.0) - (a - 1.0) * cs - beta),
                (a + 1.0) + (a - 1.0) * cs + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                (a + 1.0) + (a - 1.0) * cs - beta,
            ),
            Kind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cs + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                a * ((a + 1.0) + (a - 1.0) * cs - beta),
                (a + 1.0) - (a - 1.0) * cs + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * cs),
                (a + 1.0) - (a - 1.0) * cs - beta,
            ),
        };
        let w = 2.0 * PI * at_hz / rate;
        let one = Complex { re: 1.0, im: 0.0 };
        let z1 = Complex {
            re: w.cos(),
            im: -w.sin(),
        };
        let z2 = z1.times(z1);
        let num = one
            .scaled(b0 / a0)
            .plus(z1.scaled(b1 / a0))
            .plus(z2.scaled(b2 / a0));
        let den = one.plus(z1.scaled(a1 / a0)).plus(z2.scaled(a2 / a0));
        let h = num.over(den);
        20.0 * h.re.hypot(h.im).log10()
    }

    #[test]
    fn the_formulas_are_equalizer_apo_s_own() {
        let text = "Preamp: -7 dB\n\
            Filter 1: ON PK Fc 1000 Hz Gain 5 dB Q 1.41\n\
            Filter 2: ON PK Fc 3000 Hz Gain -4 dB BW Oct 1.5\n\
            Filter 3: ON LSC Fc 400 Hz Gain 6 dB Q 0.7\n\
            Filter 4: ON HSC 9 dB Fc 5000 Hz Gain -3 dB\n\
            Filter 5: ON LS Fc 300 Hz Gain 5 dB Q 0.9\n\
            Filter 6: ON HS 12 dB Fc 8000 Hz Gain 4 dB\n\
            Filter 7: ON LSC 6 dB Fc 320 Hz Gain 4 dB\n\
            Filter 8: ON HSC Fc 7000 Hz Gain -2 dB Q 0.8\n";
        let read = read_apo(text.as_bytes(), "Port Test.txt").unwrap();
        // What APO makes of each line. "9 dB" is S = 9/12 = 0.75, "12 dB" is S = 1 and "6 dB" is S = 0.5. LS and HS
        // name the corner: the centre is 10^(|G|/(80·S)) above (LS) or below (HS) it, S from the Q as
        // 1/((1/Q² − 2)/(A + 1/A) + 1). LSC and HSC name the centre itself, with a slope or with a Q.
        let a5 = 10f64.powf(5.0 / 40.0);
        let s5 = 1.0 / ((1.0 / (0.9 * 0.9) - 2.0) / (a5 + 1.0 / a5) + 1.0);
        let apo = [
            (Kind::Peak, 5.0, 1000.0, Width::Q(1.41)),
            (Kind::Peak, -4.0, 3000.0, Width::Octaves(1.5)),
            (Kind::LowShelf, 6.0, 400.0, Width::Q(0.7)),
            (Kind::HighShelf, -3.0, 5000.0, Width::Slope(0.75)),
            (
                Kind::LowShelf,
                5.0,
                300.0 * 10f64.powf(5.0 / (80.0 * s5)),
                Width::Q(0.9),
            ),
            (
                Kind::HighShelf,
                4.0,
                8000.0 / 10f64.powf(4.0 / 80.0),
                Width::Slope(1.0),
            ),
            (Kind::LowShelf, 4.0, 320.0, Width::Slope(0.5)),
            (Kind::HighShelf, -2.0, 7000.0, Width::Q(0.8)),
        ];
        assert_eq!(read.bands.len(), apo.len());
        for (band, &(kind, db, hz, width)) in read.bands.iter().zip(&apo) {
            assert_eq!((band.kind, band.db, band.width), (kind, db, width));
            assert!((band.hz - hz).abs() < 1e-9 * hz, "{band:?} against {hz}");
            for rate in [44_100.0, 192_000.0] {
                let grid = (0..=120).map(|k| 20.0 * 2f64.powf(f64::from(k) / 12.0));
                for at in grid.chain([hz]) {
                    let ours = band.response_db(at, rate);
                    let theirs = apo_db(kind, db, hz, width, rate, at);
                    assert!(
                        (ours - theirs).abs() < 1e-9,
                        "{band:?} at {at} Hz in {rate} Hz: {ours} against {theirs}"
                    );
                }
            }
        }
        // A peak reads its gain at its own frequency and nothing at 0 Hz; a shelf reaches its whole gain at 0 Hz
        // (low) or at half the rate (high): the cookbook's H(1) and H(−1) are exactly A².
        let peak = read.bands[0];
        assert!((peak.response_db(1000.0, 44_100.0) - 5.0).abs() < 1e-9);
        assert!(peak.response_db(0.0, 44_100.0).abs() < 1e-9);
        assert!((read.bands[2].response_db(0.0, 44_100.0) - 6.0).abs() < 1e-9);
        assert!((read.bands[3].response_db(22_050.0, 44_100.0) + 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_file_is_read_by_equalizer_apo_s_rules() {
        // Shaped like an AutoEq file, with made-up numbers, its filters not in frequency order.
        let autoeq = "Preamp: -6.2 dB\n\
            Filter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.70\n\
            Filter 2: ON PK Fc 2400 Hz Gain -3.1 dB Q 1.20\n\
            Filter 3: ON PK Fc 160 Hz Gain -2.4 dB Q 0.55\n\
            Filter 4: ON HSC Fc 10000 Hz Gain -4.0 dB Q 0.70\n\
            Filter 5: ON PK Fc 5800 Hz Gain 4.2 dB Q 3.10\n\
            Filter 6: ON PK Fc 950 Hz Gain 1.3 dB Q 1.90\n\
            Filter 7: ON PK Fc 3500 Hz Gain -2.0 dB Q 4.00\n\
            Filter 8: ON PK Fc 60 Hz Gain -0.8 dB Q 2.20\n\
            Filter 9: ON PK Fc 7600 Hz Gain 2.5 dB Q 5.50\n\
            Filter 10: ON PK Fc 13000 Hz Gain -1.7 dB Q 1.10\n";
        let want = [
            (Kind::LowShelf, 105.0, 0.7, 5.5),
            (Kind::Peak, 2400.0, 1.2, -3.1),
            (Kind::Peak, 160.0, 0.55, -2.4),
            (Kind::HighShelf, 10_000.0, 0.7, -4.0),
            (Kind::Peak, 5800.0, 3.1, 4.2),
            (Kind::Peak, 950.0, 1.9, 1.3),
            (Kind::Peak, 3500.0, 4.0, -2.0),
            (Kind::Peak, 60.0, 2.2, -0.8),
            (Kind::Peak, 7600.0, 5.5, 2.5),
            (Kind::Peak, 13_000.0, 1.1, -1.7),
        ];
        for text in [autoeq.to_string(), autoeq.replace('\n', "\r\n")] {
            let c = read_apo(text.as_bytes(), "Test Phones ParametricEQ.txt").unwrap();
            assert_eq!(
                (c.name.as_str(), c.file.as_str(), c.source.as_str()),
                ("Test Phones", "Test Phones ParametricEQ.txt", text.as_str())
            );
            assert_eq!((c.preamp_db, c.switched_off, c.passed_over), (-6.2, 0, 0));
            let got: Vec<_> = c
                .bands
                .iter()
                .map(|b| (b.kind, b.hz, b.width, b.db))
                .collect();
            let want: Vec<_> = want
                .iter()
                .map(|&(kind, hz, q, db)| (kind, hz, Width::Q(q), db))
                .collect();
            assert_eq!(got, want);
            assert_eq!(
                summary(&c),
                "Read 10 filters and a preamp of −6.2 dB from Test Phones ParametricEQ.txt."
            );
        }

        // Room EQ Wizard's header, notes and comments are passed over and counted (the empty line too), as are
        // "None" and keys APO does not know; OFF is counted apart; the Preamp lines add up (−3 − 2.5).
        let rew = "Dated: Nov 16, 2026 10:00:00\n\
            Equaliser: Generic\n\
            Notes: made up for this test\n\
            # a comment: with a colon\n\
            a line without a colon\n\
            \n\
            Preamp: -3 dB\n\
            Preamp: -2,5 dB\n\
            Filter 1: ON PK Fc 1.000 Hz Gain -3,5 dB Q 1\n\
            Filter 2: OFF PK Fc 500 Hz Gain 2 dB Q 1\n\
            Filter 3: ON None\n\
            Filter 4: ON LSC 12 dB Fc 200 Hz Gain 3 dB\n\
            Filter 5: ON LS Fc 100 Hz Gain 6 dB Q 1\n\
            Filter 6: ON LS Fc 80 Hz Gain 2 dB\n\
            Something: else\n";
        let c = read_apo(rew.as_bytes(), "Room Test.txt").unwrap();
        assert_eq!(c.name, "Room Test");
        assert_eq!((c.preamp_db, c.switched_off, c.passed_over), (-5.5, 1, 8));
        assert_eq!(c.bands.len(), 4);
        // "Fc 1.000 Hz" is 1000 Hz by APO's thousands rule; "Gain -3,5 dB" is −3.5.
        assert_eq!(c.bands[0], pk(1000.0, -3.5, 1.0));
        // "LSC 12 dB" is a slope of S = 12/12 = 1 at the frequency given.
        assert_eq!(
            c.bands[1],
            Band {
                kind: Kind::LowShelf,
                hz: 200.0,
                width: Width::Slope(1.0),
                db: 3.0
            }
        );
        // LS with Q 1 at +6 dB: A = 10^(6/40) = 1.41254, A + 1/A = 2.12048, S = 1/((1 − 2)/2.12048 + 1) = 1.89247,
        // so the centre is 100 × 10^(6/(80 × 1.89247)) = 100 × 10^0.039631 = 109.555 Hz.
        let shifted = c.bands[2];
        let a6 = 10f64.powf(6.0 / 40.0);
        let s = 1.0 / ((1.0 - 2.0) / (a6 + 1.0 / a6) + 1.0);
        assert!((shifted.hz - 100.0 * 10f64.powf(6.0 / (80.0 * s))).abs() < 1e-9);
        assert!((shifted.hz - 109.555).abs() < 0.01, "{}", shifted.hz);
        assert_eq!(
            (shifted.kind, shifted.width, shifted.db),
            (Kind::LowShelf, Width::Q(1.0), 6.0)
        );
        // A bare LS is APO's slope 0.9, at the frequency given.
        assert_eq!(
            c.bands[3],
            Band {
                kind: Kind::LowShelf,
                hz: 80.0,
                width: Width::Slope(0.9),
                db: 2.0
            }
        );
        assert_eq!(
            summary(&c),
            "Read 4 filters and a preamp of −5.5 dB from Room Test.txt."
        );
        let one = read_apo(b"Filter 1: ON PK Fc 1000 Hz Gain -2 dB Q 1\n", "One.txt").unwrap();
        assert_eq!(summary(&one), "Read 1 filter from One.txt.");
        // By the same rule "105.000" is 105 kHz, which no music holds.
        assert_eq!(
            refusal(read_apo(
                b"Filter 1: ON PK Fc 105.000 Hz Gain -3 dB Q 1\n",
                "x.txt"
            )),
            "Line 1: 105 kHz is above what music holds (Stash takes 10 Hz to 24 kHz)."
        );

        // UTF-8 with a mark, UTF-16 both ways round, and the Windows code page.
        let plain = "Preamp: -2 dB\nFilter 1: ON PK Fc 1000 Hz Gain -2 dB Q 1\n";
        let mut marked = b"\xEF\xBB\xBF".to_vec();
        marked.extend_from_slice(plain.as_bytes());
        let (mut le, mut be) = (vec![0xFF, 0xFE], vec![0xFE, 0xFF]);
        for unit in plain.encode_utf16() {
            le.extend(unit.to_le_bytes());
            be.extend(unit.to_be_bytes());
        }
        for bytes in [marked, le, be] {
            let c = read_apo(&bytes, "Test Phones.txt").unwrap();
            assert_eq!(c.source, plain);
            assert_eq!(
                (c.preamp_db, c.bands.clone()),
                (-2.0, vec![pk(1000.0, -2.0, 1.0)])
            );
        }
        // In the Windows code page "ö" is 0xF6 and "–" is 0x96; neither is UTF-8 there.
        let ansi =
            b"Notes: Kopfh\xF6rer \x96 erfunden\nFilter 1: ON PK Fc 1000 Hz Gain -2 dB Q 1\n";
        let c = read_apo(ansi, "Alt.txt").unwrap();
        assert!(
            c.source.starts_with("Notes: Kopfhörer – erfunden\n"),
            "{}",
            c.source
        );
        assert_eq!((c.bands.len(), c.passed_over), (1, 1));

        // The name.
        assert_eq!(name_of("Test Phones ParametricEq.txt"), "Test Phones");
        assert_eq!(name_of("Test Phones Parametric EQ.TXT"), "Test Phones");
        assert_eq!(name_of("Test PhonesParametricEQ.txt"), "Test Phones");
        assert_eq!(name_of("Desk Speakers.txt"), "Desk Speakers");
        assert_eq!(name_of(" ParametricEQ.txt"), "Correction");
        assert_eq!(name_of(&format!("{}.txt", "x".repeat(80))), "x".repeat(60));
    }

    #[test]
    fn every_refusal_names_its_line_in_the_listener_s_words() {
        let read = |text: &str| refusal(read_apo(text.as_bytes(), "Test Phones.txt"));
        let pk_line = "Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 1\n";
        let reads = " Stash's equalizer reads peaking (PK) and shelf (LS, HS, LSC, HSC) filters.";
        assert_eq!(
            read("Preamp: -3 dB\nFilter 1: ON HP Fc 30 Hz\n"),
            format!("Line 2 has a high-pass filter (HP).{reads}")
        );
        for (kind, what) in [
            ("LP", "a low-pass filter"),
            ("LPQ", "a low-pass filter"),
            ("HPQ", "a high-pass filter"),
            ("BP", "a band-pass filter"),
            ("NO", "a notch filter"),
            ("AP", "an all-pass filter"),
            ("IIR", "a filter of its own coefficients"),
        ] {
            assert_eq!(
                read(&format!("{pk_line}Filter 2: ON {kind} Fc 100 Hz Q 1\n")),
                format!("Line 2 has {what} ({kind}).{reads}")
            );
        }
        assert_eq!(
            read(&format!("{pk_line}GraphicEQ: 20 -3; 40 -2\n")),
            "This file is a graphic EQ (GraphicEQ). Stash reads the parametric kind: on autoeq.app, download it for Equalizer APO as ParametricEQ."
        );
        assert_eq!(
            read(&format!("Preamp: -3 dB\n{pk_line}Channel: L\n")),
            "Line 3 picks channels (Channel: L). Stash puts one correction on both sides, so the file was not brought in."
        );
        // "Channel: all" is every channel, as with no Channel line.
        assert!(read_apo(format!("Channel: all\n{pk_line}").as_bytes(), "x.txt").is_ok());
        assert_eq!(
            read("Device: Speakers\n"),
            "Line 1 picks a device (Device: Speakers). Stash keeps a correction for each output itself, so the file was not brought in."
        );
        assert_eq!(
            read(&format!("{pk_line}Include: other.txt\n")),
            "Line 2 brings in another file (Include: other.txt). Stash reads only the preamp and the filters, so the file was not brought in."
        );
        for key in [
            "Convolution",
            "Delay",
            "Copy",
            "Eval",
            "If",
            "ElseIf",
            "Else",
            "EndIf",
            "Stage",
            "LoudnessCorrection",
            "VSTPlugin",
        ] {
            let words = read(&format!("{key}: x\n{pk_line}"));
            assert!(
                words.starts_with("Line 1 ")
                    && words.contains(&format!(" ({key}: x). "))
                    && words.ends_with(", so the file was not brought in."),
                "{words}"
            );
        }
        // A value missing, counted from line 1.
        assert_eq!(
            read("Filter 1: ON PK Fc 1000 Hz Q 1\n"),
            "Line 1: no gain (Gain … dB)."
        );
        assert_eq!(
            read("Preamp: -1 dB\n\n# notes\nFilter 1: ON PK Gain -3 dB Q 1\n"),
            "Line 4: no frequency (Fc … Hz)."
        );
        assert_eq!(
            read("Filter 1: ON PK Fc 1000 Hz Gain -3 dB\n"),
            "Line 1: no Q, slope or bandwidth for this peak."
        );
        // Out of range.
        assert_eq!(
            read(&format!(
                "Preamp: -1 dB\n{pk_line}{pk_line}{pk_line}Filter 5: ON PK Fc 25000 Hz Gain -3 dB Q 1\n"
            )),
            "Line 5: 25 kHz is above what music holds (Stash takes 10 Hz to 24 kHz)."
        );
        assert_eq!(
            read("Filter 1: ON PK Fc 5 Hz Gain -3 dB Q 1\n"),
            "Line 1: 5 Hz is below what music holds (Stash takes 10 Hz to 24 kHz)."
        );
        assert_eq!(
            read("Filter 1: ON PK Fc 1000 Hz Gain -35 dB Q 1\n"),
            "Line 1: a gain of −35 dB is beyond what Stash takes (Stash takes −30 dB to +30 dB)."
        );
        assert_eq!(
            read("Filter 1: ON PK Fc 1000 Hz Gain -3 dB Q 40\n"),
            "Line 1: a Q of 40 is outside what Stash takes (Stash takes Q 0.05 to 30)."
        );
        assert_eq!(
            read("Filter 1: ON LSC 30 dB Fc 100 Hz Gain 3 dB\n"),
            "Line 1: a slope of 30 dB is outside what Stash takes (Stash takes 0.6 dB to 24 dB)."
        );
        // S = 24/12 = 2 at +30 dB: A + 1/A = 10^0.75 + 10^−0.75 = 5.80, and 5.80 × (1/2 − 1) + 2 < 0, so the
        // cookbook's square root has no value there.
        assert_eq!(
            read("Filter 1: ON LSC 24 dB Fc 100 Hz Gain 30 dB\n"),
            "Line 1: a slope of 24 dB is too steep for a gain of 30 dB."
        );
        // Too many filters, too much boost (a +28 dB peak is exactly 28 dB at its own frequency), nothing to play.
        let many: String = (1..=34)
            .map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain -1 dB Q 1\n", 100 * i))
            .collect();
        assert_eq!(
            read(&many),
            "This file has 34 filters; Stash takes up to 31."
        );
        assert_eq!(
            read("Filter 1: ON PK Fc 1000 Hz Gain 28 dB Q 1\n"),
            "This file boosts up to 28 dB; Stash takes up to 24."
        );
        let nothing = "This file has no filters Stash can use (Preamp and Filter lines, as Equalizer APO writes them).";
        assert_eq!(read("Preamp: -6 dB\n"), nothing);
        assert_eq!(
            read("Filter 1: OFF PK Fc 1000 Hz Gain -3 dB Q 1\n"),
            nothing
        );
        assert_eq!(read(""), nothing);
        // At most 64 KiB and 2,000 lines.
        let too_big = "That file is too big to be an equalizer file";
        let mut big = format!("{pk_line}#").into_bytes();
        big.resize(FILE_MOST as usize, b'x');
        assert!(read_apo(&big, "x.txt").is_ok());
        big.push(b'x');
        assert_eq!(refusal(read_apo(&big, "x.txt")), too_big);
        let lines = format!("{pk_line}{}", "#\n".repeat(1999));
        assert!(read_apo(lines.as_bytes(), "x.txt").is_ok());
        assert_eq!(read(&format!("{lines}#\n")), too_big);
    }

    #[test]
    fn flat_means_nothing_and_the_room_follows_the_peak() {
        let six = made_up(vec![pk(1000.0, 6.0, 1.41)], 0.0);
        let off = Eq {
            correction: Some(six.clone()),
            bass: Tilt::More,
            ..Eq::default()
        };
        assert!(off.is_flat());
        assert_eq!(
            (off.peak_db(), off.room_db(), off.describe()),
            (0.0, 0.0, None)
        );
        assert_eq!(Chain::plan(&off, Some(44_100)), None);
        assert!(on(None, Tilt::AsIs, Tilt::AsIs).is_flat());
        // A peak's dB at its own frequency is its gain exactly (|H(e^jw0)| = A² in the cookbook), and that
        // frequency is among the points: +6 peaks at 6, and the room adds the margin of 1.
        let one = on(Some(six.clone()), Tilt::AsIs, Tilt::AsIs);
        assert!((one.peak_db() - 6.0).abs() < 0.01 && (one.room_db() - 7.0).abs() < 0.01);
        // dBs add: two alike peaks are 12 at their frequency.
        let two = made_up(vec![pk(1000.0, 6.0, 1.41), pk(1000.0, 6.0, 1.41)], 0.0);
        assert!((on(Some(two), Tilt::AsIs, Tilt::AsIs).peak_db() - 12.0).abs() < 0.01);
        // More bass is a +4 dB low shelf, whose whole gain is reached at 0 Hz.
        let bass = on(None, Tilt::More, Tilt::AsIs);
        assert!((bass.peak_db() - 4.0).abs() < 0.01 && (bass.room_db() - 5.0).abs() < 0.01);
        // Cuts only leave nothing to make room for.
        let cuts = made_up(vec![pk(1000.0, -6.0, 1.41), pk(3000.0, -2.0, 2.0)], 0.0);
        assert_eq!(on(Some(cuts), Tilt::Less, Tilt::Less).room_db(), 0.0);
        // A file's preamp is the room when it asks for more: −10 against a peak of 6.2 (and the margin, 7.2) is 10.
        let preamp = on(
            Some(made_up(vec![pk(1000.0, 6.2, 1.41)], -10.0)),
            Tilt::AsIs,
            Tilt::AsIs,
        );
        assert!((preamp.peak_db() - 6.2).abs() < 0.01 && (preamp.room_db() - 10.0).abs() < 1e-9);
        // A positive preamp is not applied: +3 leaves +6's room at 7, and a cut's at 0.
        let louder = on(
            Some(made_up(vec![pk(1000.0, 6.0, 1.41)], 3.0)),
            Tilt::AsIs,
            Tilt::AsIs,
        );
        assert!((louder.room_db() - 7.0).abs() < 0.01);
        let cut = on(
            Some(made_up(vec![pk(1000.0, -6.0, 1.41)], 3.0)),
            Tilt::AsIs,
            Tilt::AsIs,
        );
        assert_eq!(cut.room_db(), 0.0);
        // In words.
        assert_eq!(one.describe().as_deref(), Some("Test Phones"));
        let more = on(Some(six), Tilt::More, Tilt::AsIs);
        assert_eq!(more.describe().as_deref(), Some("Test Phones, more bass"));
        let tones = on(None, Tilt::Less, Tilt::More);
        assert_eq!(tones.describe().as_deref(), Some("less bass, more treble"));
    }

    #[test]
    fn a_plan_names_its_filters_in_file_order_and_leaves_out_what_a_rate_cannot_hold() {
        let c = made_up(
            vec![
                pk(3000.0, -3.0, 1.0),
                pk(100.0, 2.0, 1.0),
                pk(16_000.0, -2.0, 1.0),
                pk(20_000.0, -1.0, 1.0),
            ],
            -4.0,
        );
        let eq = on(Some(c.clone()), Tilt::More, Tilt::AsIs);
        let ids = |rate| -> Vec<String> {
            let chain = Chain::plan(&eq, rate).unwrap();
            chain.slots.iter().map(|s| s.id.clone()).collect()
        };
        let all = ["c0", "c1", "c2", "c3", "bass", "treble"];
        assert_eq!(ids(None), all);
        // 0.49 × 44.1 kHz is 21,609 Hz, so 20 kHz is kept; 0.49 × 32 kHz is 15,680 Hz, so 16 kHz (and 20) are not.
        assert_eq!(ids(Some(44_100)), all);
        assert_eq!(ids(Some(32_000)), ["c0", "c1", "bass", "treble"]);
        let chain = Chain::plan(&eq, None).unwrap();
        assert_eq!(chain.slots[1].band, c.bands[1]);
        assert_eq!(
            (chain.slots[4].band.db, chain.slots[5].band.db),
            (BASS.2, 0.0)
        );
        assert_eq!(chain.gain, 10f64.powf(-eq.room_db() / 20.0));
        // The shape is the filters, whatever their gains and the preamp.
        let other = Correction {
            preamp_db: -9.0,
            bands: c.bands.iter().map(|b| Band { db: -b.db, ..*b }).collect(),
            ..c.clone()
        };
        let other = Chain::plan(&on(Some(other), Tilt::Less, Tilt::More), None).unwrap();
        assert!(chain.same_shape(&other) && chain.same_shape(&chain.flat()));
        let mut moved = c.clone();
        moved.bands[0].hz = 3100.0;
        let moved = Chain::plan(&on(Some(moved), Tilt::More, Tilt::AsIs), None).unwrap();
        assert!(!chain.same_shape(&moved));
        // Flat: the same filters at 0 dB, the preamp at 1.
        let flat = chain.flat();
        assert!(flat.gain == 1.0 && flat.slots.iter().all(|s| s.band.db == 0.0));
    }

    /// No number written with an exponent: no digit followed by "e" or "E" and then a digit or a sign.
    fn no_exponent(text: &str) -> bool {
        text.as_bytes().windows(3).all(|w| {
            !(w[0].is_ascii_digit()
                && (w[1] == b'e' || w[1] == b'E')
                && (w[2].is_ascii_digit() || w[2] == b'-' || w[2] == b'+'))
        })
    }

    #[test]
    fn the_strings_mpv_reads_are_exact() {
        // More bass alone: its shelf's whole +4 dB is reached at 0 Hz, so the room is 4 + 1 = 5 dB and the preamp
        // 10^(−5/20) = 0.5623413251…, "0.562341325" to nine places. The graph after "%306%" is 306 bytes: the preamp
        // 19 + 123 (the glide with an 11-character gain) + 29, the shelves 50 and 54, aformat 28, and 3 commas.
        let more_bass = Chain::plan(&on(None, Tilt::More, Tilt::AsIs), Some(44_100)).unwrap();
        assert_eq!(
            more_bass.af(),
            "@frames:lavfi=%22%asetnsamples=n=256:p=0,@eq:lavfi=%306%volume@pre=volume='if(isnan(volume)+lte(abs(0.562341325-volume),1024/sample_rate),0.562341325,volume+1024/sample_rate*sgn(0.562341325-volume))':eval=frame:precision=double,lowshelf@bass=f=105.000:t=q:w=0.7100:g=4.000:r=f64,highshelf@treble=f=6000.000:t=q:w=0.7100:g=0.000:r=f64,aformat@out=sample_fmts=fltp"
        );
        // A cut alone needs no room: the preamp is 1. Its graph is 358 bytes (the peak's 51 and one more comma).
        let cut = made_up(vec![pk(1000.0, -3.0, 1.41)], 0.0);
        let cut = Chain::plan(&on(Some(cut), Tilt::AsIs, Tilt::AsIs), Some(44_100)).unwrap();
        assert_eq!(cut.gain, 1.0);
        assert_eq!(
            cut.af(),
            "@frames:lavfi=%22%asetnsamples=n=256:p=0,@eq:lavfi=%358%volume@pre=volume='if(isnan(volume)+lte(abs(1.000000000-volume),1024/sample_rate),1.000000000,volume+1024/sample_rate*sgn(1.000000000-volume))':eval=frame:precision=double,equalizer@c0=f=1000.000:t=q:w=1.4100:g=-3.000:r=f64,lowshelf@bass=f=105.000:t=q:w=0.7100:g=0.000:r=f64,highshelf@treble=f=6000.000:t=q:w=0.7100:g=0.000:r=f64,aformat@out=sample_fmts=fltp"
        );
        // Far ends of what a filter may be, and every width.
        let far = made_up(
            vec![
                Band::new(Kind::Peak, 10.0, Width::Octaves(0.05), 0.0001).unwrap(),
                Band::new(Kind::LowShelf, 23_999.9, Width::Slope(2.0), -20.0).unwrap(),
                Band::new(Kind::HighShelf, 12_345.678, Width::Q(30.0), 29.9999).unwrap(),
            ],
            -0.0001,
        );
        let far = Chain::plan(&on(Some(far), Tilt::Less, Tilt::More), Some(192_000)).unwrap();
        for chain in [&more_bass, &cut, &far] {
            // %n% is the byte count of what follows it, in each entry.
            for entry in [FRAMES.to_string(), chain.entry()] {
                let (_, rest) = entry.split_once(":lavfi=%").unwrap();
                let (n, graph) = rest.split_once('%').unwrap();
                assert_eq!(n.parse::<usize>().unwrap(), graph.len(), "{entry}");
            }
            let af = chain.af();
            assert!(
                no_exponent(&af) && !af.contains('[') && !af.contains(']'),
                "{af}"
            );
        }
        assert!(no_exponent(&glide(1e-12)) && glide(1e-12).contains("0.000000000-volume"));
        for zero in [-0.0, -1e-7, 1e-7] {
            assert_eq!(fixed(zero, 3), "0.000");
        }
        assert_eq!(fixed(-4.0, 3), "-4.000");
    }

    /// Plays `steps` from `from` on paper. After each command it checks that the curve as it stands, through the
    /// preamp in effect, stays the margin below full scale wherever it raises the sound; a glide's gain is in effect
    /// at the larger of its two ends until a wait as long as the glide. At the end it checks the chain is `to`.
    fn play_on_paper(from: Option<&Chain>, to: Option<&Chain>, steps: &[Step]) {
        let mut slots: Vec<Slot> = from.map(|c| c.slots.clone()).unwrap_or_default();
        let mut gain = from.map_or(1.0, |c| c.gain);
        let mut gliding_from: Option<f64> = None;
        for step in steps {
            match step {
                Step::Write(af) if af == FRAMES => {
                    (slots, gain, gliding_from) = (Vec::new(), 1.0, None)
                }
                Step::Write(af) => {
                    let flat = to
                        .expect("a write of filters writes the chain to come")
                        .flat();
                    assert_eq!(af, &flat.af());
                    (slots, gain, gliding_from) = (flat.slots, 1.0, None);
                }
                Step::Wait(seconds) => {
                    // The gain here is read back from the command's nine decimals, while the wait was worked out
                    // from the gain itself, so the glide on paper can be up to 1.25e-10 s longer than the wait.
                    if gliding_from
                        .is_some_and(|before| *seconds + 1e-9 >= glide_seconds(before, gain))
                    {
                        gliding_from = None;
                    }
                }
                Step::Send(cmds) => {
                    for cmd in cmds {
                        if cmd.target == "volume@pre" {
                            let g: f64 = cmd
                                .argument
                                .strip_prefix("if(isnan(volume)+lte(abs(")
                                .and_then(|rest| rest.split('-').next())
                                .unwrap()
                                .parse()
                                .unwrap();
                            assert_eq!((cmd.command, &cmd.argument), ("volume", &glide(g)));
                            gliding_from =
                                Some(gliding_from.map_or(gain, |before| before.max(gain)));
                            gain = g;
                        } else {
                            let id = cmd.target.split_once('@').unwrap().1;
                            let slot = slots.iter_mut().find(|s| s.id == id).unwrap();
                            assert_eq!((cmd.command, &cmd.target), ("g", &slot.target()));
                            slot.band.db = cmd.argument.parse().unwrap();
                        }
                        let bands: Vec<Band> = slots.iter().map(|s| s.band).collect();
                        let peak = peak_of(&bands);
                        let in_effect = gliding_from.map_or(gain, |before| before.max(gain));
                        if peak > 0.05 {
                            assert!(
                                peak + 20.0 * in_effect.log10() <= -MARGIN_DB + 1e-6,
                                "after {cmd:?}: a peak of {peak} dB through {in_effect}"
                            );
                        }
                    }
                }
            }
        }
        let dbs = |slots: &[Slot]| -> Vec<(String, String)> {
            slots
                .iter()
                .map(|s| (s.id.clone(), fixed(s.band.db, 3)))
                .collect()
        };
        match to {
            None => assert!(slots.is_empty()),
            Some(to) => {
                assert_eq!(fixed(gain, 9), fixed(to.gain, 9));
                assert_eq!(dbs(&slots), dbs(&to.slots));
            }
        }
    }

    #[test]
    fn changes_glide_in_without_outrunning_the_room() {
        let plan = |eq: &Eq| Chain::plan(eq, Some(44_100)).unwrap();
        let check = |from: Option<&Chain>, to: Option<&Chain>| {
            let steps = transition(from, to);
            play_on_paper(from, to, &steps);
            steps
        };
        let pre = |chain: &Chain| Cmd {
            command: "volume",
            argument: glide(chain.gain),
            target: "volume@pre".into(),
        };
        let g = |target: &str, db: &str| Cmd {
            command: "g",
            argument: db.into(),
            target: target.into(),
        };
        let c = made_up(vec![pk(100.0, 6.0, 1.0), pk(3000.0, -3.0, 2.0)], 0.0);
        let b = plan(&on(Some(c.clone()), Tilt::AsIs, Tilt::AsIs));
        assert!(b.gain < 1.0);

        // On: written at flat, then the preamp falls and is waited for, then the filters move, the falling one
        // first; Bass and Treble stay at 0 dB and are not sent.
        assert_eq!(
            check(None, Some(&b)),
            [
                Step::Write(b.flat().af()),
                Step::Send(vec![pre(&b)]),
                Step::Wait((1.0 - b.gain).abs() / 4.0),
                Step::Send(vec![
                    g("equalizer@c1", "-3.000"),
                    g("equalizer@c0", "6.000")
                ]),
            ]
        );
        // Off: the filters go to 0 dB, falling first; the preamp rises after a wait; once that glide is heard the
        // entry goes and the frames stay.
        assert_eq!(
            check(Some(&b), None),
            [
                Step::Send(vec![g("equalizer@c0", "0.000"), g("equalizer@c1", "0.000")]),
                Step::Wait(0.0),
                Step::Send(vec![pre(&b.flat())]),
                Step::Wait((1.0 - b.gain).abs() / 4.0),
                Step::Write(FRAMES.to_string()),
            ]
        );
        // More bass raises the curve near 100 Hz, so the room goes up: the preamp falls first and is waited for, and
        // only Bass is sent.
        let more = plan(&on(Some(c.clone()), Tilt::More, Tilt::AsIs));
        assert!(more.gain < b.gain);
        assert_eq!(
            check(Some(&b), Some(&more)),
            [
                Step::Send(vec![pre(&more)]),
                Step::Wait((b.gain - more.gain).abs() / 4.0),
                Step::Send(vec![g("lowshelf@bass", "4.000")]),
            ]
        );
        // And back: the room goes down, so the preamp rises last, after a wait.
        assert_eq!(
            check(Some(&more), Some(&b)),
            [
                Step::Send(vec![g("lowshelf@bass", "0.000")]),
                Step::Wait(0.0),
                Step::Send(vec![pre(&b)]),
            ]
        );
        // In one send, the gains that fall go before those that rise.
        let up = plan(&on(Some(c.clone()), Tilt::More, Tilt::Less));
        let down = plan(&on(Some(c.clone()), Tilt::Less, Tilt::More));
        let filters: Vec<Vec<Cmd>> = check(Some(&up), Some(&down))
            .into_iter()
            .filter_map(|step| match step {
                Step::Send(cmds) if cmds[0].target != "volume@pre" => Some(cmds),
                _ => None,
            })
            .collect();
        assert_eq!(
            filters,
            [vec![
                g("lowshelf@bass", "-4.000"),
                g("highshelf@treble", "3.000")
            ]]
        );
        // Another correction goes through flat: down to flat and a wait, as for off, then the new one written at
        // flat, then up to it, as for on.
        let other = plan(&on(
            Some(made_up(vec![pk(250.0, 5.0, 1.4)], -2.0)),
            Tilt::More,
            Tilt::AsIs,
        ));
        assert!(!more.same_shape(&other));
        let steps = check(Some(&more), Some(&other));
        let write = steps
            .iter()
            .position(|s| *s == Step::Write(other.flat().af()))
            .unwrap();
        let off = transition(Some(&more), None);
        assert!(matches!(steps[write - 1], Step::Wait(_)));
        assert_eq!(steps[..write], off[..off.len() - 1]);
        assert_eq!(steps[write + 1..], transition(None, Some(&other))[1..]);
        // Nothing to change sends nothing.
        assert!(check(Some(&more), Some(&more)).is_empty());
        assert!(check(None, None).is_empty());
    }

    #[test]
    fn the_glide_and_the_book_are_kept_as_written() {
        assert_eq!(
            glide(0.5),
            "if(isnan(volume)+lte(abs(0.500000000-volume),1024/sample_rate),0.500000000,volume+1024/sample_rate*sgn(0.500000000-volume))"
        );

        let text = "Preamp: -4 dB\nFilter 1: ON PK Fc 1000 Hz Gain 3 dB Q 1\n";
        let correction = read_apo(text.as_bytes(), "Test Phones ParametricEQ.txt").unwrap();
        let phones = Eq {
            on: true,
            bass: Tilt::More,
            treble: Tilt::AsIs,
            correction: Some(correction),
        };
        let mut book = Book {
            last: Some("wasapi/{a}".into()),
            ..Book::default()
        };
        book.put("wasapi/{a}", "Headphones (Test)", phones.clone());
        book.put(
            "auto",
            "Autoselect device",
            Eq {
                treble: Tilt::Less,
                ..Eq::default()
            },
        );
        let json = book.to_json();
        assert_eq!(Book::from_json(&json), book);
        let kept: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            (&kept["v"], &kept["last"]),
            (&serde_json::json!(1), &serde_json::json!("wasapi/{a}"))
        );
        assert_eq!(
            kept["outputs"]["wasapi/{a}"],
            serde_json::json!({
                "description": "Headphones (Test)", "on": true, "bass": "more", "treble": "as-is",
                "correction": {"name": "Test Phones", "file": "Test Phones ParametricEQ.txt", "source": text}
            })
        );
        assert_eq!(kept["outputs"]["auto"]["treble"], "less");
        assert!(kept["outputs"]["auto"]["correction"].is_null());
        assert!(!json.contains("trouble"));

        // An output with nothing kept is off; keeping nothing for one removes it.
        assert_eq!(book.get("wasapi/{gone}"), Eq::default());
        assert_eq!(book.get("wasapi/{a}"), phones);
        book.put("auto", "Autoselect device", Eq::default());
        assert!(!book.outputs.contains_key("auto"));

        // Garbage is an empty book.
        for garbage in [
            "",
            "not json",
            "[1,2]",
            r#"{"v":1,"last":null,"outputs":5}"#,
        ] {
            let empty = Book::from_json(garbage);
            assert!(
                empty.outputs.is_empty() && empty.last.is_none() && empty.v == 1,
                "{garbage}"
            );
        }

        // A kept correction whose text no longer reads: that output is off, its tone choices stay, and it says why.
        let old = r#"{"v":1,"last":null,"outputs":{"wasapi/{b}":{"description":"Speakers (Test)","on":true,"bass":"more","treble":"as-is","correction":{"name":"Old Phones","file":"Old Phones.txt","source":"Filter 1: ON HP Fc 30 Hz\n"}}}}"#;
        let read = Book::from_json(old);
        let kept = &read.outputs["wasapi/{b}"];
        assert_eq!(
            kept.eq,
            Eq {
                bass: Tilt::More,
                ..Eq::default()
            }
        );
        assert_eq!(kept.description, "Speakers (Test)");
        assert_eq!(
            kept.trouble.as_deref(),
            Some(
                "Your correction “Old Phones” could not be read again: Line 1 has a high-pass filter (HP). Stash's equalizer reads peaking (PK) and shelf (LS, HS, LSC, HSC) filters."
            )
        );

        // A file at the size limit in the Windows code page takes more bytes as kept ("€" is one byte there and three
        // in UTF-8); it is read again all the same.
        let mut ansi = b"Filter 1: ON PK Fc 1000 Hz Gain -2 dB Q 1\n#".to_vec();
        ansi.resize(FILE_MOST as usize, 0x80);
        let euros = Eq {
            on: true,
            correction: Some(read_apo(&ansi, "Euro Phones.txt").unwrap()),
            ..Eq::default()
        };
        assert!(euros.correction.as_ref().unwrap().source.len() > FILE_MOST as usize);
        book.put("wasapi/{c}", "Headphones (Test)", euros.clone());
        let again = Book::from_json(&book.to_json());
        assert_eq!(again.outputs["wasapi/{c}"].trouble, None);
        assert_eq!(again.get("wasapi/{c}"), euros);
    }

    #[test]
    fn the_opened_output_is_read_from_mpv_s_line() {
        assert_eq!(
            opened_output(
                "ao/wasapi",
                "Selecting device '{0.0.0.00000000}.{made-up-id}' (Headphones (USB Audio))\n"
            ),
            Some((
                "wasapi/{0.0.0.00000000}.{made-up-id}".to_string(),
                "Headphones (USB Audio)".to_string()
            ))
        );
        for (prefix, text) in [
            (
                "ao/wasapi",
                "Selecting device by id: '{0.0.0.00000000}.{made-up-id}'\n",
            ),
            ("ao/wasapi", "No device specified. Selecting default.\n"),
            ("ao/wasapi", "Selecting device '' (Speakers)\n"),
            ("ao/wasapi", "Selecting device '{x}'\n"),
            ("cplayer", "Selecting device '{x}' (Speakers)\n"),
        ] {
            assert_eq!(opened_output(prefix, text), None, "{text}");
        }
    }
}
