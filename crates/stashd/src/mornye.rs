//! Mornye extensions in the core (see `stash_mornye`): each one installed, switched on, and able to search and
//! download is one of the listener's sources, after their own add-ons and before YouTube Music. Its songs are found
//! by asking it, and played from the file it downloads: as the file comes in when it can be (a single-file FLAC or
//! fast-start MP4 whose length the server told), else once it is whole. The engine reads the file through the
//! relay, as any other source's stream: `/mornye/play/{token}` is the core's own address for it.
//!
//! The add-ons, their repositories, settings, logins and verification are kept under `mornye` beside the library;
//! what they keep is sealed with a key kept in the library (`mornye.seal`).
//!
//! Routes (the window's): `GET /api/mornye`; `POST /api/mornye/repositories {url}`,
//! `/api/mornye/repositories/remove {id}`, `/api/mornye/refresh {id?}`; `POST /api/mornye/install {repository, id}`,
//! `/api/mornye/install-file` (the package's bytes), `/api/mornye/remove {key}`, `/api/mornye/enabled {key, on}`;
//! `GET /api/mornye/settings?key=`, `POST /api/mornye/settings {key, values}`; `GET /api/mornye/access?key=`,
//! `POST /api/mornye/verify {key}` (opens the verification page in a window of its own),
//! `POST /api/mornye/callback {key, url}` (the page's last address, pasted), `POST /api/mornye/logout {key}`,
//! `POST /api/mornye/action {key, action, input?, token?}`; `POST /api/mornye/ffmpeg` (fetch Stash's own FFmpeg).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::{
    Addon, AddonError, Artwork, BuiltIn, Code, Delivery, Manifest, StreamInfo, Tier, Track,
};
use stash_core::reach::Reach;
use stash_mornye::Trouble;
use stash_mornye::catalogue::{self, Kind, MAlbum, MArtist};
use stash_mornye::host::Transfer;
use stash_mornye::live::{self, Growing, Playable};
use stash_mornye::packages::{DownloadRequest, Downloaded, Packages};
use stash_mornye::track::MTrack;
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::serve::{Reply, State, body_json, offline, refuse, reply_json};

/// The ids Mornye add-ons are listed under among the listener's sources: `mornye.<key>` (a Mornye name may hold
/// what a Stash id may not).
pub(crate) const PREFIX: &str = "mornye.";
/// The setting that holds the key what add-ons keep is sealed with (hex).
const SEAL: &str = "mornye.seal";
/// How often the repositories are read again, when the window looks.
const REFRESH_EVERY: Duration = Duration::from_secs(6 * 3600);
/// The songs kept downloaded, to play again without asking (the oldest go first).
const READY_MOST: usize = 24;
/// How long a play address is answered.
const PLAY_FOR: Duration = Duration::from_secs(12 * 3600);
const PLAYS_MOST: usize = 64;
/// How long a song may take to download before playing gives up.
const PREPARE_MOST: Duration = Duration::from_secs(15 * 60);
pub(crate) const MANAGED_THERE: &str =
    "Mornye extensions are managed in Settings, under Mornye extensions";

/// Opens a verification (or sign-in) page in a window of its own. `catches` says whether the window is about to
/// open the add-on's callback: the window then stops there, hands the address to `caught`, and closes. `closed` is
/// called when it closes, however it did.
pub struct VerifyWindow {
    pub url: String,
    pub title: String,
    pub catches: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    pub caught: Arc<dyn Fn(String) + Send + Sync>,
    pub closed: Box<dyn FnOnce() + Send>,
}

/// The shell's way of opening a [`VerifyWindow`]; false when it could not.
pub type VerifyShell = Arc<dyn Fn(VerifyWindow) -> bool + Send + Sync>;

/// What a play address serves.
enum Body {
    File(PathBuf),
    Growing(Arc<Growing>),
}

struct Play {
    body: Body,
    total: u64,
    content_type: &'static str,
    made: Instant,
}

/// A song downloaded whole, kept to be played again.
struct Ready {
    /// `<key>\n<id>`.
    song: String,
    downloaded: Downloaded,
}

pub(crate) struct Mornye {
    packages: Arc<Packages>,
    port: u16,
    plays: Mutex<HashMap<String, Arc<Play>>>,
    ready: Mutex<VecDeque<Ready>>,
    made: AtomicU64,
    refreshing: AtomicBool,
    /// The add-on whose page is open in the verification window, and what came of the last verification.
    verifying: Mutex<Option<String>>,
    verified: Mutex<HashMap<String, String>>,
    pub(crate) shell: OnceLock<VerifyShell>,
    /// Stash's tools folder (its own FFmpeg), how far a fetch of FFmpeg is, and why the last one failed.
    tools: PathBuf,
    fetching: Mutex<Option<(u64, Option<u64>)>>,
    ffmpeg_trouble: Mutex<Option<String>>,
    /// An extension's verification came back from its window: the sources are to be read again.
    changed: AtomicBool,
}

/// A name kept for as long as the core runs (an [`AddonError`] names who failed with a `&'static str`).
fn kept_name(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap();
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(kept) = names.get(name) {
        return kept;
    }
    let kept: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(kept);
    kept
}

