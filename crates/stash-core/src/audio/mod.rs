//! The audio engine: what turns a file or a stream into sound. Audio never passes through the window.
//!
//! The engine is mpv ([`mpv::Mpv`]), configured for music: no video, no scripts, no config files, gapless between
//! tracks of one format. Where the sound goes is an [`Output`]: the sound card (shared, or exclusively and
//! unresampled), nowhere (tests), or a file of exactly the samples the card would have received (measurements).

pub mod mpv;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use mpv::{EndReason, Event, Mpv};

use crate::Result;

/// Where the engine sends its samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    /// The system's audio device. `exclusive` takes the device for Stash alone and plays each track at its own
    /// sample rate with no mixer in between; other programs are silent meanwhile.
    Device { exclusive: bool },
    /// Nowhere, as fast as the decoder goes.
    Null,
    /// Nowhere, at the speed of a sound card: a player with no sound, for tests of pausing and seeking.
    Silent,
    /// A WAV file holding the output stream sample for sample, as fast as the decoder goes.
    PcmFile(PathBuf),
}

/// The environment variable that names the libmpv file to load.
pub const LIBRARY_ENV: &str = "STASH_MPV_LIBRARY";

/// The output the system would choose now for "the system's choice", by the engine's name for it ("wasapi/{id}"), as
/// the engine finds it when it opens one: Windows' default output for music. None elsewhere, or when it cannot say.
pub fn system_output() -> Option<String> {
    #[cfg(windows)]
    {
        windows_default_output()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_default_output() -> Option<String> {
    use windows::Win32::Media::Audio::{
        IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize,
    };
    // SAFETY: COM is started for this thread for the call alone (and left as it was when it was started already in
    // another mode); the interfaces are released before it is stopped, and the id the system wrote is freed once copied.
    unsafe {
        let started = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let id = (|| {
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = devices.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
            let id = device.GetId().ok()?;
            let text = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as *const _));
            text
        })();
        if started {
            CoUninitialize();
        }
        // The engine names an output by the system's id without its "{0.0.0.00000000}." (measured).
        let id = id?;
        let id = id.strip_prefix("{0.0.0.00000000}.").unwrap_or(&id);
        Some(format!("wasapi/{id}"))
    }
}

/// The libmpv to load: the one [`LIBRARY_ENV`] names, else one beside the running program.
pub fn find_library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(LIBRARY_ENV).map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let name = if cfg!(windows) {
        "libmpv-2.dll"
    } else if cfg!(target_os = "macos") {
        "libmpv.2.dylib"
    } else {
        "libmpv.so.2"
    };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    beside.is_file().then_some(beside)
}

pub struct Engine {
    mpv: Mpv,
    /// The listener asked for the sound card alone (exclusive mode). It is held only while something plays: paused,
    /// the card is given back, so other programs are heard (see [`Engine::set_paused`]).
    exclusive: AtomicBool,
}

impl Engine {
    /// Starts an engine on the libmpv at `library`, idle, sending to `output`. Engine log lines of `log_level`
    /// ("no", "error", "warn", "info", "v", "debug") arrive as [`Event::Log`].
    pub fn start(library: &Path, output: &Output, log_level: &str) -> Result<Self> {
        let mut options: Vec<(&str, String)> = vec![
            // A music player: no video, no cover-art window, no scripts, no config files, nothing run for us.
            ("vid", "no".into()),
            ("audio-display", "no".into()),
            ("config", "no".into()),
            ("load-scripts", "no".into()),
            ("ytdl", "no".into()),
            ("input-default-bindings", "no".into()),
            ("terminal", "no".into()),
            // Stay alive between tracks and after the last one.
            ("idle", "yes".into()),
            // Gapless while the format stays the same; a new format reopens the output instead of resampling.
            ("gapless-audio", "weak".into()),
            // Open the next track before this one ends, so a stream is ready at the join.
            ("prefetch-playlist", "yes".into()),
            ("audio-client-name", "Stash".into()),
        ];
        match output {
            Output::Device { exclusive } => options.push((
                "audio-exclusive",
                if *exclusive { "yes" } else { "no" }.into(),
            )),
            Output::Silent => options.push(("ao", "null".into())),
            Output::Null => {
                options.push(("ao", "null".into()));
                options.push(("ao-null-untimed", "yes".into()));
            }
            Output::PcmFile(path) => {
                options.push(("ao", "pcm".into()));
                options.push(("ao-pcm-file", path.to_string_lossy().into_owned()));
            }
        }
        let borrowed: Vec<(&str, &str)> = options
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        Ok(Self {
            mpv: Mpv::new(library, &borrowed, log_level)?,
            exclusive: AtomicBool::new(matches!(output, Output::Device { exclusive: true })),
        })
    }

