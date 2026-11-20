//! Smaller songs for a phone on mobile data. A Subsonic app asks `stream` for a format (`format=mp3`, `opus` or `aac`)
//! or a most bit rate (`maxBitRate`, in kbit/s), as the listener set it for Wi‑Fi and for mobile data, and the song is
//! made over by FFmpeg as it plays (Stash's own copy in its tools folder, else the system's). Without FFmpeg, or with
//! too many songs being made over at once, the song goes as it is: it plays, only larger.
//!
//! An MP3 is made at a constant bit rate, so its length is known before it is made: an app that asks for it
//! (`estimateContentLength=true`) is told it, and seeks by asking for a range of bytes, which starts the making at that
//! time. Any format seeks with `timeOffset` (OpenSubsonic's `transcodeOffset`).

use std::io::{Cursor, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Songs made over at once (each is an FFmpeg running).
const MOST_AT_ONCE: usize = 6;
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// What a song is made into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Mp3,
    Opus,
    Aac,
}

impl Target {
    fn of(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "mp3" => Some(Self::Mp3),
            "opus" | "ogg" | "oga" => Some(Self::Opus),
            "aac" | "adts" => Some(Self::Aac),
            _ => None,
        }
    }

    /// The ending a song of the library has when it is already this.
    fn suffix(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
        }
    }

    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Opus => "audio/ogg",
            Self::Aac => "audio/aac",
        }
    }

    /// The bit rate when the app names a format and no bit rate.
    fn usual_kbps(self) -> u32 {
        match self {
            Self::Mp3 => 192,
            Self::Opus => 128,
            Self::Aac => 160,
        }
    }

    /// The bit rate made for `kbps` asked: within what the encoder makes, and for MP3 one of its own (at most that
    /// asked, so a phone's limit holds).
    fn kbps(self, kbps: u32) -> u32 {
        match self {
            Self::Mp3 => {
                const RATES: [u32; 14] = [
                    32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
                ];
                RATES
                    .iter()
                    .rev()
                    .copied()
                    .find(|r| *r <= kbps)
                    .unwrap_or(32)
            }
            Self::Opus => kbps.clamp(16, 256),
            Self::Aac => kbps.clamp(32, 320),
        }
    }
}

/// How a song is made over: into what, at what bit rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) target: Target,
    pub(crate) kbps: u32,
}

impl Plan {
    /// Bytes a second, for an MP3 (whose bit rate is constant).
    fn bytes_a_second(self) -> u64 {
        u64::from(self.kbps) * 125
    }

    /// The length of the whole song made over, when it can be known before it is made (an MP3 of a known duration).
    pub(crate) fn length(self, seconds: u64) -> Option<u64> {
        (self.target == Target::Mp3 && seconds > 0).then(|| seconds * self.bytes_a_second())
    }

    /// Where in the song (in seconds) the byte `at` of the MP3 made over falls.
    pub(crate) fn time_at(self, at: u64) -> f64 {
        at as f64 / self.bytes_a_second() as f64
    }
}

/// How the song asked for is to be made over, if at all: `format` and `max_kbps` (0: no limit) as the app asked,
/// `suffix` and `source_kbps` the song's own (its bit rate is unknown for some songs streamed from add-ons).
pub(crate) fn plan(
    format: Option<&str>,
    max_kbps: u32,
    suffix: &str,
    source_kbps: Option<u64>,
) -> Option<Plan> {
    let format = format.map(str::trim).filter(|f| !f.is_empty());
    if format.is_some_and(|f| f.eq_ignore_ascii_case("raw")) {
        return None;
    }
    // A format Stash does not make (flac, say) is taken as no format: the bit rate asked for still holds.
    let named = format.and_then(Target::of);
    let small_enough = |kbps: u32| source_kbps.is_some_and(|s| s <= u64::from(kbps));
    match named {
        None if max_kbps == 0 => None,
        None => (!small_enough(max_kbps)).then(|| Plan {
            target: Target::Mp3,
            kbps: Target::Mp3.kbps(max_kbps),
        }),
        Some(target) => {
            let kbps = target.kbps(if max_kbps > 0 {
                max_kbps
            } else {
                target.usual_kbps()
            });
            // Already that, and no larger: it goes as it is.
            let already = suffix.eq_ignore_ascii_case(target.suffix())
                && (max_kbps == 0 || small_enough(max_kbps));
            (!already).then_some(Plan { target, kbps })
        }
    }
}

/// Whether one more song may be made over now.
pub(crate) fn room() -> bool {
    RUNNING.load(Ordering::SeqCst) < MOST_AT_ONCE
}

/// What is made over: a file of this computer (which FFmpeg reads itself, so a seek is quick), or bytes from elsewhere.
pub(crate) enum Source {
    File(String),
    Bytes(Box<dyn Read + Send>),
    /// A song at the core's relay on this computer, which FFmpeg asks for the bytes it needs (a seek is quick).
    Relay(String),
}

