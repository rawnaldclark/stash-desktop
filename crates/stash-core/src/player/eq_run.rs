//! The equalizer on its way through the engine (what it is: [`crate::eq`]). Each output keeps its own; the one in use
//! is the output the engine says it opened, else the one chosen. Off or flat, the engine's filter list is empty, so
//! nothing touches the samples. On, the list is `@frames` (fixed frames, for smooth glides) and `@eq` (a gliding
//! preamp, the correction's filters, Bass, Treble, float out).
//!
//! While a song plays, a change is never a rebuild of the list (that clicks: measured): it glides there with commands
//! to the running filters (`eq::transition`), in an order that never boosts past the room the preamp leaves. The list
//! itself is written only at seams, where the audio queued is dropped anyway or the graph was built again: when the
//! engine opens the next file (its `on_preloaded` hook), just before Stash's own seeks and output changes, when the
//! engine opens another output by itself, and after a seek it made itself (Repeat one's loop).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::Result;
use crate::audio::{Engine, Output};
use crate::eq::{self, Book, Chain, Eq, Step};

use super::Device;

/// How long a change takes to be heard after it is sent, past a glide: the audio already queued for the output. A
/// sound card or the silent output queue about 0.2 s (mpv's `audio-buffer`) plus the device's own.
const LEAD: f64 = 0.35;

/// How long the system's answer to which output it would choose is taken as it is.
const SYSTEM_FOR: Duration = Duration::from_secs(2);

/// What the window is told of the equalizer of the output in use (`eq` in the player's state).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EqState {
    /// The output in use: the one the engine opened, else the one chosen.
    pub output: Device,
    /// "The system's choice" is chosen (`output` is what it opened, or what it would open now).
    pub system_choice: bool,
    pub on: bool,
    pub bass: eq::Tilt,
    pub treble: eq::Tilt,
    pub correction: Option<CorrectionShown>,
    /// How much softer Stash plays it, to leave room for the boosts (0 when off or flat, or when it only cuts).
    pub softer_db: f64,
    /// "Sennheiser HD 650, more bass"; none when off or flat.
    pub describe: Option<String>,
    /// Kept in the state for the window; v1 never reopens the output at a pause.
    pub reopen_at_pause: bool,
    /// Why the equalizer of the output in use was turned off by itself, in the listener's words.
    pub trouble: Option<String>,
}

/// A correction as the window shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CorrectionShown {
    pub name: String,
    pub file: String,
    pub filters: usize,
    pub switched_off: usize,
    pub passed_over: usize,
    /// "Read 10 filters and a preamp of −6.4 dB from Sennheiser HD 650 ParametricEQ.txt."
    pub summary: String,
}

pub(super) struct EqRun {
    pub(super) book: Book,
    /// The output the engine said it opened, until the output is chosen again.
    pub(super) opened: Option<Device>,
    /// What the engine's list holds as written: its graph is built again from this at a seek, a loop or a reload.
    instance: Option<Chain>,
    /// What the running graph holds now: the list as written, and the commands sent since.
    live: Option<Chain>,
    /// Where the steps lead: the plan for the output in use and the file playing.
    target: Option<Chain>,
    steps: VecDeque<Step>,
    /// No step goes before the engine's place in the song (`time-pos`) reaches this: a glide is heard to its end, and
    /// the audio queued before it has gone out, before anything else is sent.
    ready_at: Option<f64>,
    /// The sample rate of the file playing: filters at or above 0.49 of it are left out for it.
    rate: Option<u32>,
    /// The engine could not use the equalizer for the file playing: it plays without it.
    pub(super) failed: bool,
    /// The next file opened plays without the equalizer (its failure may have been the equalizer's: see `hold`).
    hold_next: bool,
    /// Why the equalizer of an output was turned off by itself: the output, and the words (not kept).
    trouble: Option<(String, String)>,
    /// Tests only: a list written as it is at every seam, until a change or a failure.
    pub(super) forced: Option<String>,
    /// How long a change takes to be heard past its glide (see `LEAD`); below 0 for a file output, whose queue
    /// depends on the file's rate.
    lead: f64,
    /// The engine plays to the system's outputs (not to a file, or nowhere).
    device_output: bool,
    /// The output the system would choose now ("wasapi/{id}"), as last asked, and when.
    system: Option<(Option<String>, Instant)>,
    /// The room of the equalizer last shown, so it is not worked out again at every look.
    shown_room: Option<(Eq, f64)>,
}

