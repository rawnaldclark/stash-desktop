//! One song downloaded by its add-on (spec §10; Lokal `MorneyePackages.download`, pkgs.dart:1885-2388): a runtime of
//! its own, `initialize`, the song's details, `checkAvailability` (with `enrichTrack` once), the quality, `download`
//! into a work folder, then what the file needs: opening it when it is encrypted (FFmpeg, when there is one; else the
//! key goes to the player), checking it is the song (its length within 12 s) and a sound the player takes.
//!
//! Dolby Atmos is not asked for here; post-processing hooks are not run (Stash keeps what it plays as it is).
//! A download that brings nothing new for 90 s is stopped (`The provider stopped delivering audio for 90 seconds.
//! Retry the track.`).

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::Trouble;
use crate::host::{Hooks, Transfer};
use crate::manifest::text_of;
use crate::packages::{
    CALL, DownloadRequest, Downloaded, Packages, Parts, Progress, Running, close, failure_text,
    invoke, sort_trouble,
};
use crate::session::rate_limit_message;
use crate::track::MTrack;

/// How long a download may bring nothing new.
const STALL: Duration = Duration::from_secs(90);
const STALLED: &str = "The provider stopped delivering audio for 90 seconds. Retry the track.";
const CANCELLED: &str = "Extension operation cancelled";
const AVAILABILITY: Duration = Duration::from_secs(25);
const ENRICHMENT: Duration = Duration::from_secs(20);
const DOWNLOAD: Duration = Duration::from_secs(24 * 60 * 60);
const RESOLUTION_MS: u64 = 60_000;
/// The YouTube Music extension's name.
const YOUTUBE_MUSIC: &str = "ytmusic-spotiflac";
/// How far the file's length may be from the song's.
const LENGTH_SLACK_MS: i64 = 12_000;
/// How often the watch looks at the download.
const TICK: Duration = Duration::from_millis(100);
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// The codecs the player is given (pkgs.dart:1857-1869).
const PLAYABLE: [&str; 11] = [
    "flac",
    "alac",
    "aac",
    "mp3",
    "opus",
    "vorbis",
    "wavpack",
    "pcm_s16le",
    "pcm_s24le",
    "pcm_s32le",
    "pcm_f32le",
];
/// Containers whose codec the probe may not tell, and the player opens all the same.
const LOOSE_FORMATS: [&str; 4] = ["isomp4", "ogg", "webm", "mkv"];
/// What an opened (decrypted) file may be.
const DECRYPTED: [&str; 5] = [".flac", ".m4a", ".mp4", ".ogg", ".opus"];

/// The quality to ask for (morneye_playback_quality.dart:28-100, without the Atmos branch): the one asked; else the
/// one set (`downloadQuality`) when it is an option; else among the options that are not spatial `HI_RES_LOSSLESS`,
/// then `LOSSLESS`, then the first `kind: lossless`, then the first; else `default`.
pub fn choose_quality(
    asked: Option<&str>,
    configured: Option<&str>,
    options: &[serde_json::Value],
) -> String {
    if let Some(asked) = asked.filter(|asked| !asked.is_empty()) {
        return asked.to_string();
    }
    if let Some(configured) = configured.filter(|configured| !configured.is_empty())
        && options
            .iter()
            .any(|option| option_id(option).as_deref() == Some(configured))
    {
        return configured.to_string();
    }
    let stereo = || {
        options
            .iter()
            .filter(|option| option.is_object() && !spatial(option))
    };
    let candidates: Vec<String> = stereo()
        .filter_map(option_id)
        .filter(|id| !id.is_empty())
        .collect();
    for tier in ["HI_RES_LOSSLESS", "LOSSLESS"] {
        if let Some(id) = candidates.iter().find(|id| id.to_uppercase() == tier) {
            return id.clone();
        }
    }
    if let Some(id) = stereo()
        .filter(|option| option.get("kind").and_then(Value::as_str) == Some("lossless"))
        .filter_map(option_id)
        .find(|id| !id.is_empty())
    {
        return id;
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| "default".to_string())
}

fn option_id(option: &Value) -> Option<String> {
    option
        .get("id")
        .filter(|id| !id.is_null())
        .map(|id| text_of(Some(id)))
}

/// `^(?:DOLBY[_ -]?ATMOS|ATMOS|AC-?4)$`, any case.
fn atmos_name(text: &str) -> bool {
    matches!(
        text.trim().to_uppercase().as_str(),
        "ATMOS" | "AC4" | "AC-4" | "DOLBYATMOS" | "DOLBY_ATMOS" | "DOLBY ATMOS" | "DOLBY-ATMOS"
    )
}

/// A spatial option: `kind: spatial`, or named for Atmos or AC-4.
fn spatial(option: &Value) -> bool {
    let named = |key: &str| {
        option
            .get(key)
            .filter(|v| !v.is_null())
            .is_some_and(|v| atmos_name(&text_of(Some(v))))
    };
    option
        .get("kind")
        .is_some_and(|kind| text_of(Some(kind)).to_lowercase() == "spatial")
        || named("id")
        || named("name")
        || named("label")
}