impl Source {
    /// What FFmpeg reads, and the one kind of address it may open (so nothing a song's bytes name, a playlist inside
    /// a file say, is fetched from anywhere else).
    fn input(&self) -> (&str, &'static str) {
        match self {
            Self::File(path) => (path, "file"),
            Self::Bytes(_) => ("pipe:0", "pipe"),
            Self::Relay(url) => (url, "http,tcp"),
        }
    }
}

/// FFmpeg's arguments for `plan`, from `from` seconds into the song, reading `input` through `protocols` only.
fn args(plan: Plan, from: f64, input: &str, protocols: &str) -> Vec<String> {
    // Warnings too: when a song cannot be made over, they often say why (a source that ended early, say).
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "warning", "-nostats"]
        .map(String::from)
        .to_vec();
    args.extend(["-protocol_whitelist".into(), protocols.into()]);
    if protocols.starts_with("http") {
        args.extend(
            [
                // A relay that stops answering ends the song (in microseconds).
                "-rw_timeout",
                "60000000",
                // A seek is a request of its own. FFmpeg would otherwise send it on the connection it is reading,
                // which the relay (tiny_http, which never says so) closes after each answer: the seek finds it reset.
                "-short_seek_size",
                "1",
                "-reconnect_on_network_error",
                "1",
            ]
            .map(String::from),
        );
    }
    if from > 0.0 {
        args.extend(["-ss".into(), format!("{from:.3}")]);
    }
    args.extend(["-i".into(), input.into()]);
    args.extend(["-map", "0:a:0", "-vn", "-sn", "-dn", "-map_metadata", "-1"].map(String::from));
    let (codec, layout, more, muxer): (&str, &str, &[&str], &str) = match plan.target {
        // No Xing frame and no tags: every frame the same size, so a byte is a time.
        Target::Mp3 => (
            "libmp3lame",
            "aformat=channel_layouts=mono|stereo:sample_rates=44100|48000|32000",
            &["-write_xing", "0", "-id3v2_version", "0"],
            "mp3",
        ),
        Target::Opus => ("libopus", "aformat=channel_layouts=mono|stereo", &[], "ogg"),
        Target::Aac => (
            "aac",
            "aformat=channel_layouts=mono|stereo:sample_rates=44100|48000",
            &[],
            "adts",
        ),
    };
    args.extend([
        "-af".into(),
        layout.into(),
        "-c:a".into(),
        codec.into(),
        "-b:a".into(),
        format!("{}k", plan.kbps),
    ]);
    args.extend(more.iter().map(|a| a.to_string()));
    args.extend(["-f".into(), muxer.into(), "pipe:1".into()]);
    args
}

/// The song made over as it is read. FFmpeg stops when this is dropped (the phone stopped listening).
pub(crate) struct Made {
    child: Child,
    out: ChildStdout,
}

impl Read for Made {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.out.read(buf)
    }
}

impl Drop for Made {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        RUNNING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The most of what FFmpeg says that is kept, for the reason a song could not be made over.
const MOST_SAID: usize = 4096;

/// Starts making `source` over by `plan`, from `from` seconds in, with `ffmpeg`. Its first bytes are waited for, so a
/// song FFmpeg cannot read is told as such rather than sent empty.
pub(crate) fn start(
    ffmpeg: &Path,
    source: Source,
    plan: Plan,
    from: f64,
) -> Result<Box<dyn Read + Send>, String> {
    let (input, protocols) = source.input();
    let mut command = Command::new(ffmpeg);
    command
        .args(args(plan, from, input, protocols))
        .stdin(match source {
            Source::Bytes(_) => Stdio::piped(),
            Source::File(_) | Source::Relay(_) => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("FFmpeg could not start ({e})"))?;
    RUNNING.fetch_add(1, Ordering::SeqCst);
    let said = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = (&mut err).take(MOST_SAID as u64).read_to_end(&mut bytes);
            // What is past the most kept is read and let go, so FFmpeg never waits on it.
            let _ = std::io::copy(&mut err, &mut std::io::sink());
            String::from_utf8_lossy(&bytes).trim().to_string()
        })
    });
    if let (Source::Bytes(mut bytes), Some(mut stdin)) = (source, child.stdin.take()) {
        // Ends when the bytes do, or when FFmpeg stops taking them (it was stopped).
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut bytes, &mut stdin);
        });
    }
    let out = child.stdout.take().expect("stdout is piped");
    let mut made = Made { child, out };
    let mut first = vec![0u8; 16 * 1024];
    let got = made
        .read(&mut first)
        .map_err(|e| format!("FFmpeg could not be read ({e})"))?;
    if got == 0 {
        let _ = made.child.wait();
        let said = said.and_then(|s| s.join().ok()).unwrap_or_default();
        return Err(if said.is_empty() {
            "the song could not be made smaller".into()
        } else {
            // Its last few lines: a warning before the error often says why.
            let lines: Vec<&str> = said.lines().collect();
            format!(
                "the song could not be made smaller (FFmpeg said: {})",
                lines[lines.len().saturating_sub(3)..].join(" / ")
            )
        });
    }
    first.truncate(got);
    Ok(Box::new(Cursor::new(first).chain(made)))
}