/// Whether an extension's qualities name a lossless one.
fn lossless_of(quality_options: &Value) -> bool {
    quality_options.as_array().is_some_and(|options| {
        options.iter().any(|o| {
            let words = format!(
                "{} {}",
                o.get("id").and_then(Value::as_str).unwrap_or_default(),
                o.get("kind").and_then(Value::as_str).unwrap_or_default()
            )
            .to_ascii_lowercase();
            words.contains("lossless") || words.contains("flac")
        })
    })
}

fn trouble_of(who: &'static str, trouble: Trouble) -> AddonError {
    let (code, retry_after) = match &trouble {
        Trouble::Failed(_) => (Code::Unavailable, None),
        Trouble::Unavailable(_) => (Code::NotFound, None),
        Trouble::RateLimited { seconds, .. } => (
            Code::RateLimited,
            Some((*seconds).min(u32::MAX as u64) as u32),
        ),
        Trouble::Verify { .. } => (Code::Unauthorized, None),
        Trouble::Missing(_) => (Code::Refused, None),
    };
    let message = match &trouble {
        Trouble::Verify { .. } => format!(
            "{who} asks you to pass its verification first: Settings, Mornye extensions, {who}, Verify"
        ),
        other => format!("{who}: {}", other.message()),
    };
    AddonError {
        code,
        message,
        retry_after,
        who: Some(who),
    }
}

/// The tier a probed file is: hi-res above CD, lossless for FLAC/ALAC/PCM/WavPack, high at 256 kbps or more.
fn tier_of(
    codec: &str,
    bit_depth: Option<u32>,
    sample_rate: Option<u32>,
    kbps: Option<u64>,
) -> Tier {
    if matches!(codec, "flac" | "alac" | "pcm") {
        if bit_depth.is_some_and(|b| b > 16) || sample_rate.is_some_and(|r| r > 48_000) {
            Tier::Hires
        } else {
            Tier::Lossless
        }
    } else if kbps.is_some_and(|k| k >= 256) {
        Tier::High
    } else {
        Tier::Low
    }
}

/// The codec Stash calls a probed one.
fn codec_of(probed: &str) -> &'static str {
    match probed {
        "flac" | "wavpack" => "flac",
        "alac" => "alac",
        "aac" => "aac",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "vorbis",
        p if p.starts_with("pcm") => "pcm",
        _ => "other",
    }
}

/// The container a file names, when it is one a saved song is not kept in (WebM: no tags of its own that Stash writes).
fn container_of(path: &Path) -> Option<&'static str> {
    (content_type_of(path) == "audio/webm").then_some("webm")
}

fn content_type_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "flac" => "audio/flac",
        "m4a" | "mp4" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "webm" => "audio/webm",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    }
}

/// A song of an add-on as a track of the source.
fn track(found: MTrack, lossless: bool) -> Option<Track> {
    if found.artists.trim().is_empty() || found.name.trim().is_empty() {
        return None;
    }
    let hires =
        found.bit_depth.is_some_and(|b| b > 16) || found.sample_rate.is_some_and(|r| r > 48_000);
    let max_tier = match found.codec.as_deref() {
        Some("FLAC" | "ALAC") if hires => Some(Tier::Hires),
        Some("FLAC" | "ALAC") => Some(Tier::Lossless),
        Some(_) => None,
        None if lossless => Some(if hires { Tier::Hires } else { Tier::Lossless }),
        None => None,
    };
    Some(Track {
        id: found.id,
        title: found.name,
        artist: found.artists,
        album: found.album,
        duration_ms: found.duration_ms,
        isrc: found.isrc,
        explicit: None,
        max_tier,
        artwork: found
            .artwork
            .into_iter()
            .map(|url| Artwork { url })
            .collect(),
    })
}

/// One Mornye add-on as a source.
struct Source {
    mornye: Arc<Mornye>,
    key: String,
    who: &'static str,
    /// It offers a lossless quality.
    lossless: bool,
}

impl BuiltIn for Source {
    fn search(&self, query: &str, limit: u32) -> Result<Vec<Track>, AddonError> {
        let found = self
            .mornye
            .packages
            .search(&self.key, query, limit.min(20) as usize)
            .map_err(|t| trouble_of(self.who, t))?;
        Ok(found
            .into_iter()
            .filter_map(|t| track(t, self.lossless))
            .collect())
    }

    fn stream(&self, id: &str) -> Result<StreamInfo, AddonError> {
        self.mornye
            .prepare(&self.key, id)
            .map_err(|t| trouble_of(self.who, t))
    }
}