/// Whether `chosen` asks for spatial audio.
fn spatial_choice(chosen: &str, options: &[Value]) -> bool {
    atmos_name(chosen)
        || options
            .iter()
            .any(|option| option_id(option).as_deref() == Some(chosen) && spatial(option))
}

pub(crate) fn download(
    packages: &Packages,
    key: &str,
    track_id: &str,
    request: DownloadRequest,
) -> Result<Downloaded, Trouble> {
    let parts = packages.parts(key)?;
    if let Some(seconds) = parts
        .session
        .as_ref()
        .and_then(|session| session.rate_limited_for())
    {
        return Err(Trouble::RateLimited {
            seconds,
            message: rate_limit_message(seconds),
        });
    }
    if request.cancel.load(Ordering::SeqCst) {
        return Err(Trouble::Failed(CANCELLED.into()));
    }
    let work = packages.work_folder()?;
    let result = run(packages, key, track_id, &request, &parts, &work);
    if result.is_err() {
        let _ = fs::remove_dir_all(&work);
    }
    result
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Tells the window how far the download is, one whole [`Progress`] at a time.
struct Reporter {
    current: Mutex<Progress>,
    sink: Option<Arc<dyn Fn(&Progress) + Send + Sync>>,
}

impl Reporter {
    fn update(&self, change: impl FnOnce(&mut Progress)) {
        let Some(sink) = &self.sink else {
            return;
        };
        let now = {
            let mut current = lock(&self.current);
            change(&mut current);
            current.clone()
        };
        sink(&now);
    }

    fn stage(&self, stage: &str) {
        self.update(|progress| progress.stage = Some(stage.to_string()));
    }

    fn percent(&self, percent: f64) {
        self.update(|progress| progress.percent = Some(percent));
    }

    fn message(&self, message: &str) {
        self.update(|progress| progress.message = Some(message.to_string()));
    }
}

/// Whether the download still brings something (pkgs.dart:1924-1942): a higher percent than the last one, or more
/// bytes (or bytes of a new attempt) reset its clock, while `download` runs.
struct Watch {
    watching: AtomicBool,
    stalled: AtomicBool,
    last: Mutex<Instant>,
    last_percent: Mutex<f64>,
    seen: Mutex<(Option<PathBuf>, u64)>,
    first_byte: AtomicBool,
}

impl Watch {
    fn new() -> Watch {
        Watch {
            watching: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
            last: Mutex::new(Instant::now()),
            last_percent: Mutex::new(-1.0),
            seen: Mutex::new((None, 0)),
            first_byte: AtomicBool::new(false),
        }
    }

    fn moved(&self) {
        if self.watching.load(Ordering::SeqCst) {
            *lock(&self.last) = Instant::now();
        }
    }

    fn begin(&self) {
        *lock(&self.last) = Instant::now();
        self.watching.store(true, Ordering::SeqCst);
    }

    fn end(&self) {
        self.watching.store(false, Ordering::SeqCst);
    }

    /// A percent can start again from 0 when a part is retried: it is compared with the last one, not the highest.
    fn percent(&self, percent: f64) {
        let higher = {
            let mut last = lock(&self.last_percent);
            let higher = percent > *last;
            *last = percent;
            higher
        };
        if higher {
            self.moved();
        }
    }

    /// Whether this step brought the first byte.
    fn transfer(&self, transfer: &Transfer) -> bool {
        let moved = {
            let mut seen = lock(&self.seen);
            let moved = transfer.written > 0
                && (seen.0.as_deref() != Some(transfer.part.as_path())
                    || transfer.written > seen.1);
            *seen = (Some(transfer.part.clone()), transfer.written);
            moved
        };
        if moved {
            self.moved();
        }
        transfer.written > 0 && !self.first_byte.swap(true, Ordering::SeqCst)
    }

    fn expired(&self) -> bool {
        self.watching.load(Ordering::SeqCst) && lock(&self.last).elapsed() >= STALL
    }
}

/// Closes the runtime when the download is cancelled or stalls; ends when `done` says the download is over.
fn guard(done: mpsc::Receiver<()>, watch: &Watch, running: &Running, cancel: &AtomicBool) {
    loop {
        match done.recv_timeout(TICK) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        if cancel.load(Ordering::SeqCst) {
            close(running);
            return;
        }
        if watch.expired() {
            watch.stalled.store(true, Ordering::SeqCst);
            close(running);
            return;
        }
    }
}

fn run(
    packages: &Packages,
    key: &str,
    track_id: &str,
    request: &DownloadRequest,
    parts: &Parts,
    work: &Path,
) -> Result<Downloaded, Trouble> {
    let reporter = Arc::new(Reporter {
        current: Mutex::new(Progress::default()),
        sink: request.progress.clone(),
    });
    let watch = Arc::new(Watch::new());
    let progress: Arc<dyn Fn(f64) + Send + Sync> = {
        let (watch, reporter) = (Arc::clone(&watch), Arc::clone(&reporter));
        Arc::new(move |percent: f64| {
            watch.percent(percent);
            reporter.percent(percent);
        })
    };
    let status: Arc<dyn Fn(&str) + Send + Sync> = {
        let reporter = Arc::clone(&reporter);
        Arc::new(move |text: &str| reporter.message(text))
    };
    let transfer: Arc<dyn Fn(&Transfer) + Send + Sync> = {
        let (watch, reporter, forward) = (
            Arc::clone(&watch),
            Arc::clone(&reporter),
            request.transfer.clone(),
        );
        Arc::new(move |transfer: &Transfer| {
            if watch.transfer(transfer) {
                reporter.stage("first_byte");
            }
            if let Some(forward) = &forward {
                forward(transfer);
            }
        })
    };
    let hooks = Hooks {
        progress: Some(progress),
        status: Some(status),
        transfer: Some(transfer),
    };
    let running = packages.download_runtime(key, work, hooks)?;
    let outcome = std::thread::scope(|scope| {
        let (done, wait) = mpsc::channel::<()>();
        let (watching, runtime, cancel) = (&*watch, &running, &*request.cancel);
        scope.spawn(move || guard(wait, watching, runtime, cancel));
        let outcome = resolve(
            packages, key, track_id, request, parts, &running, work, &watch, &reporter,
        );
        drop(done);
        outcome
    });
    close(&running);
    let (track, result, chosen) = match outcome {
        Ok(found) => found,
        Err(_) if watch.stalled.load(Ordering::SeqCst) => {
            return Err(Trouble::Failed(STALLED.into()));
        }
        Err(_) if request.cancel.load(Ordering::SeqCst) => {
            return Err(Trouble::Failed(CANCELLED.into()));
        }
        Err(trouble) => return Err(trouble),
    };
    finish(
        packages, key, request, work, &reporter, track, result, chosen,
    )
}

/// The add-on's part: `initialize`, the song, its availability, the quality, `download` (pkgs.dart:2001-2167).
#[allow(clippy::too_many_arguments)]
fn resolve(
    packages: &Packages,
    key: &str,
    track_id: &str,
    request: &DownloadRequest,
    parts: &Parts,
    running: &Running,
    work: &Path,
    watch: &Watch,
    reporter: &Reporter,
) -> Result<(Map<String, Value>, Map<String, Value>, String), Trouble> {
    let manifest = &parts.manifest;
    let session = parts.session.as_deref();
    let engine = &running.engine;
    let sort = |trouble: Trouble, unavailable: bool| {
        sort_trouble(
            trouble,
            &manifest.display_name,
            &parts.network,
            session,
            unavailable,
        )
    };
    let call = |method: &str, args: &[Value], timeout: Duration| {
        invoke(running, session, method, args, timeout).map_err(|trouble| sort(trouble, false))
    };

    reporter.stage("initializing");
    call("initialize", std::slice::from_ref(&parts.settings), CALL)?;

    reporter.stage("metadata");
    let mut track: Map<String, Value> = packages
        .track(key, track_id)
        .and_then(|known| known.raw.as_object().cloned())
        .unwrap_or_default();
    if track.is_empty()
        && engine.has("checkAvailability")
        && let Some(Value::Object(hint)) = &request.hint
        && usable_hint(hint, track_id)
    {
        // The song as the search showed it saves a lookup; availability is still asked.
        track = hint.clone();
    }
    if track.is_empty()
        && engine.has("getTrack")
        && let Value::Object(found) = call("getTrack", &[json!(track_id)], CALL)?
    {
        if let Some(known) =
            MTrack::of(&Value::Object(found.clone()), false).filter(|known| known.id == track_id)
        {
            packages.remember(key, &[known]);
        }
        track = found;
    }
    if track.is_empty() {
        track = stub(track_id, &manifest.name);
    }

    let provider = manifest.provider().to_string();
    let check = |track: &Map<String, Value>| -> Result<Value, Trouble> {
        if !engine.has("checkAvailability") {
            return Ok(json!({ "available": true, "track_id": track_id }));
        }
        let field = |name: &str| {
            track
                .get(name)
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or_else(|| json!(""))
        };
        let options = availability_options(&provider, track, track_id);
        call(
            "checkAvailability",
            &[field("isrc"), field("name"), field("artists"), options],
            AVAILABILITY,
        )
    };
    reporter.stage("availability");
    let mut available = check(&track)?;
    if !is_available(&available) && engine.has("enrichTrack") {
        reporter.stage("cross_provider_enrichment");
        if let Value::Object(enriched) =
            call("enrichTrack", &[Value::Object(track.clone())], ENRICHMENT)?
        {
            enrich(&mut track, enriched);
        }
        reporter.stage("availability_retry");
        available = check(&track)?;
    }
    if !is_available(&available) {
        let reason = available
            .get("reason")
            .filter(|reason| !reason.is_null())
            .map(|reason| text_of(Some(reason)))
            .unwrap_or_else(|| "The addon cannot download this track".to_string());
        return Err(sort(Trouble::Unavailable(reason), true));
    }

    let mut prepared = available
        .get("prepared_context")
        .filter(|context| !context.is_null())
        .or_else(|| available.get("preparedContext"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    prepared.insert("host_track".into(), Value::Object(track.clone()));
    if provider == "amazon" || provider == "amazon-new" {
        prepared.insert("web_metadata".into(), Value::Object(track.clone()));
    }

    let options = &manifest.quality_options;
    let configured = parts
        .settings
        .get("downloadQuality")
        .and_then(Value::as_str);
    let mut chosen = choose_quality(request.quality.as_deref(), configured, options);
    if spatial_choice(&chosen, options) {
        // Atmos needs its own checks of the delivered file (spec §10.9): the best stereo option instead.
        chosen = choose_quality(None, None, options);
    }

    reporter.stage("preparing_media");
    let id = available
        .get("track_id")
        .filter(|id| !id.is_null())
        .or_else(|| available.get("trackId").filter(|id| !id.is_null()))
        .map(|id| text_of(Some(id)))
        .unwrap_or_else(|| track_id.to_string());
    let output = work.join("audio.flac").to_string_lossy().into_owned();
    watch.begin();
    let result = call(
        "download",
        &[
            json!(id),
            json!(chosen),
            json!(output),
            json!({ "preparedContext": prepared, "resolutionTimeoutMs": RESOLUTION_MS }),
        ],
        DOWNLOAD,
    );
    watch.end();
    let result = result?;
    reporter.stage("download_complete");
    match result {
        Value::Object(result) if result.get("success") == Some(&Value::Bool(true)) => {
            Ok((track, result, chosen))
        }
        other => Err(sort(
            Trouble::Failed(failure_text(&other, "Addon download failed")),
            false,
        )),
    }
}

/// The file's part: where it is, opening it, what it is, its name (pkgs.dart:2168-2379).
#[allow(clippy::too_many_arguments)]
fn finish(
    packages: &Packages,
    key: &str,
    request: &DownloadRequest,
    work: &Path,
    reporter: &Reporter,
    track: Map<String, Value>,
    result: Map<String, Value>,
    chosen: String,
) -> Result<Downloaded, Trouble> {
    let failed = |message: &str| Trouble::Failed(message.to_string());
    let said = ["file_path", "filePath", "path"]
        .iter()
        .find_map(|name| result.get(*name).filter(|v| !v.is_null()));
    let mut file = sandboxed(
        &text_of(said),
        &packages.files_root(key),
        &[work.to_path_buf()],
    )
    .map_err(Trouble::Failed)?;
    if !non_empty(&file) {
        return Err(failed("Addon produced an empty file"));
    }

    let mut decryption_key = None;
    if let Some((key_hex, extension)) = decryption(&result)? {
        reporter.stage("decrypting_media");
        match packages.ffmpeg() {
            Some(ffmpeg) => {
                reporter.message("Decrypting audio\u{2026}");
                let target = work.join(format!("decrypted{extension}"));
                convert(&ffmpeg, &file, &target, &key_hex, &request.cancel)
                    .map_err(Trouble::Failed)?;
                file = target;
            }
            // The player opens it with the key.
            None => decryption_key = Some(key_hex),
        }
    }

    reporter.stage("verifying_media");
    let info = match crate::probe::probe(&file) {
        Ok(info) => info,
        // An `ffmpeg.mov_key` file is always an MP4 (the mov demuxer's key), whatever its name.
        Err(_) if decryption_key.is_some() => {
            json!({ "success": true, "codec": "unknown", "format": "isomp4" })
        }
        Err(error) => return Err(Trouble::Failed(error)),
    };
    let expected = track
        .get("duration_ms")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64;
    let actual = info
        .get("duration_ms")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64;
    if expected > 0 && actual > 0 && (actual - expected).abs() > LENGTH_SLACK_MS {
        return Err(failed("Downloaded audio does not match the track duration"));
    }
    let codec = text_of(info.get("codec"));
    let format = text_of(info.get("format")).to_ascii_lowercase();
    if !playable_codec(&codec, &format) {
        return Err(Trouble::Failed(format!(
            "Stash can\u{2019}t play {codec} audio"
        )));
    }

    // The player goes by the name: the container's extension, and the file in the work folder. One already in the
    // work folder keeps its old name too, since the player may be reading it live by that name.
    let wanted = crate::probe::extension_of(&format)
        .map(str::to_string)
        .or_else(|| extension_lowercase(&file));
    let named_right = wanted.is_none() || extension_lowercase(&file) == wanted;
    let in_work = file.starts_with(work);
    if !named_right || !in_work {
        let renamed = work.join(format!("playable.{}", wanted.as_deref().unwrap_or("audio")));
        let kept = if in_work {
            link_file(&file, &renamed)
        } else {
            move_file(&file, &renamed)
        };
        kept.map_err(Trouble::Failed)?;
        file = renamed;
    }
    reporter.stage("ready");
    let mut metadata = track;
    metadata.extend(result);
    Ok(Downloaded {
        file,
        work: work.to_path_buf(),
        metadata: Value::Object(metadata),
        info,
        decryption_key,
        quality: chosen,
    })
}

fn playable_codec(codec: &str, format: &str) -> bool {
    PLAYABLE.contains(&codec) || (codec == "unknown" && LOOSE_FORMATS.contains(&format))
}

fn is_available(available: &Value) -> bool {
    available.get("available") == Some(&Value::Bool(true))
}

/// A search's song stands in for `getTrack` only when it is this song and names it and its artists.
fn usable_hint(hint: &Map<String, Value>, track_id: &str) -> bool {
    let blank = |name: &str| text_of(hint.get(name)).trim().is_empty();
    !track_id.trim().is_empty()
        && text_of(hint.get("id")) == track_id
        && !blank("name")
        && !blank("artists")
}

/// What is known of a song nothing describes.
fn stub(track_id: &str, name: &str) -> Map<String, Value> {
    let mut track = Map::new();
    track.insert("id".into(), json!(track_id));
    track.insert("name".into(), json!(""));
    track.insert("artists".into(), json!(""));
    track.insert("duration_ms".into(), json!(0));
    track.insert("provider_id".into(), json!(name));
    track
}

/// What `enrichTrack` found, over the song: only values that say something.
fn enrich(track: &mut Map<String, Value>, enriched: Map<String, Value>) {
    for (field, value) in enriched {
        let empty = match &value {
            Value::Null => true,
            Value::String(text) => text.trim().is_empty(),
            Value::Number(n) => n.as_f64() == Some(0.0),
            Value::Array(items) => items.is_empty(),
            _ => false,
        };
        if !empty {
            track.insert(field, value);
        }
    }
}

/// `checkAvailability`'s options (pkgs.dart:87-123): the song, its length, the catalogue ids it carries, and the
/// add-on's own id for it when the song is the add-on's.
fn availability_options(provider: &str, track: &Map<String, Value>, original_id: &str) -> Value {
    let mut options = Map::new();
    options.insert("track".into(), Value::Object(track.clone()));
    options.insert(
        "duration_ms".into(),
        track
            .get("duration_ms")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| json!(0)),
    );
    for field in ["spotify_id", "deezer_id", "qobuz_id", "tidal_id"] {
        let text = text_of(track.get(field));
        if !text.trim().is_empty() {
            options.insert(field.into(), Value::String(text));
        }
    }
    let owner_text = text_of(track.get("provider_id"));
    let owner = owner_text.strip_suffix("-web").unwrap_or(&owner_text);
    let current = track
        .get("id")
        .filter(|id| !id.is_null())
        .map(|id| text_of(Some(id)))
        .unwrap_or_else(|| original_id.to_string());
    let owned =
        owner.is_empty() || owner == provider || (provider == "amazon-new" && owner == "amazon");
    let asin = current.len() == 10
        && current
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    let deezer_number = (1..=20).contains(&current.len())
        && current.bytes().all(|b| b.is_ascii_digit())
        && !current.starts_with('0');
    if provider.starts_with("amazon") && owned && asin {
        // Amazon's documented field for its own ASIN.
        options.insert("spotify_id".into(), Value::String(current));
    } else if provider == "deezer" {
        if owned && deezer_number {
            options.insert("deezer_id".into(), Value::String(current));
        }
    } else if owned && matches!(provider, "qobuz" | "tidal" | "soundcloud" | YOUTUBE_MUSIC) {
        // SpotiFLAC's field for a provider's own id is `spotify_id`. YouTube Music's own song goes by it too (not in
        // Lokal): the extension takes its video id there without searching, while its search for a song never
        // matches one (its rows say `artist` and `duration`, its check reads `artists` and `duration_ms`).
        let field = if matches!(provider, "soundcloud" | YOUTUBE_MUSIC) {
            "spotify_id".to_string()
        } else {
            format!("{provider}_id")
        };
        options.entry(field).or_insert(Value::String(current));
    }
    Value::Object(options)
}

/// The key (32 hex, lowercased) and the extension an encrypted result opens to (pkgs.dart:2203-2239): `decryption`,
/// or `decryption_key` (an `ffmpeg.mov_key`).
fn decryption(result: &Map<String, Value>) -> Result<Option<(String, String)>, Trouble> {
    let unsupported = || Trouble::Failed("Unsupported audio decryption descriptor".into());
    let invalid = || Trouble::Failed("Invalid decrypted audio container".into());
    let descriptor = match result.get("decryption").filter(|v| !v.is_null()) {
        Some(descriptor) => descriptor.clone(),
        None => match result.get("decryption_key").filter(|v| !v.is_null()) {
            Some(key) if !text_of(Some(key)).trim().is_empty() => {
                json!({ "strategy": "ffmpeg.mov_key", "key": key })
            }
            _ => return Ok(None),
        },
    };
    let Value::Object(descriptor) = &descriptor else {
        return Err(unsupported());
    };
    let key = text_of(descriptor.get("key"));
    if descriptor.get("strategy").and_then(Value::as_str) != Some("ffmpeg.mov_key")
        || key.len() != 32
        || !key.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(unsupported());
    }
    // Amazon sends "" for FLAC: an empty one says nothing.
    let mut extension = ".flac".to_string();
    for value in [
        result.get("output_extension"),
        descriptor.get("output_extension"),
    ] {
        match value {
            None | Some(Value::Null) => continue,
            Some(Value::String(text)) if text.trim().is_empty() => continue,
            Some(Value::String(text)) => {
                let text = text.trim().to_lowercase();
                extension = if text.starts_with('.') {
                    text
                } else {
                    format!(".{text}")
                };
                break;
            }
            Some(_) => return Err(invalid()),
        }
    }
    if !DECRYPTED.contains(&extension.as_str()) {
        return Err(invalid());
    }
    Ok(Some((key.to_ascii_lowercase(), extension)))
}

/// Opens an encrypted file with FFmpeg (morneye_ffmpeg.dart): the audio stream copied, or, when a FLAC stream will
/// not copy into its container, encoded to FLAC again (still lossless).
fn convert(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    key: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let flac = extension_lowercase(output).as_deref() == Some("flac");
    match run_ffmpeg(ffmpeg, input, output, "copy", key, cancel)? {
        None => Ok(()),
        Some(_) if flac => match run_ffmpeg(ffmpeg, input, output, "flac", key, cancel)? {
            None => Ok(()),
            Some(said) => Err(ffmpeg_failure(&said)),
        },
        Some(said) => Err(ffmpeg_failure(&said)),
    }
}

fn ffmpeg_failure(said: &str) -> String {
    if said.is_empty() {
        "FFmpeg audio conversion failed.".to_string()
    } else {
        format!("FFmpeg audio conversion failed. {said}")
    }
}

/// One FFmpeg run: `Ok(None)` when it made the file, `Ok(Some(what it said))` when it failed (the key blanked out),
/// `Err` when it could not run, was cancelled, or took more than 3 minutes.
fn run_ffmpeg(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    codec: &str,
    key: &str,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    if !non_empty(input) {
        return Err("The audio conversion input is missing or empty.".into());
    }
    let _ = fs::remove_file(output);
    let mut command = Command::new(ffmpeg);
    command
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-decryption_key",
            key,
            "-i",
        ])
        .arg(input)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-map_metadata",
            "0",
            "-c:a",
            codec,
        ])
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("FFmpeg could not start: {error}"))?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        let stop = if cancel.load(Ordering::SeqCst) {
            Some("Audio processing cancelled.")
        } else if started.elapsed() > FFMPEG_TIMEOUT {
            Some("Audio processing timed out. Retry the track.")
        } else {
            None
        };
        if let Some(stop) = stop {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(output);
            return Err(stop.into());
        }
        std::thread::sleep(TICK);
    };
    let said = reader
        .join()
        .unwrap_or_default()
        .trim()
        .replace(key, "[redacted]");
    if !status.success() {
        let _ = fs::remove_file(output);
        let tail: String = {
            let chars: Vec<char> = said.chars().collect();
            chars[chars.len().saturating_sub(500)..].iter().collect()
        };
        return Ok(Some(tail));
    }
    if !non_empty(output) {
        return Err("FFmpeg completed without producing audio.".into());
    }
    Ok(None)
}

