//! yt-dlp, for the songs YouTube's apps will not play to Stash: a video that asks to confirm the listener's age, one
//! whose label keeps it from those apps, one only YouTube's web player gets an address for. yt-dlp asks as more of
//! YouTube's apps than Stash does, and solves the scrambled addresses of the web's with a JavaScript runtime (Deno,
//! Node, Bun or QuickJS); with the listener's sign-in, it asks as them.
//!
//! It is used only when Stash's own way gave no stream ([`worth_trying`]), and only the address of the sound is taken
//! from it: the song still plays through the core's relay, as any other.
//!
//! Stash runs its own copy (in its data folder, beside the library), or the one on the system's PATH. Its own copy is
//! fetched from yt-dlp's releases on GitHub, with Deno beside it when the computer has no runtime yt-dlp can use, each
//! checked against the SHA-256 its release publishes ([`install`]).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::innertube::{BOT_CHECK, WHO};
use super::login::Login;
use super::parse::video_id;
use crate::addons::{AddonError, Code, Delivery, StreamInfo, Tier};
use crate::identity::clean_text;

/// How long yt-dlp may take to find a song's sound (it starts cold each time, and may solve YouTube's puzzles).
const GIVE_UP: Duration = Duration::from_secs(45);
/// How long `--version` may take.
const VERSION_WAIT: Duration = Duration::from_secs(20);
/// What yt-dlp is asked for: Opus first (as Stash's own way), then AAC, then any sound alone.
const FORMAT: &str = "bestaudio[acodec=opus]/bestaudio[ext=m4a]/bestaudio";
/// Only the fields Stash reads, as one line of JSON.
const PRINT: &str = "%(.{url,acodec,abr,asr,ext,duration})j";
/// Lossy at 256 kbps or more is the higher tier, as for Stash's own way.
const HIGH_KBPS: f64 = 256.0;

#[cfg(windows)]
const EXE: &str = "yt-dlp.exe";
#[cfg(not(windows))]
const EXE: &str = "yt-dlp";

/// The JavaScript runtimes yt-dlp can use, in its own order, by the name of their program.
const RUNTIMES: [(&str, &str); 4] = [
    ("deno", "deno"),
    ("node", "node"),
    ("quickjs", "qjs"),
    ("bun", "bun"),
];

/// Whether Stash's own way failing with `e` is worth asking yt-dlp: the video exists, and YouTube's apps would not
/// play it here (its age, its label, no plain address, every app asking whether the listener is a bot). Not when it
/// is gone, when YouTube cannot be reached, or when it asked to be left alone.
pub fn worth_trying(e: &AddonError) -> bool {
    match e.code {
        Code::RegionUnavailable | Code::Unavailable | Code::InvalidResponse => true,
        Code::RateLimited => e.message.starts_with(BOT_CHECK),
        _ => false,
    }
}

/// A copy of yt-dlp Stash can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtDlp {
    pub exe: PathBuf,
    /// Stash's own copy (else the system's).
    pub own: bool,
}

/// The program `name` on the system's PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}

/// The JavaScript runtime yt-dlp would use, by name: one in `folder` (beside Stash's copy), else one on the PATH.
/// Node counts from version 22, as yt-dlp takes it.
pub fn runtime_near(folder: Option<&Path>) -> Option<&'static str> {
    RUNTIMES.iter().find_map(|(name, program)| {
        let file = if cfg!(windows) {
            format!("{program}.exe")
        } else {
            program.to_string()
        };
        let found = folder
            .map(|dir| dir.join(&file))
            .filter(|path| path.is_file())
            .or_else(|| on_path(program))?;
        if *name == "node" {
            let mut asked = command(&found);
            asked.arg("--version");
            let (_, said, _) = run(asked, VERSION_WAIT)?;
            let major: u32 = said
                .trim()
                .trim_start_matches('v')
                .split('.')
                .next()?
                .parse()
                .ok()?;
            if major < 22 {
                return None;
            }
        }
        Some(*name)
    })
}