impl EqRun {
    pub(super) fn new(output: &Output) -> Self {
        Self {
            book: Book::default(),
            opened: None,
            instance: None,
            live: None,
            target: None,
            steps: VecDeque::new(),
            ready_at: None,
            rate: None,
            failed: false,
            hold_next: false,
            trouble: None,
            forced: None,
            // A file output writes as fast as the decoder goes and queues 65536 frames (1.5 s at 44.1 kHz, measured).
            lead: match output {
                Output::PcmFile(_) => -1.0,
                _ => LEAD,
            },
            device_output: matches!(output, Output::Device { .. }),
            system: None,
            shown_room: None,
        }
    }

    /// "The system's choice" is chosen for the system's outputs: which one that is, the system says.
    pub(super) fn asks_system(&self, chosen: &str) -> bool {
        self.device_output && chosen == "auto"
    }

    /// The system's answer is older than `SYSTEM_FOR`, or was never given.
    pub(super) fn system_due(&self) -> bool {
        self.system
            .as_ref()
            .is_none_or(|(_, at)| at.elapsed() >= SYSTEM_FOR)
    }

    /// What the system says it would choose now.
    pub(super) fn system_said(&mut self, output: Option<String>) {
        self.system = Some((output, Instant::now()));
    }

    /// The lead for the file playing.
    fn lead(&self) -> f64 {
        if self.lead >= 0.0 {
            self.lead
        } else {
            65_536.0 / f64::from(self.rate.unwrap_or(44_100)) + 0.25
        }
    }

    /// The name of the output in use: the one opened, else the one chosen; for "the system's choice", the one the
    /// system says it would choose, else the one opened last (until the engine says which it opened now).
    pub(super) fn output_key(&self, chosen: &str) -> String {
        if let Some(opened) = &self.opened {
            return opened.name.clone();
        }
        if chosen == "auto" {
            let system = self.system.as_ref().and_then(|(output, _)| output.clone());
            return system
                .or_else(|| self.book.last.clone())
                .unwrap_or_else(|| "auto".into());
        }
        chosen.to_string()
    }

    /// The equalizer of the output in use.
    pub(super) fn eq(&self, chosen: &str) -> Eq {
        self.book.get(&self.output_key(chosen))
    }

    /// The chain for the output in use and the file playing; none when off or flat.
    fn plan(&self, chosen: &str) -> Option<Chain> {
        Chain::plan(&self.eq(chosen), self.rate)
    }

    /// Writes `chain` as the engine's list: what the steps lead to is in place at once.
    fn write(&mut self, engine: &Engine, chain: Option<Chain>) -> Result<()> {
        engine.set_filters(chain.as_ref().map(Chain::af).as_deref())?;
        self.instance = chain.clone();
        self.live = chain.clone();
        self.target = chain;
        self.steps.clear();
        self.ready_at = None;
        Ok(())
    }

    /// A seam (the engine opens a file, or Stash seeks or opens the output again: the audio queued is dropped): the
    /// list is written as the plan says, unless it holds that already.
    pub(super) fn commit(&mut self, engine: &Engine, chosen: &str) {
        let plan = self.plan(chosen);
        if self.failed {
            return;
        }
        if let Some(list) = self.forced.clone() {
            let _ = engine.set_filters(Some(&list));
            return;
        }
        // An equalizer glided out leaves `@frames` in the list (it holds audio): a seam clears it.
        let lingering = plan.is_none() && !engine.filters().is_empty();
        if plan == self.instance && plan == self.live && self.steps.is_empty() && !lingering {
            // The audio queued was dropped: no glide is still to be heard.
            self.target = plan;
            self.ready_at = None;
            return;
        }
        let _ = self.write(engine, plan);
    }