/// The path an add-on named, as its host would let it reach it (host.dart:383-415): a relative one inside `root`
/// (never through `..`), an absolute one inside `root` or a granted folder, and no link or special file on the way.
fn sandboxed(raw: &str, root: &Path, grants: &[PathBuf]) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err("Invalid addon file path".into());
    }
    let given = Path::new(raw);
    let path = if given.is_absolute() {
        given.to_path_buf()
    } else {
        if raw.split(['/', '\\']).any(|part| part == "..") {
            return Err("Invalid addon file path".into());
        }
        root.join(given)
    };
    let path = normalized(&path);
    let base = std::iter::once(root)
        .chain(grants.iter().map(PathBuf::as_path))
        .map(normalized)
        .find(|base| path.starts_with(base))
        .ok_or_else(|| "File path is outside the addon sandbox".to_string())?;
    let mut walked = base.clone();
    for component in path.components().skip(base.components().count()) {
        walked.push(component);
        if let Ok(meta) = fs::symlink_metadata(&walked)
            && (meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()))
        {
            return Err("Addon paths cannot contain links or special files".into());
        }
    }
    Ok(path)
}

/// `.` and `..` resolved by name alone.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn non_empty(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

fn extension_lowercase(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
}

/// Gives a file a second name (a hard link, else a copy); the first stays.
fn link_file(from: &Path, to: &Path) -> Result<(), String> {
    let _ = fs::remove_file(to);
    if fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|error| format!("Cannot keep the downloaded audio: {error}"))
}