impl Mornye {
    /// The add-ons under `folder`, sealed with the library's key (made the first time).
    pub(crate) fn open(
        library: &Mutex<Library>,
        folder: PathBuf,
        port: u16,
    ) -> Result<Arc<Mornye>> {
        let seal = {
            let mut library = library.lock().unwrap();
            match library.setting(SEAL)?.filter(|hex| hex.len() == 64) {
                Some(hex) => hex,
                None => {
                    let hex = stash_mornye::hex(&stash_mornye::random_bytes(32));
                    library.set_setting(SEAL, &hex)?;
                    hex
                }
            }
        };
        let mut master = [0u8; 32];
        for (i, byte) in master.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&seal[i * 2..i * 2 + 2], 16).unwrap_or(0);
        }
        let packages = Packages::open(folder.clone(), master, env!("CARGO_PKG_VERSION"))
            .map_err(anyhow::Error::msg)?;
        let tools = folder
            .parent()
            .map(|data| data.join("tools"))
            .unwrap_or_else(|| PathBuf::from("tools"));
        packages.set_ffmpeg(stash_mornye::ffmpeg::find(&tools));
        Ok(Arc::new(Mornye {
            packages,
            port,
            plays: Mutex::default(),
            ready: Mutex::default(),
            made: AtomicU64::new(0),
            refreshing: AtomicBool::new(false),
            verifying: Mutex::default(),
            verified: Mutex::default(),
            shell: OnceLock::new(),
            tools,
            fetching: Mutex::default(),
            ffmpeg_trouble: Mutex::default(),
            changed: AtomicBool::new(false),
        }))
    }

    /// The installed add-ons that can be searched and played, as sources, in the order they were installed. One
    /// that still needs its verification page or its sign-in is left out until it has them (it would only fail).
    pub(crate) fn sources(self: &Arc<Self>) -> Vec<(bool, Addon)> {
        let mut installed = self.packages.installed();
        installed.sort_by(|a, b| {
            a.installed_at
                .cmp(&b.installed_at)
                .then_with(|| a.key.cmp(&b.key))
        });
        installed
            .into_iter()
            .filter(|a| a.playable && a.access.ready)
            .filter_map(|a| {
                let lossless = lossless_of(&a.quality_options);
                // A metadata provider: its albums, artists and playlists are pages of the window's.
                let catalogue = self.packages.has_catalogue(&a.key);
                let manifest = Manifest {
                    stash: 1,
                    id: format!("{PREFIX}{}", a.key),
                    name: a.display_name.clone(),
                    version: a.version.clone(),
                    kind: "server".to_string(),
                    description: Some(if a.description.is_empty() {
                        "A Mornye extension.".to_string()
                    } else {
                        a.description.clone()
                    }),
                    // What it plays is the whole song, opened (an encrypted one is refused): that is what is saved.
                    resources: [
                        Some("search"),
                        Some("stream"),
                        Some("download"),
                        catalogue.then_some("catalogue"),
                    ]
                    .into_iter()
                    .flatten()
                    .map(str::to_string)
                    .collect(),
                    dialect: Default::default(),
                    resolve_by_isrc: false,
                    quality: None,
                    no_downloads: false,
                    sent: Vec::new(),
                    // Its songs come from the core's own address (the add-on's reach is the host's to hold).
                    reach: Some(Reach::Here),
                };
                let source = Source {
                    mornye: Arc::clone(self),
                    key: a.key.clone(),
                    who: kept_name(&a.display_name),
                    lossless,
                };
                let addon = Addon::built_in(manifest, Arc::new(source), Reach::Here).ok()?;
                Some((a.enabled, addon))
            })
            .collect()
    }

    // ---- playing

    fn token(&self) -> String {
        let n = self.made.fetch_add(1, Ordering::Relaxed);
        format!(
            "{}{n:x}",
            stash_mornye::hex(&stash_mornye::random_bytes(12))
        )
    }

    fn hand(&self, body: Body, total: u64, content_type: &'static str) -> String {
        let token = self.token();
        let mut plays = self.plays.lock().unwrap();
        plays.retain(|_, p| p.made.elapsed() < PLAY_FOR);
        while plays.len() >= PLAYS_MOST {
            let oldest = plays
                .iter()
                .min_by_key(|(_, p)| p.made)
                .map(|(t, _)| t.clone());
            match oldest {
                Some(t) => plays.remove(&t),
                None => break,
            };
        }
        plays.insert(
            token.clone(),
            Arc::new(Play {
                body,
                total,
                content_type,
                made: Instant::now(),
            }),
        );
        format!("http://127.0.0.1:{}/mornye/play/{token}", self.port)
    }

    /// `container`: said of a WebM file, which plays but is not kept as a saved song.
    fn stream_of(
        &self,
        url: String,
        codec: &str,
        info: &Value,
        total: u64,
        container: Option<&str>,
    ) -> StreamInfo {
        let number = |name: &str| info.get(name).and_then(Value::as_u64);
        let bit_depth = number("bit_depth").map(|b| b as u32).filter(|b| *b > 0);
        let sample_rate = number("sample_rate").map(|r| r as u32).filter(|r| *r > 0);
        let duration_ms = number("duration_ms").filter(|d| *d > 0);
        let bitrate = number("bitrate")
            .filter(|b| *b > 0)
            .or_else(|| duration_ms.map(|ms| total.saturating_mul(8_000) / ms.max(1)));
        let codec = codec_of(codec);
        StreamInfo {
            url,
            manifest: Delivery::None,
            tier: tier_of(codec, bit_depth, sample_rate, bitrate.map(|b| b / 1000)),
            codec: Some(codec.to_string()),
            container: container.map(str::to_string),
            bit_depth,
            sample_rate,
            bitrate,
            duration_ms,
            expires_at: None,
            encrypted: None,
        }
    }

    /// A song downloaded whole before, still there.
    fn ready_one(&self, song: &str) -> Option<(PathBuf, Value)> {
        let ready = self.ready.lock().unwrap();
        ready
            .iter()
            .find(|r| r.song == song && r.downloaded.file.is_file())
            .map(|r| (r.downloaded.file.clone(), r.downloaded.info.clone()))
    }

    fn keep_ready(&self, song: String, downloaded: Downloaded) {
        let mut ready = self.ready.lock().unwrap();
        if let Some(at) = ready.iter().position(|r| r.song == song)
            && let Some(old) = ready.remove(at)
        {
            old.downloaded.cleanup();
        }
        ready.push_back(Ready { song, downloaded });
        while ready.len() > READY_MOST {
            if let Some(old) = ready.pop_front() {
                old.downloaded.cleanup();
            }
        }
    }

    /// Where the add-on `key`'s song `id` plays from: the core's address for its file, as soon as it can be played.
    fn prepare(self: &Arc<Self>, key: &str, id: &str) -> Result<StreamInfo, Trouble> {
        let song = format!("{key}\n{id}");
        if let Some((file, info)) = self.ready_one(&song) {
            let total = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            let codec = info
                .get("codec")
                .and_then(Value::as_str)
                .unwrap_or("other")
                .to_string();
            let url = self.hand(Body::File(file.clone()), total, content_type_of(&file));
            return Ok(self.stream_of(url, &codec, &info, total, container_of(&file)));
        }
        enum Came {
            Live(Arc<Growing>, Playable),
            Whole(Downloaded),
            Failed(Trouble),
        }
        let (tx, rx) = mpsc::channel::<Came>();
        let growing: Arc<Mutex<Option<Arc<Growing>>>> = Arc::default();
        let transfer = {
            let (tx, growing) = (tx.clone(), Arc::clone(&growing));
            move |t: &Transfer| {
                if t.total == 0 || !live::playable_name(&t.file) {
                    return;
                }
                let mut slot = growing.lock().unwrap();
                if let Some(g) = slot.as_ref() {
                    g.wrote(t.written);
                    return;
                }
                if t.written < live::ENOUGH {
                    return;
                }
                let mut head = vec![0u8; live::ENOUGH as usize];
                let read = File::open(if t.done { &t.file } else { &t.part })
                    .and_then(|mut f| f.read_exact(&mut head));
                if read.is_err() {
                    return;
                }
                if let Some(playable) = live::playable(&head) {
                    let g = Growing::new(t.part.clone(), t.file.clone(), t.total);
                    g.wrote(t.written);
                    *slot = Some(Arc::clone(&g));
                    let _ = tx.send(Came::Live(g, playable));
                }
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        // Its length as the search gave it: what the file's rate is reckoned from while it comes in.
        let known_ms = self.packages.track(key, id).and_then(|t| t.duration_ms);
        let request = DownloadRequest {
            quality: None,
            for_playback: true,
            hint: self
                .packages
                .track(key, id)
                .map(|t| json!({ "id": t.id, "name": t.name, "artists": t.artists, "duration_ms": t.duration_ms })),
            progress: None,
            transfer: Some(Arc::new(transfer)),
            cancel: Arc::clone(&cancel),
        };
        let mornye = Arc::clone(self);
        let (key_owned, id_owned) = (key.to_string(), id.to_string());
        std::thread::Builder::new()
            .name("mornye-download".into())
            .spawn(move || {
                let came = mornye.packages.download(&key_owned, &id_owned, request);
                let live = growing.lock().unwrap().clone();
                match came {
                    Ok(downloaded) => {
                        if let Some(g) = &live {
                            g.finished(true);
                        }
                        let _ = tx.send(Came::Whole(downloaded.clone()));
                        if downloaded.decryption_key.is_none() {
                            mornye.keep_ready(format!("{key_owned}\n{id_owned}"), downloaded);
                        } else if live.is_none() {
                            downloaded.cleanup();
                        }
                    }
                    Err(trouble) => {
                        if let Some(g) = &live {
                            g.finished(false);
                        }
                        let _ = tx.send(Came::Failed(trouble));
                    }
                }
            })
            .map_err(|e| Trouble::Failed(format!("the download could not start ({e})")))?;
        let came = rx.recv_timeout(PREPARE_MOST).unwrap_or_else(|_| {
            cancel.store(true, Ordering::SeqCst);
            Came::Failed(Trouble::Failed("the song took too long to download".into()))
        });
        match came {
            Came::Live(g, playable) => {
                let total = g.total();
                let mut info = match &playable {
                    Playable::Flac {
                        sample_rate,
                        bit_depth,
                    } => {
                        json!({ "sample_rate": sample_rate, "bit_depth": bit_depth })
                    }
                    Playable::Mp4 => json!({}),
                };
                info["duration_ms"] = json!(known_ms);
                let codec = match playable {
                    Playable::Flac { .. } => "flac",
                    // A fast-start MP4: AAC or ALAC; told apart once it is whole.
                    Playable::Mp4 => "aac",
                };
                let url = self.hand(Body::Growing(g), total, playable.content_type());
                Ok(self.stream_of(url, codec, &info, total, None))
            }
            Came::Whole(downloaded) => {
                if downloaded.decryption_key.is_some() {
                    return Err(Trouble::Failed(
                        "this song comes encrypted, and Stash needs FFmpeg to open it (put ffmpeg on the PATH, or in Stash's tools folder)"
                            .into(),
                    ));
                }
                let total = std::fs::metadata(&downloaded.file)
                    .map(|m| m.len())
                    .unwrap_or(0);
                let codec = downloaded
                    .info
                    .get("codec")
                    .and_then(Value::as_str)
                    .unwrap_or("other")
                    .to_string();
                let url = self.hand(
                    Body::File(downloaded.file.clone()),
                    total,
                    content_type_of(&downloaded.file),
                );
                let container = container_of(&downloaded.file);
                Ok(self.stream_of(url, &codec, &downloaded.info, total, container))
            }
            Came::Failed(trouble) => Err(trouble),
        }
    }

    /// Answers the engine's (the relay's) request for a play address, on a thread of its own.
    pub(crate) fn answer(self: &Arc<Self>, request: Request) {
        let mornye = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("mornye-play".into())
            .spawn(move || mornye.play(request));
        if let Err(e) = spawned {
            eprintln!("a Mornye song could not be served: {e}");
        }
    }

    fn play(&self, request: Request) {
        let token = request
            .url()
            .trim_start_matches("/mornye/play/")
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_string();
        let play = self.plays.lock().unwrap().get(&token).cloned();
        let Some(play) = play else {
            let _ = request
                .respond(Response::from_string("no such song").with_status_code(StatusCode(404)));
            return;
        };
        if *request.method() != Method::Get && *request.method() != Method::Head {
            let _ = request.respond(Response::from_string("").with_status_code(StatusCode(405)));
            return;
        }
        let range = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Range"))
            .and_then(|h| parse_range(h.value.as_str(), play.total));
        let (start, end) = range.unwrap_or((0, play.total.saturating_sub(1)));
        if play.total == 0 || start >= play.total {
            let _ = request.respond(
                Response::from_string("")
                    .with_status_code(StatusCode(416))
                    .with_header(header("Content-Range", &format!("bytes */{}", play.total))),
            );
            return;
        }
        let length = end - start + 1;
        let reader: Box<dyn Read + Send> = match &play.body {
            Body::File(path) => match File::open(path).and_then(|mut f| {
                f.seek(SeekFrom::Start(start))?;
                Ok(f)
            }) {
                Ok(file) => Box::new(file.take(length)),
                Err(_) => {
                    let _ = request
                        .respond(Response::from_string("gone").with_status_code(StatusCode(410)));
                    return;
                }
            },
            Body::Growing(g) => Box::new(GrowingReader {
                growing: Arc::clone(g),
                at: start,
                left: length,
            }),
        };
        let mut headers = vec![
            header("Content-Type", play.content_type),
            header("Accept-Ranges", "bytes"),
        ];
        let status = if range.is_some() {
            headers.push(header(
                "Content-Range",
                &format!("bytes {start}-{end}/{}", play.total),
            ));
            206
        } else {
            200
        };
        let response = Response::new(
            StatusCode(status),
            headers,
            reader,
            Some(length as usize),
            None,
        );
        let _ = request.respond(response);
    }

    // ---- verifying

    /// Whether an extension's verification came back since this was last asked.
    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }

    /// Opens the add-on's verification (or sign-in) page in the shell's window; false without a shell.
    fn open_window(self: &Arc<Self>, key: &str, url: String, title: String) -> bool {
        let Some(shell) = self.shell.get() else {
            return false;
        };
        *self.verifying.lock().unwrap() = Some(key.to_string());
        self.verified.lock().unwrap().remove(key);
        let catches = {
            let (packages, key) = (Arc::clone(&self.packages), key.to_string());
            Arc::new(move |url: &str| packages.is_callback(&key, url))
        };
        let caught = {
            let (mornye, key) = (Arc::clone(self), key.to_string());
            Arc::new(move |url: String| {
                let said = match mornye.packages.callback(&key, &url) {
                    Ok(_) => "ok".to_string(),
                    Err(e) => e,
                };
                mornye.verified.lock().unwrap().insert(key.clone(), said);
                mornye.changed.store(true, Ordering::SeqCst);
            })
        };
        let closed = {
            let (mornye, key) = (Arc::clone(self), key.to_string());
            Box::new(move || {
                let mut verifying = mornye.verifying.lock().unwrap();
                if verifying.as_deref() == Some(key.as_str()) {
                    *verifying = None;
                }
            })
        };
        shell(VerifyWindow {
            url,
            title: if title.is_empty() {
                "Verify".to_string()
            } else {
                format!("{title}: verify")
            },
            catches,
            caught,
            closed,
        })
    }

    /// Reads the repositories again when they are due, on a thread of its own.
    fn refresh_when_due(self: &Arc<Self>) {
        if self.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let mornye = Arc::clone(self);
        std::thread::spawn(move || {
            mornye.packages.refresh_due(REFRESH_EVERY);
            mornye.refreshing.store(false, Ordering::SeqCst);
        });
    }

    fn told(&self) -> Value {
        json!({
            "installed": self.packages.installed(),
            "repositories": self.packages.repositories(),
            "catalogue": self.packages.catalogue(),
            "refreshing": self.refreshing.load(Ordering::SeqCst),
            "verifying": *self.verifying.lock().unwrap(),
            "window": self.shell.get().is_some(),
            "ffmpeg": self.ffmpeg_told(),
        })
    }

    /// Whether FFmpeg is here, can be fetched, is being fetched, or could not be.
    pub(crate) fn ffmpeg_told(&self) -> Value {
        json!({
            "found": stash_mornye::ffmpeg::find(&self.tools).is_some(),
            "can_fetch": stash_mornye::ffmpeg::can_fetch(),
            "fetching": self.fetching.lock().unwrap().map(|(got, total)| json!({ "got": got, "total": total })),
            "trouble": *self.ffmpeg_trouble.lock().unwrap(),
        })
    }

    /// Fetches Stash's own FFmpeg on a thread of its own; false when it is being fetched already.
    fn fetch_ffmpeg(self: &Arc<Self>) -> bool {
        {
            let mut fetching = self.fetching.lock().unwrap();
            if fetching.is_some() {
                return false;
            }
            *fetching = Some((0, None));
        }
        *self.ffmpeg_trouble.lock().unwrap() = None;
        let mornye = Arc::clone(self);
        std::thread::spawn(move || {
            let told = |got: u64, total: Option<u64>| {
                *mornye.fetching.lock().unwrap() = Some((got, total))
            };
            match stash_mornye::ffmpeg::fetch(&mornye.tools, &told) {
                Ok(path) => mornye.packages.set_ffmpeg(Some(path)),
                Err(why) => *mornye.ffmpeg_trouble.lock().unwrap() = Some(why),
            }
            *mornye.fetching.lock().unwrap() = None;
        });
        true
    }

    fn access(&self, key: &str) -> Result<Value, String> {
        let mut status = self.packages.auth_status(key)?;
        status["access"] = serde_json::to_value(self.packages.access(key)?).unwrap_or(Value::Null);
        status["verifying"] = json!(self.verifying.lock().unwrap().as_deref() == Some(key));
        status["result"] = json!(self.verified.lock().unwrap().get(key));
        Ok(status)
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` within `total`.
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (a, b) = spec.split_once('-')?;
    let last = total.checked_sub(1)?;
    if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        return Some((total.saturating_sub(n), last));
    }
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() {
        last
    } else {
        b.parse::<u64>().ok()?.min(last)
    };
    (start <= end).then_some((start, end))
}

/// A growing file read from `at`, `left` bytes, waiting for them to come in.
struct GrowingReader {
    growing: Arc<Growing>,
    at: u64,
    left: u64,
}

impl Read for GrowingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let want = buf.len().min(self.left.min(256 * 1024) as usize);
        let n = self.growing.read_at(self.at, &mut buf[..want])?;
        self.at += n as u64;
        self.left -= n as u64;
        Ok(n)
    }
}

fn key_of(body: &Value) -> &str {
    body.get("key").and_then(Value::as_str).unwrap_or_default()
}

fn failed(status: u16, e: String) -> Reply {
    // Lokal's words for a package Stash will not run, said plainly.
    let e = match e.as_str() {
        "Invalid addon network permissions" => {
            "This extension does not name the sites it reaches (it asks for any \
                                                address), and Stash runs only extensions that do"
                .to_string()
        }
        _ => e,
    };
    refuse(status, &e)
}

// ---- an extension's catalogue: its albums, artists and playlists, as the window's pages

/// A song of a catalogue page, as a search gives the window one (`serve::found_track`).
fn page_song(found: MTrack, lossless: bool) -> Option<Value> {
    track(found, lossless).and_then(|t| crate::serve::found_track(&t))
}

fn page_album(album: &MAlbum) -> Value {
    json!({
        "id": album.id,
        "title": album.name,
        "artist": album.artists,
        "artist_id": album.artist_id,
        "year": album.year.as_deref().and_then(|y| y.parse::<u32>().ok()),
        "single": album.single(),
        "tracks": album.total_tracks,
        "art": album.artwork,
    })
}

fn page_artist(artist: &MArtist) -> Value {
    json!({ "id": artist.id, "name": artist.name, "art": artist.artwork })
}

/// `GET /api/mornye/find?key=&q=` (its artists and albums matching `q`), `/api/mornye/artist?key=&id=` (the artist,
/// their best known songs, all their albums and singles), `/api/mornye/album?key=&id=` and
/// `/api/mornye/playlist?key=&id=` (with their songs). The songs play from the extension (`mornye.<key>`).
fn catalogue_page(mornye: &Arc<Mornye>, path: &str, key: &str, id: &str, q: &str) -> Reply {
    let Some(addon) = mornye
        .packages
        .get(key)
        .filter(|_| mornye.packages.has_catalogue(key))
    else {
        return refuse(404, "that extension has no catalogue");
    };
    let who = kept_name(&addon.display_name);
    let lossless = lossless_of(&addon.quality_options);
    let songs = |tracks: Vec<MTrack>| -> Vec<Value> {
        tracks
            .into_iter()
            .filter_map(|t| page_song(t, lossless))
            .collect()
    };
    let answer: Result<Value, Trouble> = match path {
        "/api/mornye/find" => find_page(mornye, key, q.trim()),
        "/api/mornye/artist" => mornye
            .packages
            .page(key, Kind::Artist, id)
            .and_then(|page| {
                catalogue::artist_of(&page)
                    .ok_or_else(|| Trouble::Unavailable("That artist is not there".into()))
            })
            .map(|artist| {
                let (singles, albums): (Vec<&MAlbum>, Vec<&MAlbum>) =
                    artist.albums.iter().partition(|a| a.single());
                json!({
                    "id": artist.id,
                    "name": artist.name,
                    "art": artist.artwork,
                    "albums": albums.into_iter().map(page_album).collect::<Vec<_>>(),
                    "singles": singles.into_iter().map(page_album).collect::<Vec<_>>(),
                    "songs": songs(artist.top_tracks.clone()),
                })
            }),
        "/api/mornye/album" => mornye
            .packages
            .page(key, Kind::Album, id)
            .and_then(|page| {
                catalogue::album_of(&page)
                    .ok_or_else(|| Trouble::Unavailable("That album is not there".into()))
            })
            .map(|album| {
                let mut out = page_album(&album);
                out["songs"] = json!(songs(album.tracks));
                out
            }),
        _ => mornye
            .packages
            .page(key, Kind::Playlist, id)
            .and_then(|page| {
                catalogue::playlist_of(&page)
                    .ok_or_else(|| Trouble::Unavailable("That playlist is not there".into()))
            })
            .map(|playlist| {
                json!({
                    "id": playlist.id,
                    "title": playlist.name,
                    "author": playlist.owner,
                    "art": playlist.artwork,
                    "songs": songs(playlist.tracks),
                })
            }),
    };
    match answer {
        Ok(value) => reply_json(200, value),
        Err(trouble) => {
            let e = trouble_of(who, trouble);
            let status = match e.code {
                Code::NotFound => 404,
                Code::Unauthorized => 401,
                Code::RateLimited => 429,
                Code::Refused => 409,
                _ => 502,
            };
            refuse(status, &e.in_words())
        }
    }
}

/// An extension's artists, then its albums, matching `q`: one it cannot find still leaves the other.
fn find_page(mornye: &Arc<Mornye>, key: &str, q: &str) -> Result<Value, Trouble> {
    if q.is_empty() {
        return Ok(json!({ "artists": [], "albums": [] }));
    }
    let artists = mornye.packages.find(key, q, Kind::Artist, 8);
    let albums = mornye.packages.find(key, q, Kind::Album, 12);
    if let (Err(trouble), Err(_)) = (&artists, &albums) {
        return Err(trouble.clone());
    }
    let artists: Vec<Value> = artists
        .unwrap_or_default()
        .iter()
        .filter_map(catalogue::artist_of)
        .map(|a| page_artist(&a))
        .collect();
    let albums: Vec<Value> = albums
        .unwrap_or_default()
        .iter()
        .filter_map(catalogue::album_of)
        .map(|a| page_album(&a))
        .collect();
    Ok(json!({ "artists": artists, "albums": albums }))
}

/// The window's routes.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/mornye") {
        return Ok(None);
    }
    let Some(mornye) = state.mornye() else {
        return Ok(Some(refuse(
            404,
            "Mornye extensions are not part of this core",
        )));
    };
    let query = |name: &str| -> String {
        url::form_urlencoded::parse(
            request
                .url()
                .split_once('?')
                .map(|(_, q)| q)
                .unwrap_or("")
                .as_bytes(),
        )
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
    };
    if *request.method() == Method::Get {
        let reply = match path {
            "/api/mornye" => {
                mornye.refresh_when_due();
                reply_json(200, mornye.told())
            }
            "/api/mornye/settings" => {
                let key = query("key");
                match (mornye.packages.settings(&key), mornye.packages.get(&key)) {
                    (Ok(settings), Some(addon)) => reply_json(
                        200,
                        json!({
                            "settings": settings,
                            "schema": addon.settings,
                            "quality_options": addon.quality_options,
                            "global_actions": addon.global_actions,
                        }),
                    ),
                    (Err(e), _) => failed(404, e),
                    (_, None) => refuse(404, "that extension is not installed"),
                }
            }
            "/api/mornye/access" => match mornye.access(&query("key")) {
                Ok(status) => reply_json(200, status),
                Err(e) => failed(404, e),
            },
            "/api/mornye/find"
            | "/api/mornye/artist"
            | "/api/mornye/album"
            | "/api/mornye/playlist" => {
                if offline(state) {
                    return Ok(Some(refuse(
                        409,
                        "Stash is offline: the extension is not asked",
                    )));
                }
                catalogue_page(&mornye, path, &query("key"), &query("id"), &query("q"))
            }
            _ => return Ok(None),
        };
        return Ok(Some(reply));
    }
    if *request.method() != Method::Post {
        return Ok(None);
    }
    // A package file comes as itself, not as JSON.
    if path == "/api/mornye/install-file" {
        let mut bytes = Vec::new();
        request
            .as_reader()
            .take(33 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        let reply = match mornye.packages.install_file(&bytes) {
            Ok(installed) => {
                state.reload_addons()?;
                reply_json(200, json!({ "installed": installed }))
            }
            Err(e) => failed(400, e),
        };
        return Ok(Some(reply));
    }
    let body = body_json(request)?;
    let text = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let online = |what: &str| -> Option<Reply> {
        offline(state).then(|| refuse(409, &format!("Stash is offline: go online to {what}")))
    };
    let reply = match path {
        "/api/mornye/repositories" => {
            if let Some(r) = online("add a repository") {
                return Ok(Some(r));
            }
            match mornye.packages.add_repository(text("url").trim()) {
                Ok(_) => reply_json(200, mornye.told()),
                Err(e) => failed(400, e),
            }
        }
        "/api/mornye/repositories/remove" => match mornye.packages.remove_repository(&text("id")) {
            Ok(()) => reply_json(200, mornye.told()),
            Err(e) => failed(400, e),
        },
        "/api/mornye/refresh" => {
            if let Some(r) = online("read the repositories") {
                return Ok(Some(r));
            }
            let id = text("id");
            mornye
                .packages
                .refresh((!id.is_empty()).then_some(id.as_str()));
            reply_json(200, mornye.told())
        }
        "/api/mornye/install" => {
            if let Some(r) = online("install an extension") {
                return Ok(Some(r));
            }
            match mornye.packages.install(&text("repository"), &text("id")) {
                Ok(installed) => {
                    state.reload_addons()?;
                    reply_json(200, json!({ "installed": installed }))
                }
                Err(e) => failed(400, e),
            }
        }
        "/api/mornye/remove" => match mornye.packages.remove(key_of(&body)) {
            Ok(()) => {
                state.reload_addons()?;
                reply_json(200, mornye.told())
            }
            Err(e) => failed(400, e),
        },
        "/api/mornye/enabled" => {
            let on = body.get("on").and_then(Value::as_bool).unwrap_or(true);
            match mornye.packages.set_enabled(key_of(&body), on) {
                Ok(()) => {
                    state.reload_addons()?;
                    reply_json(200, mornye.told())
                }
                Err(e) => failed(400, e),
            }
        }
        "/api/mornye/settings" => {
            let values = body.get("values").cloned().unwrap_or_else(|| json!({}));
            match mornye.packages.set_settings(key_of(&body), &values) {
                Ok(settings) => {
                    // A song kept from before was downloaded with the old settings; a sign-in may be complete now.
                    mornye.forget_ready(key_of(&body));
                    state.reload_addons()?;
                    reply_json(200, json!({ "settings": settings }))
                }
                Err(e) => failed(400, e),
            }
        }
        "/api/mornye/verify" => {
            if let Some(r) = online("verify an extension") {
                return Ok(Some(r));
            }
            let key = key_of(&body).to_string();
            match mornye.packages.verify(&key) {
                Ok(None) => {
                    state.reload_addons()?;
                    reply_json(200, json!({ "ready": true }))
                }
                Ok(Some(url)) => {
                    let title = mornye
                        .packages
                        .get(&key)
                        .map(|a| a.display_name)
                        .unwrap_or_default();
                    let opened =
                        url.starts_with("https://") && mornye.open_window(&key, url.clone(), title);
                    reply_json(200, json!({ "ready": false, "window": opened, "url": url }))
                }
                Err(e) => failed(400, e),
            }
        }
        "/api/mornye/ffmpeg" => {
            if let Some(r) = online("fetch FFmpeg") {
                return Ok(Some(r));
            }
            if !stash_mornye::ffmpeg::can_fetch() {
                return Ok(Some(refuse(
                    404,
                    "Stash cannot fetch FFmpeg for this computer: install it yourself",
                )));
            }
            mornye.fetch_ffmpeg();
            reply_json(202, mornye.told())
        }
        "/api/mornye/callback" => match mornye.packages.callback(key_of(&body), text("url").trim())
        {
            Ok(said) => {
                state.reload_addons()?;
                reply_json(200, said)
            }
            Err(e) => failed(400, e),
        },
        "/api/mornye/logout" => match mornye.packages.logout(key_of(&body)) {
            Ok(()) => {
                mornye.forget_ready(key_of(&body));
                state.reload_addons()?;
                reply_json(200, json!({ "ok": true }))
            }
            Err(e) => failed(400, e),
        },
        "/api/mornye/action" => {
            let input = body.get("input").cloned().filter(|v| !v.is_null());
            let token = body.get("token").and_then(Value::as_str);
            match mornye
                .packages
                .invoke_action(key_of(&body), &text("action"), input, token)
            {
                Ok(said) => {
                    // A sign-in done by an action makes the extension a source.
                    state.reload_addons()?;
                    // An action may hand back a page to open (a login).
                    let page = said
                        .get("open_auth_url")
                        .and_then(Value::as_str)
                        .filter(|u| u.starts_with("https://"))
                        .map(str::to_string);
                    let mut said = said;
                    if let Some(url) = page {
                        let key = key_of(&body).to_string();
                        let title = mornye
                            .packages
                            .get(&key)
                            .map(|a| a.display_name)
                            .unwrap_or_default();
                        said["window"] = json!(mornye.open_window(&key, url, title));
                    }
                    reply_json(200, said)
                }
                Err(e) => failed(400, e),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(reply))
}

impl Mornye {
    /// Lets go of the songs kept downloaded from `key`.
    fn forget_ready(&self, key: &str) {
        let prefix = format!("{key}\n");
        let mut ready = self.ready.lock().unwrap();
        ready.retain(|r| {
            let keep = !r.song.starts_with(&prefix);
            if !keep {
                r.downloaded.cleanup();
            }
            keep
        });
    }

    /// Switches the add-on listed as `id` (`mornye.<key>`) on or off.
    pub(crate) fn set_enabled_by_id(&self, id: &str, on: bool) -> Result<(), String> {
        self.packages
            .set_enabled(id.strip_prefix(PREFIX).unwrap_or(id), on)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_read_within_the_file() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-200", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=50-10", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    #[test]
    fn a_file_s_tier_follows_what_it_is() {
        assert_eq!(
            tier_of("flac", Some(16), Some(44_100), None),
            Tier::Lossless
        );
        assert_eq!(tier_of("flac", Some(24), Some(96_000), None), Tier::Hires);
        assert_eq!(tier_of("aac", None, Some(44_100), Some(320)), Tier::High);
        assert_eq!(tier_of("opus", None, Some(48_000), Some(128)), Tier::Low);
        assert_eq!(codec_of("wavpack"), "flac");
        assert_eq!(codec_of("pcm_s24le"), "pcm");
    }
}