/// Starts `exe` with no window of its own on Windows.
fn command(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// What a program wrote, and how it ended: nothing when it ran past `wait` (it is stopped).
fn run(mut command: Command, wait: Duration) -> Option<(bool, String, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let read = |mut from: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = from.by_ref().take(1024 * 1024).read_to_end(&mut text);
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = read(Box::new(child.stdout.take()?));
    let err = read(Box::new(child.stderr.take()?));
    let began = Instant::now();
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status;
        }
        if began.elapsed() > wait {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    Some((
        status.success(),
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    ))
}

/// The listener's sign-in as a cookies file for yt-dlp, deleted when done with.
struct Cookies(PathBuf);

impl Cookies {
    fn write(login: &Login) -> Option<Cookies> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        let path = std::env::temp_dir().join(format!(
            "stash-yt-{}-{}.txt",
            std::process::id(),
            now.as_nanos()
        ));
        std::fs::write(&path, login.cookies_txt(now.as_secs())).ok()?;
        Some(Cookies(path))
    }
}

impl Drop for Cookies {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn trouble(code: Code, words: impl Into<String>) -> AddonError {
    AddonError {
        code,
        message: words.into(),
        retry_after: None,
        who: Some(WHO),
    }
}

/// What yt-dlp's complaint means, in the listener's words.
fn complaint(said: &str) -> AddonError {
    let line = said
        .lines()
        .rev()
        .find(|line| line.starts_with("ERROR:"))
        .or_else(|| said.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or_default();
    // "ERROR: [youtube] abcdefghijk: Sign in to confirm your age. …": what follows the video's id.
    let what = line
        .strip_prefix("ERROR:")
        .unwrap_or(line)
        .trim()
        .trim_start_matches("[youtube]")
        .trim();
    let what = what
        .split_once(": ")
        .filter(|(id, _)| video_id(id).is_some())
        .map_or(what, |(_, rest)| rest);
    let what = clean_text(what, 200);
    let lower = what.to_lowercase();
    if lower.contains("confirm your age") || lower.contains("age-restricted") {
        trouble(
            Code::RegionUnavailable,
            "YouTube asks to confirm your age for this song: sign in to YouTube Music in Settings, with an account \
             old enough",
        )
    } else if lower.contains("not a bot") {
        trouble(
            Code::RateLimited,
            "YouTube is checking that you are not a bot: signing in to YouTube Music in Settings helps",
        )
    } else if lower.contains("video unavailable") || lower.contains("private video") {
        trouble(
            Code::NotFound,
            format!("the video is no longer on YouTube (“{what}”)"),
        )
    } else if what.is_empty() {
        trouble(Code::Unavailable, "yt-dlp could not play it either")
    } else {
        trouble(
            Code::Unavailable,
            format!("yt-dlp could not play it either (“{what}”)"),
        )
    }
}

/// The address of the sound and what it is, from what yt-dlp printed.
fn stream_of(printed: &str) -> Result<StreamInfo, AddonError> {
    let line = printed
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or_else(|| trouble(Code::InvalidResponse, "yt-dlp gave no address"))?;
    let said: Value = serde_json::from_str(line)
        .map_err(|_| trouble(Code::InvalidResponse, "yt-dlp's answer could not be read"))?;
    let url = said
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| crate::addons::checked_url(url, "the stream").is_ok())
        .ok_or_else(|| {
            trouble(
                Code::InvalidResponse,
                "yt-dlp gave no address Stash can use",
            )
        })?;
    let codec = said
        .get("acodec")
        .and_then(Value::as_str)
        .map(|codec| match codec {
            "opus" => "opus",
            codec if codec.starts_with("mp4a") => "aac",
            _ => "other",
        });
    let container = said
        .get("ext")
        .and_then(Value::as_str)
        .map(|ext| match ext {
            "webm" => "webm",
            _ => "mp4",
        });
    let kbps = said.get("abr").and_then(Value::as_f64);
    let expires_at = url::Url::parse(url).ok().and_then(|url| {
        url.query_pairs()
            .find(|(name, _)| name == "expire")?
            .1
            .parse::<i64>()
            .ok()?
            .checked_mul(1_000)
    });
    Ok(StreamInfo {
        url: url.to_string(),
        manifest: Delivery::None,
        tier: if kbps.is_some_and(|kbps| kbps >= HIGH_KBPS) {
            Tier::High
        } else {
            Tier::Low
        },
        codec: codec.map(str::to_string),
        container: container.map(str::to_string),
        bit_depth: None,
        sample_rate: said
            .get("asr")
            .and_then(Value::as_u64)
            .and_then(|rate| u32::try_from(rate).ok()),
        bitrate: kbps
            .map(|kbps| (kbps * 1_000.0) as u64)
            .filter(|rate| *rate > 0),
        duration_ms: said
            .get("duration")
            .and_then(Value::as_f64)
            .map(|seconds| (seconds * 1_000.0) as u64),
        expires_at,
        encrypted: None,
    })
}

impl YtDlp {
    /// Stash's own copy in `folder`, else the one on the system's PATH.
    pub fn find(folder: &Path) -> Option<YtDlp> {
        let own = folder.join(EXE);
        if own.is_file() {
            return Some(YtDlp {
                exe: own,
                own: true,
            });
        }
        on_path("yt-dlp").map(|exe| YtDlp { exe, own: false })
    }