/// Moves a file (copied, then removed, when a rename cannot).
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map_err(|error| format!("Cannot keep the downloaded audio: {error}"))?;
    let _ = fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tidal() -> Vec<Value> {
        vec![
            json!({ "id": "DOLBY_ATMOS", "label": "Dolby Atmos", "kind": "spatial" }),
            json!({ "id": "HI_RES_LOSSLESS", "label": "HiRes FLAC", "kind": "lossless" }),
            json!({ "id": "LOSSLESS", "label": "Lossless", "kind": "lossless" }),
            json!({ "id": "HIGH", "kind": "lossy" }),
        ]
    }

    #[test]
    fn the_quality_asked_or_set_wins() {
        assert_eq!(
            choose_quality(Some("HIGH"), Some("LOSSLESS"), &tidal()),
            "HIGH"
        );
        assert_eq!(choose_quality(Some("anything"), None, &[]), "anything");
        assert_eq!(choose_quality(None, Some("LOSSLESS"), &tidal()), "LOSSLESS");
        // One set that is not an option is passed over.
        assert_eq!(
            choose_quality(None, Some("MAX"), &tidal()),
            "HI_RES_LOSSLESS"
        );
        assert_eq!(
            choose_quality(Some(""), Some(""), &tidal()),
            "HI_RES_LOSSLESS"
        );
    }

    #[test]
    fn the_best_stereo_option_is_chosen() {
        let amazon = vec![
            json!({ "id": "best", "kind": "lossless" }),
            json!({ "id": "opus", "kind": "lossy" }),
            json!({ "id": "eac3", "kind": "spatial" }),
            json!({ "id": "ac4", "label": "Dolby Atmos (AC-4)", "kind": "spatial" }),
        ];
        assert_eq!(choose_quality(None, None, &amazon), "best");
        let lossy = vec![
            json!({ "id": "AAC_320", "kind": "lossy" }),
            json!({ "id": "AAC_160", "kind": "lossy" }),
        ];
        assert_eq!(choose_quality(None, None, &lossy), "AAC_320");
        assert_eq!(
            choose_quality(None, None, &[json!({ "id": "flac" })]),
            "flac"
        );
        assert_eq!(
            choose_quality(None, None, &[json!({ "id": "lossless" })]),
            "lossless"
        );
        assert_eq!(
            choose_quality(None, None, &[json!({ "id": "AC-4" })]),
            "default"
        );
        assert_eq!(
            choose_quality(None, None, &[json!({ "id": "x", "name": "Dolby Atmos" })]),
            "default"
        );
        assert_eq!(choose_quality(None, None, &[]), "default");
        assert_eq!(
            choose_quality(None, None, &[json!("HI_RES_LOSSLESS"), json!({ "id": 7 })]),
            "7"
        );
    }

    #[test]
    fn spatial_choices_are_told() {
        assert!(spatial_choice("DOLBY_ATMOS", &tidal()));
        assert!(spatial_choice("dolby atmos", &[]));
        assert!(spatial_choice(
            "eac3",
            &[json!({ "id": "eac3", "kind": "Spatial" })]
        ));
        assert!(!spatial_choice("HI_RES_LOSSLESS", &tidal()));
    }

    #[test]
    fn availability_options_carry_the_add_ons_own_id() {
        let track = |extra: Value| -> Map<String, Value> {
            let mut track = json!({ "id": "B000ABCDEF", "name": "s", "duration_ms": 1000, "tidal_id": 55, "qobuz_id": " " });
            if let (Value::Object(track), Value::Object(extra)) = (&mut track, extra) {
                track.extend(extra);
            }
            track.as_object().cloned().unwrap()
        };
        let amazon =
            availability_options("amazon", &track(json!({ "provider_id": "amazon" })), "x");
        assert_eq!(amazon["spotify_id"], "B000ABCDEF");
        assert_eq!(amazon["tidal_id"], "55");
        assert!(amazon.get("qobuz_id").is_none());
        assert_eq!(amazon["duration_ms"], 1000);
        assert_eq!(amazon["track"]["name"], "s");

        // Another catalogue's song keeps only what it carries.
        let foreign =
            availability_options("amazon", &track(json!({ "provider_id": "deezer" })), "x");
        assert!(foreign.get("spotify_id").is_none());

        let deezer = json!({ "id": 3135556, "provider_id": "deezer" });
        let deezer = availability_options("deezer", deezer.as_object().unwrap(), "x");
        assert_eq!(deezer["deezer_id"], "3135556");
        assert_eq!(deezer["duration_ms"], 0);

        let qobuz = json!({ "provider_id": "qobuz-web", "qobuz_id": "kept" });
        let qobuz = availability_options("qobuz", qobuz.as_object().unwrap(), "orig");
        assert_eq!(qobuz["qobuz_id"], "kept");
        let tidal = availability_options("tidal", &Map::new(), "orig");
        assert_eq!(tidal["tidal_id"], "orig");
        let soundcloud = availability_options("soundcloud", &Map::new(), "sc1");
        assert_eq!(soundcloud["spotify_id"], "sc1");
        let own = json!({ "id": "dQw4w9WgXcQ", "provider_id": YOUTUBE_MUSIC });
        let youtube = availability_options(YOUTUBE_MUSIC, own.as_object().unwrap(), "x");
        assert_eq!(youtube["spotify_id"], "dQw4w9WgXcQ");
        let foreign = json!({ "id": "123", "provider_id": "deezer" });
        let youtube = availability_options(YOUTUBE_MUSIC, foreign.as_object().unwrap(), "x");
        assert!(youtube.get("spotify_id").is_none());
    }

    #[test]
    fn hints_and_enrichment() {
        let hint = json!({ "id": "1", "name": "Song", "artists": "A" });
        assert!(usable_hint(hint.as_object().unwrap(), "1"));
        assert!(!usable_hint(hint.as_object().unwrap(), "2"));
        let nameless = json!({ "id": "1", "name": " ", "artists": "A" });
        assert!(!usable_hint(nameless.as_object().unwrap(), "1"));

        let mut track = stub("1", "demo");
        let found = json!({ "name": "Found", "isrc": "", "duration_ms": 0, "genres": [], "artists": "B", "explicit": false });
        enrich(&mut track, found.as_object().cloned().unwrap());
        assert_eq!(track["name"], "Found");
        assert_eq!(track["artists"], "B");
        assert_eq!(track["explicit"], false);
        assert!(track.get("isrc").is_none() && track.get("genres").is_none());
        assert_eq!(track["provider_id"], "demo");
    }

    fn result(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    /// A made-up value, for the tests only.
    const SAMPLE: &str = "00112233445566778899AABBCCDDEEFF";

    #[test]
    fn decryption_descriptors_are_checked() {
        assert_eq!(
            decryption(&result(json!({ "success": true }))).unwrap(),
            None
        );
        assert_eq!(
            decryption(&result(json!({ "decryption_key": "  " }))).unwrap(),
            None
        );
        assert_eq!(
            decryption(&result(
                json!({ "decryption_key": SAMPLE, "output_extension": "" })
            ))
            .unwrap(),
            Some((SAMPLE.to_ascii_lowercase(), ".flac".to_string()))
        );
        assert_eq!(
            decryption(&result(
                json!({ "decryption_key": SAMPLE, "output_extension": "M4A" })
            ))
            .unwrap(),
            Some((SAMPLE.to_ascii_lowercase(), ".m4a".to_string()))
        );
        let described = json!({ "decryption": { "strategy": "ffmpeg.mov_key", "key": SAMPLE, "output_extension": ".opus" } });
        assert_eq!(decryption(&result(described)).unwrap().unwrap().1, ".opus");
        let unsupported = "Unsupported audio decryption descriptor";
        for bad in [
            json!({ "decryption_key": "abc" }),
            json!({ "decryption": { "strategy": "aes", "key": SAMPLE } }),
            json!({ "decryption": "yes" }),
        ] {
            assert_eq!(
                decryption(&result(bad)).unwrap_err(),
                Trouble::Failed(unsupported.into())
            );
        }
        let container = Trouble::Failed("Invalid decrypted audio container".into());
        assert_eq!(
            decryption(&result(
                json!({ "decryption_key": SAMPLE, "output_extension": ".exe" })
            ))
            .unwrap_err(),
            container
        );
        assert_eq!(
            decryption(&result(
                json!({ "decryption_key": SAMPLE, "output_extension": 4 })
            ))
            .unwrap_err(),
            container
        );
    }

    #[test]
    fn codecs_the_player_takes() {
        assert!(playable_codec("flac", "flac"));
        assert!(playable_codec("aac", "isomp4"));
        assert!(playable_codec("unknown", "isomp4"));
        assert!(playable_codec("unknown", "webm"));
        assert!(!playable_codec("unknown", "wave"));
        assert!(!playable_codec("eac3", "isomp4"));
    }

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stash-mornye-download-{}",
            crate::hex(&crate::random_bytes(6))
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn paths_stay_in_the_sandbox() {
        let base = folder();
        let root = base.join("files");
        let work = base.join("work");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&work).unwrap();
        let grants = [work.clone()];
        let inside = work.join("audio.flac");
        assert_eq!(
            sandboxed(&inside.to_string_lossy(), &root, &grants).unwrap(),
            normalized(&inside)
        );
        assert_eq!(
            sandboxed("songs/a.flac", &root, &grants).unwrap(),
            normalized(&root.join("songs/a.flac"))
        );
        let sneaky = work.join("..").join("files").join("x.flac");
        assert_eq!(
            sandboxed(&sneaky.to_string_lossy(), &root, &grants).unwrap(),
            normalized(&root.join("x.flac"))
        );
        let outside = base.join("other.flac");
        assert_eq!(
            sandboxed(&outside.to_string_lossy(), &root, &grants).unwrap_err(),
            "File path is outside the addon sandbox"
        );
        assert_eq!(
            sandboxed("../escape.flac", &root, &grants).unwrap_err(),
            "Invalid addon file path"
        );
        assert_eq!(
            sandboxed("", &root, &grants).unwrap_err(),
            "Invalid addon file path"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn a_progress_report_is_whole() {
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let sink: Arc<dyn Fn(&Progress) + Send + Sync> = {
            let seen = Arc::clone(&seen);
            Arc::new(move |progress: &Progress| seen.lock().unwrap().push(progress.clone()))
        };
        let reporter = Reporter {
            current: Mutex::default(),
            sink: Some(sink),
        };
        reporter.stage("preparing_media");
        reporter.percent(40.0);
        reporter.message("Receiving audio");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(
            seen[2],
            Progress {
                stage: Some("preparing_media".into()),
                percent: Some(40.0),
                message: Some("Receiving audio".into())
            }
        );
    }

    #[test]
    fn the_watch_counts_only_new_progress() {
        let watch = Watch::new();
        watch.begin();
        *lock(&watch.last) = Instant::now() - STALL;
        assert!(watch.expired());
        watch.percent(10.0);
        assert!(!watch.expired());
        *lock(&watch.last) = Instant::now() - STALL;
        watch.percent(10.0);
        assert!(watch.expired(), "the same percent is not news");
        let step = |written| Transfer {
            part: PathBuf::from("a.part"),
            file: PathBuf::from("a.flac"),
            total: 100,
            written,
            done: false,
        };
        assert!(!watch.transfer(&step(0)));
        assert!(watch.expired(), "a header without bytes is not news");
        assert!(watch.transfer(&step(5)), "the first byte");
        assert!(!watch.expired());
        assert!(!watch.transfer(&step(9)));
        watch.end();
        *lock(&watch.last) = Instant::now() - STALL;
        assert!(!watch.expired(), "only `download` is watched");
    }
}
