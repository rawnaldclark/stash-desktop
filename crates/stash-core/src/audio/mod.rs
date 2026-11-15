//! The audio engine: what turns a file or a stream into sound. Audio never passes through the window.
//!
//! The engine is mpv ([`mpv::Mpv`]), configured for music: no video, no scripts, no config files, gapless between
//! tracks of one format. Where the sound goes is an [`Output`]: the sound card (shared, or exclusively and
//! unresampled), nowhere (tests), or a file of exactly the samples the card would have received (measurements).

pub mod mpv;

use std::path::{Path, PathBuf};

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
    /// A WAV file holding the output stream sample for sample, as fast as the decoder goes.
    PcmFile(PathBuf),
}

/// The environment variable that names the libmpv file to load.
pub const LIBRARY_ENV: &str = "STASH_MPV_LIBRARY";

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

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.mpv.set("pause", if paused { "yes" } else { "no" })
    }

    /// Jumps to `seconds` from the start of the current track.
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        self.mpv
            .command(&["seek", &format!("{seconds:.3}"), "absolute"])
    }

    pub fn stop(&self) -> Result<()> {
        self.mpv.command(&["stop"])
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