    /// Its version ("2026.11.10"), as it says.
    pub fn version(&self) -> Option<String> {
        let mut asked = command(&self.exe);
        asked.arg("--version");
        let (ok, out, _) = run(asked, VERSION_WAIT)?;
        let version = clean_text(out.trim(), 40);
        (ok && !version.is_empty()).then_some(version)
    }

    /// The JavaScript runtime it will use, by name: one beside it (Stash puts Deno there), else one on the PATH.
    pub fn runtime(&self) -> Option<&'static str> {
        runtime_near(self.exe.parent())
    }

    /// Where the video `id` plays from, asked as the listener when `login` is given.
    pub fn stream(&self, id: &str, login: Option<&Login>) -> Result<StreamInfo, AddonError> {
        let Some(id) = video_id(id) else {
            return Err(trouble(Code::NotFound, "that is not a YouTube video"));
        };
        let cookies = login.and_then(Cookies::write);
        let mut asked = command(&self.exe);
        asked.args([
            "--ignore-config",
            "--no-playlist",
            "--no-warnings",
            "--no-progress",
            "--socket-timeout",
            "15",
            "-f",
            FORMAT,
            "--print",
            PRINT,
        ]);
        // Deno is used when found; the others only when allowed (yt-dlp takes the first it finds, in its order).
        for (name, _) in &RUNTIMES[1..] {
            asked.args(["--js-runtimes", name]);
        }
        if let Some(cookies) = &cookies {
            asked.arg("--cookies").arg(&cookies.0);
        }
        asked
            .arg("--")
            .arg(format!("https://music.youtube.com/watch?v={id}"));
        let Some((ok, out, err)) = run(asked, GIVE_UP) else {
            return Err(trouble(
                Code::Timeout,
                "yt-dlp took too long to find the song",
            ));
        };
        drop(cookies);
        if ok {
            stream_of(&out)
        } else {
            Err(complaint(&err))
        }
    }
}

// ---- Stash's own copy: fetched from the releases on GitHub

/// What a release names the files for this computer: yt-dlp's program, and Deno's archive (none: Deno has no build
/// for it).
fn assets() -> Option<(&'static str, Option<&'static str>)> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    Some(match (os, arch) {
        ("windows", "x86_64") => ("yt-dlp.exe", Some("deno-x86_64-pc-windows-msvc.zip")),
        ("windows", "aarch64") => ("yt-dlp_arm64.exe", Some("deno-aarch64-pc-windows-msvc.zip")),
        ("windows", "x86") => ("yt-dlp_x86.exe", None),
        ("macos", "x86_64") => ("yt-dlp_macos", Some("deno-x86_64-apple-darwin.zip")),
        ("macos", "aarch64") => ("yt-dlp_macos", Some("deno-aarch64-apple-darwin.zip")),
        ("linux", "x86_64") => ("yt-dlp_linux", Some("deno-x86_64-unknown-linux-gnu.zip")),
        ("linux", "aarch64") => (
            "yt-dlp_linux_aarch64",
            Some("deno-aarch64-unknown-linux-gnu.zip"),
        ),
        _ => return None,
    })
}

/// Whether yt-dlp has a build for this computer that Stash can fetch.
pub fn can_install() -> bool {
    assets().is_some()
}

const YT_DLP_RELEASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const DENO_RELEASE: &str = "https://github.com/denoland/deno/releases/latest/download";
/// The largest file taken: yt-dlp's program is about 18 MB, Deno's archive about 45.
const LARGEST: u64 = 160 * 1024 * 1024;

/// How far a download is, for the window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// What is being fetched ("yt-dlp", "Deno").
    pub what: String,
    pub got: u64,
    pub total: Option<u64>,
}