    /// The engine opens a file (its `on_preloaded` hook): its sample rate, and the list for it.
    pub(super) fn preloaded(&mut self, engine: &Engine, chosen: &str, rate: Option<u32>) {
        self.rate = rate;
        self.failed = std::mem::take(&mut self.hold_next);
        self.ready_at = None;
        self.commit(engine, chosen);
    }

    /// The equalizer of the output in use changed. Nothing playing: the list is written. Playing: it glides there; a
    /// first step that rebuilds the list runs here, so the engine's refusal answers the change, unless a glide sent
    /// before is still to be heard (it would be cut short): then it waits its turn, as every step does.
    pub(super) fn change(&mut self, engine: &Engine, chosen: &str, playing: bool) -> Result<()> {
        self.forced = None;
        if self
            .trouble
            .as_ref()
            .is_some_and(|(o, _)| *o == self.output_key(chosen))
        {
            self.trouble = None;
        }
        let plan = self.plan(chosen);
        if !playing || self.failed {
            self.failed = false;
            return self.write(engine, plan);
        }
        let mut steps: VecDeque<Step> = eq::transition(self.live.as_ref(), plan.as_ref()).into();
        let heard = self.live.is_none()
            || self.ready_at.is_none_or(|at| {
                engine
                    .mpv()
                    .get("time-pos")
                    .and_then(|p| p.parse::<f64>().ok())
                    .is_some_and(|now| now >= at)
            });
        if let Some(Step::Write(list)) = steps.front().filter(|_| heard) {
            let list = list.clone();
            engine.set_filters(Some(list.as_str()).filter(|l| !l.is_empty()))?;
            steps.pop_front();
            self.wrote(&list, plan.as_ref());
        }
        self.target = plan;
        self.steps = steps;
        Ok(())
    }

    /// After a step wrote `list`: what the engine holds now. A transition writes only `@frames` alone (to none) or
    /// `@frames` with the target's chain at flat.
    fn wrote(&mut self, list: &str, target: Option<&Chain>) {
        let chain = if list == eq::FRAMES || list.is_empty() {
            None
        } else {
            target.map(Chain::flat)
        };
        self.instance = chain.clone();
        self.live = chain;
    }

    /// Whether steps wait to be sent: the event loop looks more often then.
    pub(super) fn busy(&self) -> bool {
        !self.steps.is_empty()
    }

    /// Sends the steps that are due, `position` being the engine's place in the song (`time-pos`). A command the
    /// engine refuses (its graph is being built: right after a start or a seek) is sent again on the next look.
    pub(super) fn tick(&mut self, engine: &Engine, position: Option<f64>) {
        let Some(now) = position else { return };
        while let Some(step) = self.steps.front().cloned() {
            if self.ready_at.is_some_and(|at| now < at) {
                return;
            }
            match step {
                Step::Write(list) => {
                    if engine
                        .set_filters(Some(list.as_str()).filter(|l| !l.is_empty()))
                        .is_err()
                    {
                        return;
                    }
                    let target = self.target.clone();
                    self.wrote(&list, target.as_ref());
                }
                Step::Send(cmds) => {
                    for cmd in &cmds {
                        if engine
                            .filter_command(eq::LABEL, cmd.command, &cmd.argument, &cmd.target)
                            .is_err()
                        {
                            return;
                        }
                    }
                    if let Some(live) = self.live.as_mut() {
                        for cmd in &cmds {
                            apply(live, cmd);
                        }
                    }
                }
                Step::Wait(seconds) => self.ready_at = Some(now + seconds + self.lead()),
            }
            self.steps.pop_front();
        }
    }

    /// A file could not be opened with the equalizer: the list is cleared and the next file opened (that one again)
    /// plays without it, the equalizer kept as it is, until it is known whose fault it was.
    pub(super) fn hold(&mut self, engine: &Engine) {
        let _ = engine.set_filters(None);
        self.instance = None;
        self.live = None;
        self.target = None;
        self.steps.clear();
        self.ready_at = None;
        self.forced = None;
        self.hold_next = true;
    }