/// Exactly `left` bytes of `inner`: cut where it is longer, and filled with nothing where it ends early. An MP3 made
/// over is a few bytes off the length it was said to have, and a player told a length reads that many.
pub(crate) struct Exactly<R> {
    inner: R,
    left: u64,
    ended: bool,
}

impl<R: Read> Exactly<R> {
    pub(crate) fn new(inner: R, length: u64) -> Self {
        Self {
            inner,
            left: length,
            ended: false,
        }
    }
}

impl<R: Read> Read for Exactly<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let most = buf
            .len()
            .min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = if self.ended {
            0
        } else {
            self.inner.read(&mut buf[..most])?
        };
        let n = if n == 0 {
            self.ended = true;
            buf[..most].fill(0);
            most
        } else {
            n
        };
        self.left -= n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_is_made_over_only_when_the_app_asks_for_less_or_another_kind() {
        // Nothing asked, or the song as it is.
        assert_eq!(plan(None, 0, "flac", Some(900)), None);
        assert_eq!(plan(Some("raw"), 128, "flac", Some(900)), None);
        // A most bit rate alone: an MP3 at most that, unless the song is no larger.
        assert_eq!(
            plan(None, 128, "flac", Some(900)),
            Some(Plan {
                target: Target::Mp3,
                kbps: 128
            })
        );
        assert_eq!(plan(None, 320, "mp3", Some(256)), None);
        assert_eq!(
            plan(Some(""), 150, "flac", None),
            Some(Plan {
                target: Target::Mp3,
                kbps: 128
            })
        );
        // A format Stash does not make is no format.
        assert_eq!(plan(Some("flac"), 0, "flac", Some(900)), None);
        // A format: at the bit rate asked, else its usual one.
        assert_eq!(
            plan(Some("opus"), 96, "flac", Some(900)),
            Some(Plan {
                target: Target::Opus,
                kbps: 96
            })
        );
        assert_eq!(
            plan(Some("mp3"), 0, "flac", None),
            Some(Plan {
                target: Target::Mp3,
                kbps: 192
            })
        );
        assert_eq!(plan(Some("mp3"), 0, "mp3", Some(320)), None);
        assert_eq!(
            plan(Some("OPUS"), 1000, "flac", Some(900)),
            Some(Plan {
                target: Target::Opus,
                kbps: 256
            })
        );
        // Already that format, and no larger: as it is.
        assert_eq!(plan(Some("mp3"), 320, "mp3", Some(256)), None);
        assert_eq!(
            plan(Some("mp3"), 128, "mp3", Some(320)),
            Some(Plan {
                target: Target::Mp3,
                kbps: 128
            })
        );
    }

    #[test]
    fn an_mp3_s_length_and_its_times_follow_from_its_bit_rate() {
        let p = Plan {
            target: Target::Mp3,
            kbps: 128,
        };
        assert_eq!(p.length(200), Some(200 * 16_000));
        assert_eq!(p.time_at(16_000 * 30), 30.0);
        assert_eq!(p.length(0), None);
        assert_eq!(
            Plan {
                target: Target::Opus,
                kbps: 128
            }
            .length(200),
            None
        );
    }

    #[test]
    fn ffmpeg_is_asked_for_a_stream_it_can_write_to_a_pipe() {
        let a = args(
            Plan {
                target: Target::Mp3,
                kbps: 128,
            },
            12.5,
            "pipe:0",
            "pipe",
        )
        .join(" ");
        assert!(a.contains("-protocol_whitelist pipe -ss 12.500 -i pipe:0"));
        assert!(
            a.contains("-c:a libmp3lame -b:a 128k -write_xing 0 -id3v2_version 0 -f mp3 pipe:1")
        );
        let a = args(
            Plan {
                target: Target::Opus,
                kbps: 96,
            },
            0.0,
            "song.flac",
            "file",
        )
        .join(" ");
        assert!(!a.contains("-ss"));
        assert!(a.contains("-protocol_whitelist file -i song.flac"));
        assert!(a.ends_with("-c:a libopus -b:a 96k -f ogg pipe:1"));
        let a = args(
            Plan {
                target: Target::Aac,
                kbps: 160,
            },
            3.0,
            "http://127.0.0.1:9/relay/x",
            "http,tcp",
        )
        .join(" ");
        assert!(a.contains(
            "-protocol_whitelist http,tcp -rw_timeout 60000000 -short_seek_size 1 -reconnect_on_network_error 1 -ss 3.000 -i http://127.0.0.1:9/relay/x"
        ));
        assert!(a.ends_with("-c:a aac -b:a 160k -f adts pipe:1"));
    }

    #[test]
    fn exactly_cuts_a_long_stream_and_fills_a_short_one() {
        let mut out = Vec::new();
        Exactly::new(Cursor::new(vec![1u8; 10]), 4)
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, vec![1, 1, 1, 1]);
        out.clear();
        Exactly::new(Cursor::new(vec![1u8; 3]), 6)
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, vec![1, 1, 1, 0, 0, 0]);
    }
}