fn fetch(
    agent: &ureq::Agent,
    url: &str,
    what: &str,
    told: &dyn Fn(Progress),
) -> Result<Vec<u8>, String> {
    let mut answer = agent
        .get(url)
        .call()
        .map_err(|e| format!("{what} could not be fetched ({e})"))?;
    if !answer.status().is_success() {
        return Err(format!(
            "{what} could not be fetched (GitHub said {})",
            answer.status().as_u16()
        ));
    }
    let total = answer
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|total| total > LARGEST) {
        return Err(format!("{what} is larger than Stash takes"));
    }
    let mut body = answer.body_mut().with_config().limit(LARGEST).reader();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut chunk = vec![0u8; 256 * 1024];
    let mut last = Instant::now();
    loop {
        let n = body
            .read(&mut chunk)
            .map_err(|e| format!("{what} stopped coming in ({e})"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if last.elapsed() > Duration::from_millis(200) {
            last = Instant::now();
            told(Progress {
                what: what.to_string(),
                got: bytes.len() as u64,
                total,
            });
        }
    }
    Ok(bytes)
}

/// The SHA-256 a release's list of sums gives `file`: a "<hex>  <file>" line (yt-dlp's list, Deno's for Linux and
/// macOS), or the one sum of a list that names the file (Deno's for Windows, in PowerShell's "Hash : <hex>" form).
fn sum_for(list: &str, file: &str) -> Option<String> {
    let hex = |token: &str| {
        (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| token.to_ascii_lowercase())
    };
    let paired = list.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sum = hex(parts.next()?)?;
        (parts.next()?.trim_start_matches('*') == file).then_some(sum)
    });
    if paired.is_some() {
        return paired;
    }
    let sums: Vec<String> = list
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter_map(hex)
        .collect();
    match sums.as_slice() {
        [only] if list.contains(file) => Some(only.clone()),
        _ => None,
    }
}

fn checked(bytes: &[u8], list: &str, file: &str, what: &str) -> Result<(), String> {
    let expected = sum_for(list, file)
        .ok_or_else(|| format!("{what}'s release does not say what {file} should be"))?;
    let got: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got == expected {
        Ok(())
    } else {
        Err(format!(
            "{what} did not match what its release says it is: nothing was kept (try again)"
        ))
    }
}

/// Writes `bytes` as `name` in `folder`, whole or not at all, runnable.
fn keep(folder: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let part = folder.join(format!("{name}.part"));
    std::fs::write(&part, bytes).map_err(|e| format!("{name} could not be written ({e})"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&part, folder.join(name)).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("{name} could not be put in place ({e})")
    })
}