    /// The engine could not use the equalizer (a list it could not build, or a filter that failed while running): the
    /// list is cleared and the equalizer of the output in use turned off, with `why`.
    pub(super) fn fail(&mut self, engine: &Engine, chosen: &str, description: &str, why: &str) {
        self.failed = true;
        self.forced = None;
        let _ = engine.set_filters(None);
        self.instance = None;
        self.live = None;
        self.target = None;
        self.steps.clear();
        self.ready_at = None;
        let key = self.output_key(chosen);
        let mut off = self.book.get(&key);
        off.on = false;
        self.book.put(&key, description, off);
        self.trouble = Some((key, why.to_string()));
    }

    /// Why the equalizer of `output` is off by itself: it failed in the engine, or its correction could not be read
    /// again at start.
    fn trouble(&self, output: &str) -> Option<String> {
        match &self.trouble {
            Some((o, why)) if o == output => Some(format!(
                "Your equalizer could not be used and was turned off: {why}"
            )),
            _ => self
                .book
                .outputs
                .get(output)
                .and_then(|k| k.trouble.clone()),
        }
    }

    /// The engine says it opened `device`. True when the book changed (a new output opened last), for it to be kept.
    /// The audio queued is gone and the graph was built again from the list as written: another output gets its own
    /// equalizer written at once; the same one opened again (the system changed its format, or lost it and found it)
    /// glides from what was written to the plan, as a change does.
    pub(super) fn opened_output(&mut self, engine: &Engine, chosen: &str, device: Device) -> bool {
        let again = self.opened.as_ref() == Some(&device);
        let mut changed = self.book.last.as_deref() != Some(device.name.as_str());
        // An equalizer set for "the system's choice" before the system said which output that is goes to the one
        // it opened, when that has none of its own.
        if !again
            && self.device_output
            && !self.book.outputs.contains_key(&device.name)
            && let Some(mut kept) = self.book.outputs.remove("auto")
        {
            kept.description = device.description.clone();
            self.book.outputs.insert(device.name.clone(), kept);
            changed = true;
        }
        self.book.last = Some(device.name.clone());
        self.opened = Some(device);
        let plan = self.plan(chosen);
        if again {
            self.live = self.instance.clone();
            self.ready_at = None;
            self.steps = eq::transition(self.instance.as_ref(), plan.as_ref()).into();
            self.target = plan;
            return changed;
        }
        if plan != self.instance || plan != self.live || !self.steps.is_empty() {
            self.failed = false;
            let _ = self.write(engine, plan);
        }
        changed
    }

    /// What the window is told: the output in use is `output`.
    pub(super) fn state(&mut self, output: Device, system_choice: bool) -> EqState {
        let eq = self.book.get(&output.name);
        let flat = eq.is_flat();
        let room = match &self.shown_room {
            Some((shown, room)) if *shown == eq => *room,
            _ => {
                let room = eq.room_db();
                self.shown_room = Some((eq.clone(), room));
                room
            }
        };
        EqState {
            system_choice,
            on: eq.on,
            bass: eq.bass,
            treble: eq.treble,
            correction: eq.correction.as_ref().map(|c| CorrectionShown {
                name: c.name.clone(),
                file: c.file.clone(),
                filters: c.bands.len(),
                switched_off: c.switched_off,
                passed_over: c.passed_over,
                summary: eq::summary(c),
            }),
            softer_db: if flat {
                0.0
            } else {
                (room * 10.0).round() / 10.0
            },
            describe: eq.describe(),
            reopen_at_pause: false,
            trouble: self.trouble(&output.name),
            output,
        }
    }
}

/// `cmd` sent to the running chain: its new value in `chain`.
fn apply(chain: &mut Chain, cmd: &eq::Cmd) {
    match cmd.command {
        "volume" => {
            if let Some(gain) = glide_target(&cmd.argument) {
                chain.gain = gain;
            }
        }
        "g" => {
            let id = cmd.target.split('@').nth(1).unwrap_or_default();
            if let (Some(slot), Ok(db)) = (
                chain.slots.iter_mut().find(|s| s.id == id),
                cmd.argument.parse::<f64>(),
            ) {
                slot.band.db = db;
            }
        }
        _ => {}
    }
}

/// The gain a glide expression (`eq::glide`) ends on.
fn glide_target(expression: &str) -> Option<f64> {
    let after = expression.split("abs(").nth(1)?;
    after.split('-').next()?.parse().ok()
}