    /// Plays `locator` (a path or a URL) now, in place of whatever is playing.
    pub fn play_now(&self, locator: &str) -> Result<()> {
        self.mpv.command(&["loadfile", locator, "replace"])
    }

    /// Queues `locator` after what is playing; it starts by itself (at once, if nothing is playing).
    pub fn play_next(&self, locator: &str) -> Result<()> {
        self.mpv.command(&["loadfile", locator, "append-play"])
    }

    /// Pauses, or plays on. In exclusive mode a pause gives the sound card back (the output is opened again shared,
    /// paused), so other programs are heard while Stash is quiet; playing on takes it again.
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        if paused {
            self.mpv.set("pause", "yes")?;
            self.hold_card(false);
            Ok(())
        } else {
            self.hold_card(true);
            self.mpv.set("pause", "no")
        }
    }

    /// Exclusive mode, asked for or not: held at once when something plays, otherwise when something next plays.
    pub fn set_exclusive(&self, exclusive: bool) -> Result<()> {
        self.exclusive.store(exclusive, Ordering::SeqCst);
        let paused = self.mpv.get("pause").as_deref() == Some("yes");
        let held = exclusive && !paused;
        self.mpv
            .set("audio-exclusive", if held { "yes" } else { "no" })?;
        Ok(())
    }

    /// Whether the listener asked for the sound card alone (it is held only while something plays).
    pub fn exclusive(&self) -> bool {
        self.exclusive.load(Ordering::SeqCst)
    }

    /// In exclusive mode, takes the sound card (`hold`) or gives it back, opening the output again when that changes.
    fn hold_card(&self, hold: bool) {
        if !self.exclusive() {
            return;
        }
        let want = if hold { "yes" } else { "no" };
        if self.mpv.get("audio-exclusive").as_deref() == Some(want) {
            return;
        }
        if self.mpv.set("audio-exclusive", want).is_ok() {
            // With nothing playing there is no output to open again: it opens as asked with the next track.
            let _ = self.mpv.command(&["ao-reload"]);
        }
    }

    /// Jumps to `seconds` from the start of the current track, to the sample (not to the nearest packet).
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        self.mpv
            .command(&["seek", &format!("{seconds:.3}"), "absolute+exact"])
    }

    pub fn stop(&self) -> Result<()> {
        self.mpv.command(&["stop"])
    }

    /// The audio filter list, through the `af` command (`af set`; `af clr ""` for none). With a chain playing, a list
    /// that cannot be built is refused and the old one stays; with nothing playing, nothing is checked.
    pub fn set_filters(&self, list: Option<&str>) -> Result<()> {
        match list {
            Some(list) => self.mpv.command(&["af", "set", list]),
            None => self.mpv.command(&["af", "clr", ""]),
        }
    }

    /// The audio filter list as mpv holds it ("" for none).
    pub fn filters(&self) -> String {
        self.mpv.get("af").unwrap_or_default()
    }

    /// Sends `command` with `argument` to the filter `target` inside the list entry labelled `label` (`af-command`):
    /// a change while it plays, with no rebuild. Fails while the entry's graph is not built yet (right after a start
    /// or a seek).
    pub fn filter_command(
        &self,
        label: &str,
        command: &str,
        argument: &str,
        target: &str,
    ) -> Result<()> {
        self.mpv
            .command(&["af-command", label, command, argument, target])
    }

    /// The next thing the engine reports, waiting up to `timeout_seconds`.
    pub fn next_event(&self, timeout_seconds: f64) -> Event {
        self.mpv.wait_event(timeout_seconds)
    }

    /// The player itself, for properties and commands this type has no word for yet.
    pub fn mpv(&self) -> &Mpv {
        &self.mpv
    }
}