/// Fetches yt-dlp into `folder` (replacing Stash's copy there), and Deno beside it when `with_deno`: each from its
/// latest release on GitHub, checked against the SHA-256 the release publishes.
pub fn install(folder: &Path, with_deno: bool, told: &dyn Fn(Progress)) -> Result<YtDlp, String> {
    let Some((program, deno)) = assets() else {
        return Err("yt-dlp has no build for this computer".to_string());
    };
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("Stash's tools folder could not be made ({e})"))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(15 * 60)))
        .build()
        .into();
    let quiet = |_: Progress| {};
    let sums = fetch(
        &agent,
        &format!("{YT_DLP_RELEASE}/SHA2-256SUMS"),
        "yt-dlp's list of sums",
        &quiet,
    )?;
    let bytes = fetch(
        &agent,
        &format!("{YT_DLP_RELEASE}/{program}"),
        "yt-dlp",
        told,
    )?;
    checked(&bytes, &String::from_utf8_lossy(&sums), program, "yt-dlp")?;
    keep(folder, EXE, &bytes)?;
    if with_deno && let Some(archive) = deno {
        let sum = fetch(
            &agent,
            &format!("{DENO_RELEASE}/{archive}.sha256sum"),
            "Deno's sum",
            &quiet,
        )?;
        let zipped = fetch(&agent, &format!("{DENO_RELEASE}/{archive}"), "Deno", told)?;
        checked(&zipped, &String::from_utf8_lossy(&sum), archive, "Deno")?;
        let name = if cfg!(windows) { "deno.exe" } else { "deno" };
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zipped))
            .map_err(|e| format!("Deno's archive could not be opened ({e})"))?;
        let mut file = archive
            .by_name(name)
            .map_err(|_| "Deno's archive holds no program".to_string())?;
        let mut program = Vec::new();
        file.by_ref()
            .take(LARGEST * 2)
            .read_to_end(&mut program)
            .map_err(|e| format!("Deno could not be unpacked ({e})"))?;
        keep(folder, name, &program)?;
    }
    YtDlp::find(folder)
        .filter(|found| found.own)
        .ok_or_else(|| "yt-dlp was fetched but is not in place".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_yt_dlp_printed_is_read_as_a_stream() {
        let printed = "{\"url\": \"https://rr1.googlevideo.com/videoplayback?expire=1794900000&itag=251\", \"acodec\": \"opus\", \"abr\": 129.475, \"asr\": 48000, \"ext\": \"webm\", \"duration\": 213.0}\n";
        let stream = stream_of(printed).unwrap();
        assert_eq!(stream.codec.as_deref(), Some("opus"));
        assert_eq!(stream.container.as_deref(), Some("webm"));
        assert_eq!(stream.bitrate, Some(129_475));
        assert_eq!(stream.sample_rate, Some(48_000));
        assert_eq!(stream.duration_ms, Some(213_000));
        assert_eq!(stream.expires_at, Some(1_794_900_000_000));
        assert_eq!(stream.tier, Tier::Low);
        // An address Stash would not take (not https), or none at all.
        assert!(stream_of("{\"url\": \"file:///etc/passwd\"}").is_err());
        assert!(stream_of("").is_err());
    }

    #[test]
    fn yt_dlp_s_complaints_are_put_in_the_listener_s_words() {
        let age = complaint(
            "WARNING: something\nERROR: [youtube] abcdefghijk: Sign in to confirm your age. This video may be inappropriate for some users.\n",
        );
        assert_eq!(age.code, Code::RegionUnavailable);
        assert!(age.message.contains("confirm your age"), "{}", age.message);
        let gone = complaint("ERROR: [youtube] abcdefghijk: Video unavailable\n");
        assert_eq!(gone.code, Code::NotFound);
        let bot = complaint(
            "ERROR: [youtube] abcdefghijk: Sign in to confirm you’re not a bot. Use --cookies…\n",
        );
        assert_eq!(bot.code, Code::RateLimited);
        let other = complaint("ERROR: [youtube] abcdefghijk: Requested format is not available\n");
        assert_eq!(other.code, Code::Unavailable);
        assert!(
            other.message.contains("Requested format is not available"),
            "{}",
            other.message
        );
    }

    #[test]
    fn only_trouble_yt_dlp_could_get_past_sends_a_song_to_it() {
        let e = |code: Code, message: &str| AddonError {
            code,
            message: message.to_string(),
            retry_after: None,
            who: Some(WHO),
        };
        assert!(worth_trying(&e(
            Code::RegionUnavailable,
            "YouTube does not play this video without signing in"
        )));
        assert!(worth_trying(&e(
            Code::Unavailable,
            "YouTube gave no address its sound plays from"
        )));
        assert!(worth_trying(&e(Code::RateLimited, BOT_CHECK)));
        assert!(!worth_trying(&e(
            Code::RateLimited,
            "YouTube is being asked too often"
        )));
        assert!(!worth_trying(&e(
            Code::NotFound,
            "the video is no longer on YouTube"
        )));
        assert!(!worth_trying(&e(Code::Network, "")));
    }

    #[test]
    fn a_release_s_sums_are_read_for_the_file_asked() {
        let list = format!(
            "{}  yt-dlp\n{}  yt-dlp.exe\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(sum_for(&list, "yt-dlp.exe"), Some("b".repeat(64)));
        assert_eq!(sum_for(&list, "yt-dlp_macos"), None);
        // Deno's: one line for its archive, or PowerShell's form on Windows.
        let deno = format!("{}  deno-x86_64-unknown-linux-gnu.zip\n", "c".repeat(64));
        assert_eq!(
            sum_for(&deno, "deno-x86_64-unknown-linux-gnu.zip"),
            Some("c".repeat(64))
        );
        let windows = format!(
            "\r\nAlgorithm : SHA256\r\nHash      : {}\r\nPath      : C:\\a\\deno\\target\\release\\deno-x86_64-pc-windows-msvc.zip\r\n",
            "D".repeat(64)
        );
        assert_eq!(
            sum_for(&windows, "deno-x86_64-pc-windows-msvc.zip"),
            Some("d".repeat(64))
        );
        assert_eq!(sum_for(&windows, "deno-aarch64-pc-windows-msvc.zip"), None);
        assert!(checked(b"x", &list, "yt-dlp.exe", "yt-dlp").is_err());
        let right: String = Sha256::digest(b"x")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(
            checked(
                b"x",
                &format!("{right}  yt-dlp.exe\n"),
                "yt-dlp.exe",
                "yt-dlp"
            )
            .is_ok()
        );
    }
}
