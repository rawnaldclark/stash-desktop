//! What an add-on asks the core for (spec §2; Lokal `morneye_host.dart`): its files, kept in a folder of its own
//! (and the folders a download grants it), downloads into them, its network, its keeping, its signed session, its
//! login, ciphers and helpers.
//!
//! One [`Host`] per running add-on ([`crate::engine::Engine`]); it answers each call the add-on makes through the
//! prelude's `bridge` with `{"value": …}`, or `{"failure": "…"}` (which the prelude throws).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::bytes;
use crate::crypto;
use crate::engine::{Engine, HostFn};
use crate::manifest::Manifest;
use crate::net::Network;
use crate::session::Session;
use crate::store::Store;

/// How far a download into a file is, for playing it while it comes in (spec §2.1.1 "live hooks").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// The `.part` file being written.
    pub part: PathBuf,
    /// The file it becomes.
    pub file: PathBuf,
    /// Its whole length, 0 when the server did not say.
    pub total: u64,
    pub written: u64,
    pub done: bool,
}

/// What the core hears from an add-on's work.
#[derive(Default, Clone)]
pub struct Hooks {
    /// A percent (0 to 100) of the work, from the add-on's `progress` and from downloads.
    pub progress: Option<Arc<dyn Fn(f64) + Send + Sync>>,
    /// Words about it (`utils.setDownloadStatus`, at most 200 characters; "Downloading audio segments…").
    pub status: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    /// Each step of a single-file download (not of segments), for playing it while it comes in.
    pub transfer: Option<Arc<dyn Fn(&Transfer) + Send + Sync>>,
}

/// How a host is made.
pub struct HostSetup {
    /// The add-on's key (its storage namespace).
    pub key: String,
    pub manifest: Arc<Manifest>,
    /// Its files' folder (`<mornye>/data/<key>/files`), made when the host is.
    pub root: PathBuf,
    /// Other folders it may write in (a download's work folder).
    pub grants: Vec<PathBuf>,
    pub network: Arc<Network>,
    pub store: Arc<Store>,
    pub session: Option<Arc<Session>>,
    pub hooks: Hooks,
    /// FFmpeg, when the computer has it (`ffmpeg.convert`); without it, `ffmpeg.convert` answers
    /// `{success: false, error: "FFmpeg is not available"}`.
    pub ffmpeg: Option<PathBuf>,
    /// Stash's version, for `utils.appUserAgent`.
    pub app_version: String,
}

pub struct Host {
    setup: HostSetup,
    created: Instant,
    aborted: AtomicBool,
    engine: OnceLock<Weak<Engine>>,
}

impl Host {
    pub fn new(setup: HostSetup) -> Arc<Host> {
        let _ = std::fs::create_dir_all(&setup.root);
        Arc::new(Host {
            setup,
            created: Instant::now(),
            aborted: AtomicBool::new(false),
            engine: OnceLock::new(),
        })
    }

    /// The engine whose callbacks the host runs (an add-on's `onProgress`, a patterned transform's progress).
    pub fn bind(&self, engine: &Arc<Engine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    /// Cancels the add-on's work: its waits and transfers stop (`Extension operation cancelled`).
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    pub fn aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    pub fn setup(&self) -> &HostSetup {
        &self.setup
    }

    /// The bridge the engine is given: each call answered as `{"value"}` or `{"failure"}` (failures worded as
    /// rt.dart:178-184 says).
    pub fn bridge(self: &Arc<Self>) -> HostFn {
        let host = Arc::clone(self);
        Arc::new(move |method, json| {
            let args: Vec<Value> = match serde_json::from_str::<Value>(json) {
                Ok(Value::Array(args)) => args,
                Ok(other) => vec![other],
                Err(_) => Vec::new(),
            };
            match host.call(method, args) {
                Ok(value) => serde_json::json!({ "value": value }).to_string(),
                Err(failure) => serde_json::json!({ "failure": failure }).to_string(),
            }
        })
    }

    /// Runs the add-on's callback `id` with `args` while a call waits; its error message, empty when it went well.
    pub fn run_callback(&self, id: u64, args: &[Value]) -> String {
        match self.engine.get().and_then(Weak::upgrade) {
            Some(engine) => engine.callback(id, args),
            None => String::new(),
        }
    }

    /// One call (spec §2: `file.*`, `http.*`, `storage.*`, `credentials.*`, `session.*`, `auth.*`, `ffmpeg.*`,
    /// `progress`, `log` (dropped: they can hold tokens), `encode`, `decode`, `base64Bytes`, `decodeBase64Bytes`,
    /// `url.*`, `matching.*`, `gobackend.*`, `utils.*`; else `Unsupported host API: <method>`), with every return
    /// shape and every error as the spec gives it: `file.*` and `http.*` failures are answered, not thrown.
    pub fn call(&self, method: &str, args: Vec<Value>) -> Result<Value, String> {
        self.check()?;
        let (name, action) = split(method);
        let args = args.as_slice();
        match name {
            "file" => return Ok(self.file_answer(action, args)),
            "http" => return Ok(self.http_call(action, args)),
            "storage" | "credentials" => return self.keeping(name, action, args),
            "session" => {
                if let Some(answer) = self.session_call(action, args)? {
                    return Ok(answer);
                }
            }
            "auth" => return self.auth_call(action, args),
            "ffmpeg" => return Ok(self.ffmpeg_call(action, args)),
            _ => {}
        }
        if method == "progress" {
            if let (Some(hook), Some(percent)) = (&self.setup.hooks.progress, arg(args, 0).as_f64())
            {
                hook(percent);
            }
            return Ok(Value::Bool(true));
        }
        if name == "gobackend" && action == "getAudioQuality" {
            let path = self.file(arg(args, 0))?;
            return crate::probe::probe(&path);
        }
        if name == "utils"
            && let Some(answer) = self.utils_call(action, args)
        {
            return answer;
        }
        if let Some(answer) = stateless(method, args) {
            return answer;
        }
        Err(format!("Unsupported host API: {method}"))
    }

    // ----- waiting and cancelling -----

    fn check(&self) -> Result<(), String> {
        if self.aborted() {
            Err(CANCELLED.into())
        } else {
            Ok(())
        }
    }

    /// Waits `pause`, failing as soon as the work is cancelled.
    fn wait(&self, pause: Duration) -> Result<(), String> {
        let until = Instant::now() + pause;
        loop {
            self.check()?;
            let now = Instant::now();
            if now >= until {
                return Ok(());
            }
            std::thread::sleep((until - now).min(Duration::from_millis(50)));
        }
    }

    /// Runs the add-on's function `descriptor` (`{"__callback": id}`), when it is one; its errors are ignored.
    fn notify(&self, descriptor: Option<&Value>, args: &[Value]) {
        if let Some(id) = callback_id(descriptor) {
            let _ = self.run_callback(id, args);
        }
    }

    // ----- files -----

    fn bases(&self) -> Vec<PathBuf> {
        bases_of(&self.setup.root, &self.setup.grants)
    }

    /// `raw` inside the add-on's folder or a granted one, with no links (spec §2.1).
    fn file(&self, raw: &Value) -> Result<PathBuf, String> {
        sandbox(self.setup.manifest.permissions.file, &self.bases(), raw)
    }

    /// `path` with `suffix` added to its name, checked again.
    fn sibling(&self, path: &Path, suffix: &str) -> Result<PathBuf, String> {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        confine(&self.bases(), Path::new(&name))
    }

    /// `file.*`: every failure is an answer `{success: false, error, error_type}` (`exists` answers `false`).
    fn file_answer(&self, action: &str, args: &[Value]) -> Value {
        match self.file_call(action, args) {
            Ok(answer) => answer,
            Err(_) if action == "exists" => Value::Bool(false),
            Err(error) => json!({
                "success": false,
                "error": if error == CANCELLED { "Cancelled".to_string() } else { strip_error_kind(&error) },
                "error_type": if self.aborted() { "cancelled" } else { "download_error" },
            }),
        }
    }

    fn file_call(&self, action: &str, args: &[Value]) -> Result<Value, String> {
        let raw = arg(args, 0);
        let data = arg(args, 1);
        let empty = Map::new();
        let options = args.get(2).and_then(Value::as_object).unwrap_or(&empty);
        match action {
            "download" => {
                let callback = options.get("onProgress");
                let progress = |written: u64, total: u64| {
                    self.notify(callback, &[json!(written), json!(total)])
                };
                return self.transfer(&text(raw), &text(data), options, &progress, false);
            }
            "downloadSegments" => return self.segments(raw, &text(data), options),
            "transformPatternedBlocks" => return self.patterned(raw, data, options, args.get(3)),
            _ => {}
        }
        let path = self.file(raw)?;
        match action {
            "exists" => Ok(Value::Bool(fs::metadata(&path).is_ok())),
            "getSize" => {
                let meta = fs::metadata(&path).map_err(io_text)?;
                if !meta.is_file() {
                    return Err("Cannot retrieve length of file".into());
                }
                Ok(json!({ "success": true, "size": meta.len() }))
            }
            "delete" => {
                if fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
                    fs::remove_file(&path).map_err(io_text)?;
                }
                Ok(json!({ "success": true }))
            }
            "copy" | "move" => {
                let target = self.file(data)?;
                make_parent(&target)?;
                if action == "copy" {
                    fs::copy(&path, &target).map_err(io_text)?;
                } else {
                    move_file(&path, &target)?;
                }
                Ok(json!({ "success": true }))
            }
            "read" => read_file(&path, None),
            "readBytes" => read_file(&path, Some(data.as_object().unwrap_or(&empty))),
            "write" => write_text(&path, data),
            "writeBytes" => write_bytes(&path, data, options),
            _ => Err("Unknown file API".into()),
        }
    }

    /// `file.download` (and each piece of `file.downloadSegments`): resumed with `Range` and `If-Range`, retried
    /// with backoff, a 30 s stall limit per gap, at most 4 GiB (spec §2.1.1).
    fn transfer(
        &self,
        url: &str,
        destination: &str,
        options: &Map<String, Value>,
        progress: &dyn Fn(u64, u64),
        segment: bool,
    ) -> Result<Value, String> {
        let mut settings = self
            .setup
            .manifest
            .download_transfer()
            .as_object()
            .cloned()
            .unwrap_or_default();
        settings.extend(
            options
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        let target = self.file(&Value::String(destination.to_string()))?;
        make_parent(&target)?;
        let part = self.sibling(&target, ".part")?;
        let checkpoint = self.sibling(&target, ".checkpoint")?;
        let attempts = int_of(settings.get("maxAttempts")).unwrap_or(3).clamp(1, 6) as u32;
        let resume = settings.get("resume") != Some(&Value::Bool(false));
        let persistent = settings.get("persistentCheckpoint") == Some(&Value::Bool(true));
        let headers = crate::net::headers_of(settings.get("headers"));
        // A request that names its own range (an HLS byte range) is never resumed with another one, nor chunked.
        let own_range = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("range"));
        let resumable = resume && !own_range;
        let chunk =
            (settings.get("chunked") == Some(&Value::Bool(true)) && !own_range).then(|| {
                int_of(settings.get("chunkSize"))
                    .map_or(CHUNK, |size| size.clamp(64 * 1024, 64 * 1024 * 1024) as u64)
            });
        let url_hash = crate::hex(&crypto::sha256(url.as_bytes()));
        let mut validator = String::new();
        if persistent
            && resume
            && let Some(saved) = fs::read(&checkpoint)
                .ok()
                .and_then(|saved| serde_json::from_slice::<Value>(&saved).ok())
            && saved.get("urlHash").and_then(Value::as_str) == Some(url_hash.as_str())
        {
            validator = text_or_empty(saved.get("validator").unwrap_or(&NULL));
        }
        let job = Job {
            url,
            destination,
            target: &target,
            part: &part,
            checkpoint: &checkpoint,
            headers: &headers,
            resumable,
            chunk,
            persistent,
            direct_media: settings.get("directMedia") == Some(&Value::Bool(true)),
            url_hash: &url_hash,
            segment,
            live: if segment {
                None
            } else {
                self.setup.hooks.transfer.as_deref()
            },
            progress,
        };
        let mut throttle = Throttle::default();
        for attempt in 1..=attempts {
            self.check()?;
            let fault = match self.attempt(&job, attempt, &mut validator, &mut throttle) {
                Ok(answer) => return Ok(answer),
                Err(fault) => fault,
            };
            if self.aborted() {
                return Err(fault.text());
            }
            if (validator.is_empty() || !resumable) && fs::metadata(&part).is_ok() {
                let _ = fs::remove_file(&part);
            }
            let retryable = match fault {
                Fault::Status(status) => RETRY_STATUSES.contains(&status),
                Fault::Text(_) => true,
            };
            if attempt == attempts || !retryable {
                return Err(fault.text());
            }
            self.wait(Duration::from_millis((500u64 << (attempt - 1)).min(8_000)))?;
        }
        Err("Download failed".into())
    }

    /// One try of a [`Host::transfer`]. A chunked download (`chunked: true`) asks for `chunkSize` bytes at a time,
    /// as servers that slow one long answer (YouTube's) serve each range at full speed.
    fn attempt(
        &self,
        job: &Job<'_>,
        attempt: u32,
        validator: &mut String,
        throttle: &mut Throttle,
    ) -> Result<Value, Fault> {
        let mut offset = 0u64;
        if job.resumable && !validator.is_empty() {
            offset = fs::metadata(job.part).map(|meta| meta.len()).unwrap_or(0);
        }
        let mut output: Option<File> = None;
        let mut written = offset;
        let mut total = 0u64;
        let mut first = true;
        let mut resumed = false;
        loop {
            let from = written;
            let mut headers = job.headers.to_vec();
            if from > 0 || job.chunk.is_some() {
                headers.retain(|(name, _)| {
                    !name.eq_ignore_ascii_case("range") && !name.eq_ignore_ascii_case("if-range")
                });
                let end = job
                    .chunk
                    .map(|size| (from + size - 1).to_string())
                    .unwrap_or_default();
                headers.push(("Range".into(), format!("bytes={from}-{end}")));
                if !validator.is_empty() {
                    headers.push(("If-Range".into(), validator.clone()));
                }
            }
            let answer = self
                .setup
                .network
                .request(job.url, "GET", &[], &headers, job.direct_media, Some(STALL))
                .map_err(Fault::Text)?;
            let status = answer.status;
            if !(200..300).contains(&status) {
                return Err(Fault::Status(status));
            }
            let range = answer.header("content-range").and_then(content_range);
            let next_validator = match answer.header("etag") {
                Some(tag) if !tag.starts_with("W/") => tag.to_string(),
                _ => answer.header("last-modified").unwrap_or("").to_string(),
            };
            let length = answer
                .header("content-length")
                .and_then(|value| value.trim().parse::<u64>().ok());
            if first {
                if from > 0 && (status != 206 || range.map(|(start, _, _)| start) != Some(from)) {
                    if status == 206 {
                        return Err(Fault::Text("Invalid resumed content range".into()));
                    }
                    written = 0;
                }
                if written > 0 && next_validator != *validator {
                    return Err(Fault::Text("Download validator changed".into()));
                }
                *validator = next_validator;
                resumed = written > 0;
                // What the file holds in all: the whole file when resuming or in chunks (as Lokal), or the piece a
                // range of the add-on's own asked for.
                total = match range {
                    Some((_, _, whole)) if job.chunk.is_some() && status == 206 => whole,
                    Some((start, end, _)) => written + (end + 1).saturating_sub(start),
                    None => length.map_or(0, |length| length + written),
                };
                if total > MAX_AUDIO {
                    return Err(Fault::Text(
                        "Addon audio exceeds the file size limit".into(),
                    ));
                }
                if job.persistent {
                    let saved = json!({ "urlHash": job.url_hash, "validator": validator.as_str() });
                    fs::write(job.checkpoint, saved.to_string()).map_err(io_fault)?;
                }
                let opened = if written > 0 {
                    OpenOptions::new().append(true).create(true).open(job.part)
                } else {
                    File::create(job.part)
                };
                output = Some(opened.map_err(io_fault)?);
                self.tell_live(job, total, written, false);
            } else if status != 206 || range.map(|(start, _, _)| start) != Some(from) {
                return Err(Fault::Text("Invalid chunked content range".into()));
            } else if !next_validator.is_empty() && next_validator != *validator {
                return Err(Fault::Text("Download validator changed".into()));
            }
            let Some(out) = output.as_mut() else {
                return Err(Fault::Text("Download failed".into()));
            };
            let mut body = answer.body;
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let read = match body.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) => return Err(Fault::Text(error.to_string())),
                };
                self.check().map_err(Fault::Text)?;
                out.write_all(&buffer[..read]).map_err(io_fault)?;
                written += read as u64;
                if written > MAX_AUDIO {
                    return Err(Fault::Text(
                        "Addon audio exceeds the file size limit".into(),
                    ));
                }
                self.tell_live(job, total, written, false);
                // The add-on hears at most four times a second (and at the end); live readers every chunk.
                let now = Instant::now();
                if (total > 0 && written == total) || throttle.due(now) {
                    throttle.last = Some(now);
                    throttle.written = Some(written);
                    self.report_bytes(job, written, total);
                }
            }
            drop(body);
            first = false;
            let more = job.chunk.is_some() && status == 206 && total > 0 && written < total;
            if !more {
                break;
            }
            if written == from {
                return Err(Fault::Text("Incomplete addon audio download".into()));
            }
        }
        if throttle.written != Some(written) {
            self.report_bytes(job, written, total);
        }
        if written == 0 || (total > 0 && written != total) {
            return Err(Fault::Text("Incomplete addon audio download".into()));
        }
        drop(output);
        self.check().map_err(Fault::Text)?;
        fs::rename(job.part, job.target).map_err(io_fault)?;
        if fs::metadata(job.checkpoint).is_ok() {
            let _ = fs::remove_file(job.checkpoint);
        }
        self.tell_live(job, total, written, true);
        let mut answer = json!({
            "success": true,
            "path": job.destination,
            "size": written,
            "attempts": attempt,
        });
        if resumed {
            answer["resumed"] = Value::Bool(true);
        }
        Ok(answer)
    }

    fn tell_live(&self, job: &Job<'_>, total: u64, written: u64, done: bool) {
        if let Some(live) = job.live {
            live(&Transfer {
                part: job.part.to_path_buf(),
                file: job.target.to_path_buf(),
                total,
                written,
                done,
            });
        }
    }

    fn report_bytes(&self, job: &Job<'_>, written: u64, total: u64) {
        if !job.segment
            && total > 0
            && let Some(hook) = &self.setup.hooks.progress
        {
            hook((written as f64 * 100.0 / total as f64).clamp(0.0, 100.0));
        }
        (job.progress)(written, total);
    }

    /// `file.downloadSegments`: batches fetched in parallel, joined in order into one file (spec §2.1.2).
    fn segments(
        &self,
        list: &Value,
        destination: &str,
        options: &Map<String, Value>,
    ) -> Result<Value, String> {
        let list = match list.as_array() {
            Some(list) if !list.is_empty() && list.len() <= MAX_SEGMENTS => list,
            _ => return Err("Invalid download segments".into()),
        };
        let output = self.file(&Value::String(destination.to_string()))?;
        let dir = self.sibling(&output, ".segments")?;
        fs::create_dir_all(&dir).map_err(io_text)?;
        let result = self.join_segments(list, &output, &dir, options);
        let _ = fs::remove_dir_all(&dir);
        let size = result?;
        Ok(json!({ "success": true, "path": destination, "size": size, "segments": list.len() }))
    }

    fn join_segments(
        &self,
        list: &[Value],
        output: &Path,
        dir: &Path,
        options: &Map<String, Value>,
    ) -> Result<u64, String> {
        let width = int_of(options.get("maxParallel"))
            .or_else(|| {
                int_of(
                    self.setup
                        .manifest
                        .download_transfer()
                        .get("maxParallelSegments"),
                )
            })
            .unwrap_or(3)
            .clamp(1, 8) as usize;
        let state = Mutex::new(SegmentProgress::default());
        let callback = options.get("onProgress");
        let temp = self.sibling(output, ".part")?;
        let mut sink = File::create(&temp).map_err(io_text)?;
        let joined = (|| -> Result<u64, String> {
            let mut size = 0u64;
            let mut buffer = vec![0u8; 256 * 1024];
            for start in (0..list.len()).step_by(width) {
                let indices: Vec<usize> = (start..(start + width).min(list.len())).collect();
                let state = &state;
                let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
                    let running: Vec<_> = indices
                        .iter()
                        .map(|&index| {
                            scope.spawn(move || {
                                self.fetch_segment(list, index, dir, options, state, callback)
                            })
                        })
                        .collect();
                    running
                        .into_iter()
                        .map(|thread| {
                            thread
                                .join()
                                .unwrap_or_else(|_| Err("Segment download failed".into()))
                        })
                        .collect()
                });
                if let Some(Err(error)) = results.into_iter().find(Result::is_err) {
                    return Err(error);
                }
                // Only whole pieces, in their order, ever reach the file.
                for &index in &indices {
                    let piece_path = dir.join(index.to_string());
                    let mut piece = File::open(&piece_path).map_err(io_text)?;
                    loop {
                        let read = match piece.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(read) => read,
                            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                            Err(error) => return Err(io_text(error)),
                        };
                        self.check()?;
                        size += read as u64;
                        if size > MAX_AUDIO {
                            return Err("Segmented audio exceeds the file size limit".into());
                        }
                        sink.write_all(&buffer[..read]).map_err(io_text)?;
                    }
                    sink.flush().map_err(io_text)?;
                    drop(piece);
                    fs::remove_file(&piece_path).map_err(io_text)?;
                }
            }
            Ok(size)
        })();
        drop(sink);
        match joined {
            Ok(size) => {
                fs::rename(&temp, output).map_err(io_text)?;
                Ok(size)
            }
            Err(error) => {
                let _ = fs::remove_file(&temp);
                Err(error)
            }
        }
    }

    /// One piece of a segmented download, into `<dir>/<index>`.
    fn fetch_segment(
        &self,
        list: &[Value],
        index: usize,
        dir: &Path,
        options: &Map<String, Value>,
        state: &Mutex<SegmentProgress>,
        callback: Option<&Value>,
    ) -> Result<(), String> {
        let segment = &list[index];
        let url = match segment {
            Value::String(url) => url.clone(),
            Value::Object(fields) => text(fields.get("url").unwrap_or(&NULL)),
            _ => String::new(),
        };
        let mut headers = Map::new();
        if let Some(Value::Object(shared)) = options.get("headers") {
            headers.extend(shared.clone());
        }
        if let Some(Value::Object(own)) = segment.get("headers") {
            headers.extend(own.clone());
        }
        let mut piece_options = options.clone();
        piece_options.insert("headers".into(), Value::Object(headers));
        let destination = dir.join(index.to_string()).to_string_lossy().into_owned();
        let count = list.len();
        let progress = |written: u64, _total: u64| {
            lock(state).bytes.insert(index, written);
            self.report_segments(state, count, callback);
        };
        self.transfer(&url, &destination, &piece_options, &progress, true)?;
        lock(state).completed += 1;
        self.report_segments(state, count, callback);
        Ok(())
    }

    /// Tells the add-on (one call at a time) and the core how many pieces are done, when that number changed.
    fn report_segments(
        &self,
        state: &Mutex<SegmentProgress>,
        count: usize,
        callback: Option<&Value>,
    ) {
        let mut state = lock(state);
        let done = state.completed;
        if state.reported == Some(done) {
            return;
        }
        state.reported = Some(done);
        let sum: u64 = state.bytes.values().sum();
        if let Some(hook) = &self.setup.hooks.progress {
            hook(done as f64 * 100.0 / count as f64);
        }
        if let Some(hook) = &self.setup.hooks.status {
            hook(SEGMENTS_STATUS);
        }
        self.notify(callback, &[json!(sum), json!(0), json!(done), json!(count)]);
    }

    /// `file.transformPatternedBlocks` (spec §2.1.3): every `transformEvery`-th block of `segmentSize` bytes, from
    /// `transformOffset`, run through the cipher on its own from the same IV; the add-on's progress function is told
    /// after every block, and its error stops the work.
    fn patterned(
        &self,
        input: &Value,
        output: &Value,
        options: &Map<String, Value>,
        progress: Option<&Value>,
    ) -> Result<Value, String> {
        let source = self.file(input)?;
        let target = self.file(output)?;
        let temp = self.sibling(&target, ".tmp")?;
        let size = int_of(options.get("segmentSize")).unwrap_or(0);
        let every = int_of(options.get("transformEvery")).unwrap_or(1);
        let offset = int_of(options.get("transformOffset")).unwrap_or(0);
        let padding_ok = match options.get("padding") {
            None | Some(Value::Null) => true,
            Some(padding) => padding.as_str() == Some("none"),
        };
        if size < 1
            || size > MAX_READ as i64
            || every < 1
            || offset < 0
            || offset >= every
            || !padding_ok
        {
            return Err("Invalid patterned transform options".into());
        }
        let reader = File::open(&source).map_err(io_text)?;
        make_parent(&target)?;
        let writer = File::create(&temp).map_err(io_text)?;
        let pattern = Pattern {
            size: size as usize,
            every: every as u64,
            offset: offset as u64,
        };
        let done = self
            .transform_blocks(reader, writer, options, &pattern, progress)
            .and_then(|counts| {
                self.check()?;
                fs::rename(&temp, &target).map_err(io_text)?;
                Ok(counts)
            });
        let (processed, blocks, transformed) = match done {
            Ok(counts) => counts,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        };
        Ok(json!({
            "success": true,
            "path": output,
            "bytes_processed": processed,
            "segments_processed": blocks,
            "segments_transformed": transformed,
        }))
    }

    /// Bytes processed, blocks read, blocks transformed.
    fn transform_blocks(
        &self,
        reader: File,
        writer: File,
        options: &Map<String, Value>,
        pattern: &Pattern,
        progress: Option<&Value>,
    ) -> Result<(u64, u64, u64), String> {
        let total = reader.metadata().map_err(io_text)?.len();
        let mut reader = BufReader::with_capacity(1 << 20, reader);
        let mut writer = BufWriter::with_capacity(1 << 20, writer);
        let partial = options.get("transformPartial") == Some(&Value::Bool(true));
        let encrypt = options.get("operation").and_then(Value::as_str) == Some("encrypt");
        let callback = callback_id(progress);
        let mut cipher: Option<crypto::BlockCipher> = None;
        let mut buffer = vec![0u8; pattern.size];
        let (mut processed, mut index, mut transformed) = (0u64, 0u64, 0u64);
        loop {
            self.check()?;
            let count = read_full(&mut reader, &mut buffer)?;
            if count == 0 {
                break;
            }
            let block = &buffer[..count];
            if index % pattern.every == pattern.offset && (count == pattern.size || partial) {
                let keyed = match cipher.take() {
                    Some(keyed) => keyed,
                    None => cipher_of(options)?,
                };
                let out = keyed.apply(encrypt, block)?;
                cipher = Some(keyed);
                writer.write_all(&out).map_err(io_text)?;
                transformed += 1;
            } else {
                writer.write_all(block).map_err(io_text)?;
            }
            processed += count as u64;
            index += 1;
            if let Some(id) = callback {
                let error = self.run_callback(id, &[json!(processed), json!(total)]);
                if !error.is_empty() {
                    return Err(error);
                }
            }
        }
        writer.flush().map_err(io_text)?;
        Ok((processed, index, transformed))
    }

    // ----- network, keeping, sessions, logins -----

    /// `http.*`: failures are answered `{ok: false, status: 0, error}`, never thrown.
    fn http_call(&self, action: &str, args: &[Value]) -> Value {
        let network = &self.setup.network;
        if action == "clearCookies" {
            network.clear_cookies();
            return Value::Bool(true);
        }
        let empty = Map::new();
        let options = args.get(1).and_then(Value::as_object).unwrap_or(&empty);
        let mut headers = crate::net::headers_of(options.get("headers"));
        let method = opt_text(options, "method", "GET");
        let body = opt_text(options, "body", "");
        let as_fetch = options.get("asFetch") == Some(&Value::Bool(true));
        if !as_fetch
            && (!body.is_empty() || method == "POST")
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        network
            .json_response(&text(arg(args, 0)), &method, &body, &headers, as_fetch)
            .unwrap_or_else(|error| json!({ "ok": false, "status": 0, "error": error }))
    }

    /// `storage.*` / `credentials.*`, kept under the add-on's key.
    fn keeping(&self, purpose: &str, action: &str, args: &[Value]) -> Result<Value, String> {
        if !self.setup.manifest.permissions.storage {
            return Err("Storage permission denied".into());
        }
        let store = &self.setup.store;
        let namespace = &self.setup.key;
        match action {
            "get" => {
                let saved = store.read(namespace, purpose);
                let key = text(arg(args, 0));
                Ok(
                    json!({ "found": saved.contains_key(&key), "value": saved.get(&key).cloned().unwrap_or(Value::Null) }),
                )
            }
            "clear" => {
                store.write(namespace, purpose, &Map::new())?;
                Ok(json!({ "success": true }))
            }
            _ => {
                let value = if action == "remove" {
                    None
                } else {
                    Some(arg(args, 1).clone())
                };
                store.update(namespace, purpose, &text(arg(args, 0)), value)?;
                Ok(json!({ "success": true }))
            }
        }
    }

    /// `session.*`; `None` for an action it does not know (which then fails as unsupported).
    fn session_call(&self, action: &str, args: &[Value]) -> Result<Option<Value>, String> {
        let Some(session) = &self.setup.session else {
            return Err("Signed sessions are not configured".into());
        };
        let answer = match action {
            "signedFetch" => session.signed_fetch(
                &text(arg(args, 0)),
                &text(arg(args, 1)),
                &text_or_empty(arg(args, 2)),
                &crate::net::headers_of(args.get(3)),
                &|pause| self.wait(pause),
            )?,
            "completeGrant" => session.complete_grant(arg(args, 0).as_str().map(str::to_string))?,
            "status" => session.status(),
            "clear" => session.clear(),
            _ => return Ok(None),
        };
        Ok(Some(answer))
    }

    /// `auth.*`: a call during which the person disconnected answers `Disconnected`; what changed is kept.
    fn auth_call(&self, action: &str, args: &[Value]) -> Result<Value, String> {
        let network = &self.setup.network;
        let generation = network.generation();
        let answer = crate::auth::call(network, action, args)?;
        if network.generation() != generation {
            return Ok(json!({ "success": false, "error": "Disconnected" }));
        }
        if !matches!(
            action,
            "getAuthCode" | "getTokens" | "isAuthenticated" | "getPKCE"
        ) {
            network.keep();
        }
        Ok(answer)
    }

    // ----- FFmpeg -----

    /// `ffmpeg.*`: every failure is answered `{success: false, error}`.
    fn ffmpeg_call(&self, action: &str, args: &[Value]) -> Value {
        let answer = match action {
            "getInfo" => self
                .file(arg(args, 0))
                .and_then(|path| crate::probe::probe(&path)),
            "convert" if args.len() < 2 => Err("Expected input and output paths".to_string()),
            "convert" => self.convert(args),
            _ => Err("Unsupported FFmpeg operation".to_string()),
        };
        answer.unwrap_or_else(|error| json!({ "success": false, "error": error }))
    }

    fn convert(&self, args: &[Value]) -> Result<Value, String> {
        let input = self.file(&Value::String(text(arg(args, 0))))?;
        let output = self.file(&Value::String(text(arg(args, 1))))?;
        let empty = Map::new();
        let options = args.get(2).and_then(Value::as_object).unwrap_or(&empty);
        let codec = opt_text(options, "codec", "copy").to_lowercase();
        convert_audio(
            self.setup.ffmpeg.as_deref(),
            &input,
            &output,
            &codec,
            &|| self.aborted(),
        )?;
        let size = fs::metadata(&output).map_err(io_text)?.len();
        Ok(json!({ "success": true, "path": output.to_string_lossy(), "bytes_processed": size }))
    }

    // ----- helpers that need the host -----

    /// The `utils.*` that need the host; `None` for the others.
    fn utils_call(&self, action: &str, args: &[Value]) -> Option<Result<Value, String>> {
        let answer = match action {
            "isDownloadCancelled" | "isRequestCancelled" => Value::Bool(self.aborted()),
            "appUserAgent" => Value::String(format!(
                "Stash/{} Morneye-compatible/{}",
                self.setup.app_version,
                crate::COMPAT_VERSION
            )),
            "setDownloadStatus" => {
                let words: String = text(arg(args, 0)).chars().take(200).collect();
                if let Some(hook) = &self.setup.hooks.status {
                    hook(&words);
                }
                Value::Bool(true)
            }
            "getResolutionRemainingMs" => {
                let spent = self
                    .created
                    .elapsed()
                    .as_millis()
                    .min(RESOLUTION_MS as u128) as u64;
                json!(RESOLUTION_MS - spent)
            }
            "sleep" => {
                let pause = int_of(args.first()).unwrap_or(0).clamp(0, 300_000) as u64;
                if let Err(error) = self.wait(Duration::from_millis(pause)) {
                    return Some(Err(error));
                }
                Value::Bool(true)
            }
            _ => return None,
        };
        Some(Ok(answer))
    }
}

const MAX_READ: u64 = 16 * 1024 * 1024;
const MAX_AUDIO: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SEGMENTS: usize = 20_000;
/// How long a download may go without a byte before its connection is dropped and the download resumed.
const STALL: Duration = Duration::from_secs(30);
/// A chunked download's piece when the add-on names none.
const CHUNK: u64 = 1024 * 1024;
const RETRY_STATUSES: [u16; 6] = [408, 429, 500, 502, 503, 504];
const CANCELLED: &str = "Extension operation cancelled";
const INVALID_PATH: &str = "Invalid addon file path";
const OUTSIDE: &str = "File path is outside the addon sandbox";
const LINKS: &str = "Addon paths cannot contain links or special files";
const SEGMENTS_STATUS: &str = "Downloading audio segments…";
const RANDOM_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36";
/// The time an add-on is told it has to find a song (`utils.getResolutionRemainingMs`).
const RESOLUTION_MS: u64 = 60_000;
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(180);
/// Keeps FFmpeg from opening a console window on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static NULL: Value = Value::Null;

/// A download, for each of its tries.
struct Job<'a> {
    url: &'a str,
    /// As the add-on gave it (what the answer's `path` says).
    destination: &'a str,
    target: &'a Path,
    part: &'a Path,
    checkpoint: &'a Path,
    headers: &'a [(String, String)],
    resumable: bool,
    /// The bytes asked for at a time, for a chunked download.
    chunk: Option<u64>,
    persistent: bool,
    direct_media: bool,
    url_hash: &'a str,
    segment: bool,
    live: Option<&'a (dyn Fn(&Transfer) + Send + Sync)>,
    progress: &'a dyn Fn(u64, u64),
}

/// Why a try failed: an HTTP status (retried only for 408, 429 and 5xx gateway trouble), or anything else.
enum Fault {
    Status(u16),
    Text(String),
}

impl Fault {
    fn text(self) -> String {
        match self {
            Fault::Status(status) => format!("Download returned HTTP {status}"),
            Fault::Text(text) => text,
        }
    }
}

/// When the add-on last heard of a download's bytes.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
    written: Option<u64>,
}

impl Throttle {
    fn due(&self, now: Instant) -> bool {
        self.last
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(250))
    }
}

/// A segmented download's progress, shared by its pieces.
#[derive(Default)]
struct SegmentProgress {
    bytes: HashMap<usize, u64>,
    completed: usize,
    reported: Option<usize>,
}

/// Which blocks a patterned transform changes.
struct Pattern {
    size: usize,
    every: u64,
    offset: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn split(method: &str) -> (&str, &str) {
    method.split_once('.').unwrap_or((method, ""))
}

fn arg(args: &[Value], index: usize) -> &Value {
    args.get(index).unwrap_or(&NULL)
}

/// Dart's `'$value'`: a string as it is, `null` as `null`, others as JSON.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Dart's `'${value ?? ''}'`.
fn text_or_empty(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        text(value)
    }
}

/// Dart's `'${options[key] ?? fallback}'`.
fn opt_text(options: &Map<String, Value>, key: &str, fallback: &str) -> String {
    match options.get(key) {
        None | Some(Value::Null) => fallback.to_string(),
        Some(value) => text(value),
    }
}

/// A JSON number as a whole number (`(v as num?)?.toInt()`); nothing for anything else.
fn int_of(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_i64().or_else(|| value.as_f64().map(|f| f as i64))
}

/// The id of an add-on function the prelude packed as `{"__callback": id}`.
fn callback_id(value: Option<&Value>) -> Option<u64> {
    let id = value?.get("__callback")?;
    id.as_u64()
        .or_else(|| id.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

fn io_text(error: std::io::Error) -> String {
    error.to_string()
}

fn io_fault(error: std::io::Error) -> Fault {
    Fault::Text(error.to_string())
}

/// Drops a leading `SomethingError: ` or `SomethingException: ` (Lokal's `^\w+(Error|Exception): `).
fn strip_error_kind(error: &str) -> String {
    let word = error
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(error.len());
    let (head, rest) = error.split_at(word);
    let named = (head.len() > 5 && head.ends_with("Error"))
        || (head.len() > 9 && head.ends_with("Exception"));
    match rest.strip_prefix(": ") {
        Some(message) if named => message.to_string(),
        _ => error.to_string(),
    }
}

/// `Content-Range: bytes <start>-<end>/<total>`.
fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.strip_prefix("bytes ")?;
    let (start, rest) = rest.split_once('-')?;
    let (end, total) = rest.split_once('/')?;
    let number = |digits: &str| {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse::<u64>().ok()
    };
    Some((number(start)?, number(end)?, number(total)?))
}

// ----- the sandbox -----

/// The add-on's folder and those granted to it, absolute and normalized.
fn bases_of(root: &Path, grants: &[PathBuf]) -> Vec<PathBuf> {
    std::iter::once(root)
        .chain(grants.iter().map(PathBuf::as_path))
        .map(|base| normalize(&std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf())))
        .collect()
}

/// An add-on's path (spec §2.1): a non-empty string without NUL; a relative one without a `..` part, taken from
/// the add-on's folder (`bases[0]`); within one of `bases`; with no link or special file along the way.
fn sandbox(allowed: bool, bases: &[PathBuf], raw: &Value) -> Result<PathBuf, String> {
    if !allowed {
        return Err("File permission denied".into());
    }
    let raw = match raw {
        Value::String(raw) if !raw.is_empty() && !raw.contains('\0') => raw.as_str(),
        _ => return Err(INVALID_PATH.into()),
    };
    let path = Path::new(raw);
    if !path.is_absolute() && raw.split(['/', '\\']).any(|part| part == "..") {
        return Err(INVALID_PATH.into());
    }
    let Some(root) = bases.first() else {
        return Err(OUTSIDE.into());
    };
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    confine(bases, &joined)
}

/// `path`, normalized, if it is one of `bases` or under one, with no link or special file from that base down.
fn confine(bases: &[PathBuf], path: &Path) -> Result<PathBuf, String> {
    let path = normalize(path);
    let (base, rest) = bases
        .iter()
        .find_map(|base| relative_to(base, &path).map(|rest| (base, rest)))
        .ok_or_else(|| OUTSIDE.to_string())?;
    let mut current = base.clone();
    no_link(&current)?;
    for part in rest {
        current.push(part);
        no_link(&current)?;
    }
    Ok(current)
}

/// The parts of `path` below `base`, when it is `base` or under it (letters of any case on Windows).
fn relative_to(base: &Path, path: &Path) -> Option<Vec<OsString>> {
    let mut theirs = path.components();
    for ours in base.components() {
        if !same_component(ours, theirs.next()?) {
            return None;
        }
    }
    Some(theirs.map(|part| part.as_os_str().to_os_string()).collect())
}

fn same_component(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `.` dropped and `..` taken back, by the text alone (never above the root).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn no_link(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let kind = meta.file_type();
            if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
                Err(LINKS.into())
            } else {
                Ok(())
            }
        }
        Err(_) => Ok(()),
    }
}

// ----- file operations -----

fn make_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(io_text),
        None => Ok(()),
    }
}

/// A rename, or a copy and delete where a rename cannot go.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) => {
            if fs::metadata(from).is_ok_and(|meta| meta.is_file()) && fs::copy(from, to).is_ok() {
                fs::remove_file(from).map_err(io_text)
            } else {
                Err(io_text(error))
            }
        }
    }
}

/// `file.read` (`options` absent: all of it, as text, at most 16 MiB) or `file.readBytes` (`offset`, `length`,
/// `encoding`).
fn read_file(path: &Path, options: Option<&Map<String, Value>>) -> Result<Value, String> {
    let empty = Map::new();
    let binary = options.is_some();
    let options = options.unwrap_or(&empty);
    let start = int_of(options.get("offset")).unwrap_or(0);
    let length = int_of(options.get("length")).unwrap_or(MAX_READ as i64);
    if start < 0 || length < 0 || length > MAX_READ as i64 {
        return Err("Invalid binary read range".into());
    }
    let mut file = File::open(path).map_err(io_text)?;
    let size = file.metadata().map_err(io_text)?.len();
    if !binary && size > MAX_READ {
        return Err("File read exceeds 16 MB".into());
    }
    let start = start as u64;
    let wanted = size.saturating_sub(start).min(length as u64);
    file.seek(SeekFrom::Start(start)).map_err(io_text)?;
    let mut bytes = Vec::with_capacity(wanted as usize);
    file.take(wanted).read_to_end(&mut bytes).map_err(io_text)?;
    let read = bytes.len() as u64;
    let data = if binary {
        bytes::encoded(&bytes, &opt_text(options, "encoding", "base64"))
    } else {
        Value::String(String::from_utf8_lossy(&bytes).into_owned())
    };
    Ok(json!({
        "success": true,
        "data": data,
        "offset": start,
        "size": size,
        "eof": start + read >= size,
        "bytes_read": read,
    }))
}

/// `file.write`: the text, as UTF-8, in place of what was there.
fn write_text(path: &Path, data: &Value) -> Result<Value, String> {
    let value = text(data).into_bytes();
    if value.len() as u64 > MAX_READ {
        return Err("File write exceeds 16 MB".into());
    }
    make_parent(path)?;
    let mut file = File::create(path).map_err(io_text)?;
    file.write_all(&value).map_err(io_text)?;
    let _ = file.sync_all();
    Ok(json!({ "success": true, "bytes_written": value.len() }))
}

/// `file.writeBytes`: appended (`append`), or written at `offset` over a file emptied first (`truncate`) or kept.
fn write_bytes(path: &Path, data: &Value, options: &Map<String, Value>) -> Result<Value, String> {
    let value = bytes::decode(data, &opt_text(options, "encoding", "base64"))?;
    if value.len() as u64 > MAX_READ {
        return Err("File write exceeds 16 MB".into());
    }
    make_parent(path)?;
    let start = int_of(options.get("offset")).unwrap_or(0);
    if start < 0 {
        return Err("Invalid write offset".into());
    }
    let append = options.get("append") == Some(&Value::Bool(true));
    let truncate = options.get("truncate") == Some(&Value::Bool(true));
    let opened = if append {
        OpenOptions::new().append(true).create(true).open(path)
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(truncate)
            .open(path)
    };
    let mut file = opened.map_err(io_text)?;
    if !append {
        file.seek(SeekFrom::Start(start as u64)).map_err(io_text)?;
    }
    file.write_all(&value).map_err(io_text)?;
    Ok(json!({ "success": true, "bytes_written": value.len() }))
}

/// Fills `buffer` from `reader` unless the end comes first; how much it holds.
fn read_full(reader: &mut impl Read, buffer: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(io_text(error)),
        }
    }
    Ok(filled)
}

/// The cipher a transform's options describe (`algorithm`, `mode` = cbc, `key` + `keyEncoding` = utf8, `iv` +
/// `ivEncoding` = utf8, `padding`).
fn cipher_of(options: &Map<String, Value>) -> Result<crypto::BlockCipher, String> {
    let iv = bytes::decode(
        options.get("iv").unwrap_or(&NULL),
        &opt_text(options, "ivEncoding", "utf8"),
    )?;
    let key = bytes::decode(
        options.get("key").unwrap_or(&NULL),
        &opt_text(options, "keyEncoding", "utf8"),
    )?;
    crypto::BlockCipher::new(
        &opt_text(options, "algorithm", ""),
        &opt_text(options, "mode", "cbc"),
        &key,
        &iv,
        options.get("padding").and_then(Value::as_str) == Some("pkcs7"),
    )
}

/// Runs FFmpeg as Lokal does (`morneye_ffmpeg.dart`): the first audio stream, its metadata, `copy` or `flac`; a
/// failed copy into `.flac` is tried once more with the FLAC encoder. Three minutes at most.
fn convert_audio(
    ffmpeg: Option<&Path>,
    input: &Path,
    output: &Path,
    codec: &str,
    aborted: &dyn Fn() -> bool,
) -> Result<(), String> {
    if codec != "copy" && codec != "flac" {
        return Err("Unsupported audio conversion".into());
    }
    if fs::metadata(input).map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Err("The audio conversion input is missing or empty.".into());
    }
    let Some(ffmpeg) = ffmpeg else {
        return Err("FFmpeg is not available".into());
    };
    make_parent(output)?;
    if fs::metadata(output).is_ok() {
        fs::remove_file(output).map_err(io_text)?;
    }
    let mut command = Command::new(ffmpeg);
    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
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
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("FFmpeg could not start: {error}"))?;
    let stderr = child.stderr.take();
    let said = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let started = Instant::now();
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(output);
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                stop(&mut child);
                return Err(io_text(error));
            }
        }
        if aborted() {
            stop(&mut child);
            return Err("Audio processing cancelled.".into());
        }
        if started.elapsed() >= FFMPEG_TIMEOUT {
            stop(&mut child);
            return Err("Audio processing timed out. Retry the track.".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let said = said.join().unwrap_or_default();
    if !status.success() {
        let _ = fs::remove_file(output);
        // Some muxers refuse a FLAC stream copied out of MP4; FLAC to FLAC is still lossless.
        if codec == "copy" && output.to_string_lossy().to_lowercase().ends_with(".flac") {
            return convert_audio(Some(ffmpeg), input, output, "flac", aborted);
        }
        let said = said.trim();
        let detail = if said.is_empty() {
            String::new()
        } else {
            let count = said.chars().count();
            format!(
                " {}",
                said.chars()
                    .skip(count.saturating_sub(500))
                    .collect::<String>()
            )
        };
        return Err(format!("FFmpeg audio conversion failed.{detail}"));
    }
    if fs::metadata(output).map(|meta| meta.len()).unwrap_or(0) == 0 {
        return Err("FFmpeg completed without producing audio.".into());
    }
    Ok(())
}

// ----- calls that need nothing of the host -----

/// `log`, `encode`, `decode`, `base64Bytes`, `decodeBase64Bytes`, `url.*`, `matching.*`, most of `gobackend.*` and
/// `utils.*`; `None` for what is not one of them.
fn stateless(method: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let first = arg(args, 0);
    match method {
        // Guest logs never reach the app's logs: they can hold tokens.
        // Unless the person running the core asks for them (`STASH_MORNYE_LOG=1`), to see why an add-on fails.
        "log" => {
            if std::env::var_os("STASH_MORNYE_LOG").is_some_and(|v| v == "1") {
                let level = text(first);
                let words: Vec<String> = arg(args, 1)
                    .as_array()
                    .map(|parts| parts.iter().map(text).collect())
                    .unwrap_or_default();
                eprintln!("[mornye {level}] {}", words.join(" "));
            }
            return Some(Ok(Value::Bool(true)));
        }
        "encode" => return Some(Ok(bytes::js_bytes(text(first).as_bytes()))),
        "decode" => {
            return Some(
                bytes::decode(first, "base64")
                    .map(|b| Value::String(String::from_utf8_lossy(&b).into_owned())),
            );
        }
        "base64Bytes" => {
            return Some(bytes::decode(first, "base64").map(|b| Value::String(bytes::base64(&b))));
        }
        "decodeBase64Bytes" => {
            return Some(
                bytes::decode(&Value::String(text_or_empty(first)), "base64").map(|b| json!(b)),
            );
        }
        _ => {}
    }
    let (name, action) = split(method);
    match name {
        "url" => url_call(action, args),
        "matching" => Some(matching_call(action, args)),
        "gobackend" => gobackend_call(action, args),
        "utils" => utils_pure(action, args),
        _ => None,
    }
}

fn url_call(action: &str, args: &[Value]) -> Option<Result<Value, String>> {
    Some(match action {
        "parse" => parse_url(&text(arg(args, 0)), arg(args, 1).as_str()),
        "query" => Ok(Value::Array(
            query_entries(arg(args, 0))
                .into_iter()
                .map(|(key, value)| json!([key, value]))
                .collect(),
        )),
        "encode" => Ok(Value::String(encode_query(arg(args, 0)))),
        _ => return None,
    })
}

fn matching_call(action: &str, args: &[Value]) -> Result<Value, String> {
    if action == "normalizeString" {
        return Ok(Value::String(normalize_string(&text(arg(args, 0)))));
    }
    compare_strings(&text(arg(args, 0)), &text(arg(args, 1))).map(|similarity| json!(similarity))
}

fn gobackend_call(action: &str, args: &[Value]) -> Option<Result<Value, String>> {
    Some(Ok(match action {
        "sanitizeFilename" => Value::String(sanitize(&text_or_empty(arg(args, 0)))),
        "buildFilename" => Value::String(build_filename(
            &text(arg(args, 0)),
            arg(args, 1).as_object(),
        )),
        "getLocalTime" => local_time(),
        // Stash keeps no ISRC index for add-ons.
        "checkISRCExists" => json!({ "exists": false, "file_path": "" }),
        "addToISRCIndex" => json!({ "success": true }),
        "getLyricsLRC" => json!({ "lyrics": "", "lrc": "" }),
        _ => return None,
    }))
}

/// The `utils.*` that need nothing of the host.
fn utils_pure(action: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let first = arg(args, 0);
    let second = arg(args, 1);
    let empty = Map::new();
    let options = second.as_object().unwrap_or(&empty);
    let answer = match action {
        "appVersion" => json!(crate::COMPAT_VERSION),
        "randomUserAgent" => json!(RANDOM_USER_AGENT),
        "base64Encode" => json!(bytes::base64(text(first).as_bytes())),
        "base64Decode" => json!(
            bytes::decode(&Value::String(text(first)), "base64")
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        ),
        "md5" => json!(crate::hex(&crypto::md5(text(first).as_bytes()))),
        "sha256" => json!(crate::hex(&crypto::sha256(text(first).as_bytes()))),
        // (message, key)
        "hmacSHA256" => json!(crate::hex(&crypto::hmac_sha256(
            text(second).as_bytes(),
            text(first).as_bytes()
        ))),
        "hmacSHA256Base64" => json!(bytes::base64(&crypto::hmac_sha256(
            text(second).as_bytes(),
            text(first).as_bytes()
        ))),
        // (key, data): the other way round, and raw bytes back.
        "hmacSHA1" => match (key_bytes(first), key_bytes(second)) {
            (Ok(key), Ok(data)) => json!(crypto::hmac_sha1(&key, &data)),
            (Err(error), _) | (_, Err(error)) => return Some(Err(error)),
        },
        "encrypt" | "decrypt" => text_crypt(action == "decrypt", first, second),
        "generateKey" => {
            let length = int_of(args.first()).unwrap_or(32);
            if (1..=4096).contains(&length) {
                let key = crate::random_bytes(length as usize);
                json!({ "success": true, "key": bytes::base64(&key), "hex": crate::hex(&key) })
            } else {
                json!({ "success": false, "error": "Invalid key length" })
            }
        }
        "encryptBlockCipher" => block_call("encrypt", first, options),
        "decryptBlockCipher" => block_call("decrypt", first, options),
        "decryptCTRSegments" => block_call("segments", first, options),
        _ => return None,
    };
    Some(Ok(answer))
}

/// A key or message for `utils.hmacSHA1`: bytes as given (a list, or a `Uint8Array`), else the text's UTF-8.
fn key_bytes(value: &Value) -> Result<Vec<u8>, String> {
    if value.is_array() || value.get("__bytes").is_some_and(Value::is_array) {
        bytes::decode(value, "base64")
    } else {
        Ok(text(value).into_bytes())
    }
}

/// `utils.encrypt` / `utils.decrypt`: `{success: true, data}` or `{success: false, error}`.
fn text_crypt(decrypt: bool, value: &Value, key: &Value) -> Value {
    let key = text(key);
    let result = if decrypt {
        let sealed = match value {
            Value::String(sealed) => Ok(sealed.clone()),
            other => bytes::decode(other, "base64").map(|b| bytes::base64(&b)),
        };
        sealed.and_then(|sealed| crypto::open_text(&sealed, &key))
    } else {
        crypto::seal_text(&text(value), &key)
    };
    match result {
        Ok(data) => json!({ "success": true, "data": data }),
        Err(error) => json!({ "success": false, "error": error }),
    }
}

/// `utils.encrypt/decryptBlockCipher` and `utils.decryptCTRSegments` (spec §2.12); failures answered.
fn block_call(operation: &str, data: &Value, options: &Map<String, Value>) -> Value {
    block_work(operation, data, options)
        .unwrap_or_else(|error| json!({ "success": false, "error": error }))
}

fn block_work(
    operation: &str,
    data: &Value,
    options: &Map<String, Value>,
) -> Result<Value, String> {
    let input = bytes::decode(data, &opt_text(options, "inputEncoding", "base64"))?;
    if input.len() as u64 > MAX_READ {
        return Err("Binary payload exceeds 16 MB".into());
    }
    let size = if options.get("algorithm").and_then(Value::as_str) == Some("blowfish") {
        8
    } else {
        16
    };
    let output_encoding = opt_text(options, "outputEncoding", "base64");
    if operation != "segments" {
        let output = cipher_of(options)?.apply(operation == "encrypt", &input)?;
        return Ok(json!({
            "success": true,
            "data": bytes::encoded(&output, &output_encoding),
            "block_size": size,
        }));
    }
    let Some(segments) = options.get("segments").and_then(Value::as_array) else {
        return Err("Segments array is required".into());
    };
    let mut output = input;
    let mut count = 0u64;
    for segment in segments {
        let bounds = match (int_of(segment.get("offset")), int_of(segment.get("size"))) {
            (Some(start), Some(length)) if start >= 0 && length >= 0 => start
                .checked_add(length)
                .filter(|&end| end as u64 <= output.len() as u64)
                .map(|end| (start as usize, end as usize)),
            _ => None,
        };
        let Some((start, end)) = bounds else {
            return Err("Cipher segment is out of bounds".into());
        };
        if start == end {
            continue;
        }
        let iv = bytes::decode(
            segment.get("iv").unwrap_or(&NULL),
            &opt_text(options, "ivEncoding", "base64"),
        )?;
        if iv.len() > size {
            return Err("Segment IV is too long".into());
        }
        let mut padded = vec![0u8; size];
        padded[..iv.len()].copy_from_slice(&iv);
        let key = bytes::decode(
            options.get("key").unwrap_or(&NULL),
            &opt_text(options, "keyEncoding", "hex"),
        )?;
        let clear = crypto::block_cipher(
            false,
            &opt_text(options, "algorithm", "aes"),
            "ctr",
            &key,
            &padded,
            false,
            &output[start..end],
        )?;
        output[start..end].copy_from_slice(&clear);
        count += 1;
    }
    Ok(json!({
        "success": true,
        "data": bytes::encoded(&output, &output_encoding),
        "segments_processed": count,
    }))
}

// ----- url.* -----

/// The fields of a WHATWG `URL` (spec §2.8).
fn parse_url(input: &str, base: Option<&str>) -> Result<Value, String> {
    let parsed = match base {
        Some(base) => url::Url::parse(base).and_then(|base| base.join(input)),
        None => url::Url::parse(input),
    };
    let url = parsed.map_err(|_| "Invalid URL".to_string())?;
    let scheme = url.scheme();
    let hostname = url.host_str().unwrap_or("");
    let port = url.port().map(|port| port.to_string()).unwrap_or_default();
    let host = if port.is_empty() {
        hostname.to_string()
    } else {
        format!("{hostname}:{port}")
    };
    let mut pathname = url.path().to_string();
    if url.has_host() && pathname.is_empty() {
        pathname = "/".into();
    }
    let search = url
        .query()
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let hash = url
        .fragment()
        .filter(|fragment| !fragment.is_empty())
        .map(|fragment| format!("#{fragment}"))
        .unwrap_or_default();
    let origin = if url.has_host() {
        format!("{scheme}://{host}")
    } else {
        "null".to_string()
    };
    Ok(json!({
        "href": url.as_str(),
        "origin": origin,
        "protocol": format!("{scheme}:"),
        "host": host,
        "hostname": hostname,
        "port": port,
        "pathname": pathname,
        "search": search,
        "hash": hash,
        "username": url.username(),
        "password": url.password().unwrap_or(""),
    }))
}

/// `URLSearchParams` entries of `init`: pairs from a list, a map's entries, or a query string.
fn query_entries(init: &Value) -> Vec<(String, String)> {
    match init {
        Value::Array(list) => list
            .iter()
            .filter_map(|entry| match entry.as_array() {
                Some(pair) if pair.len() >= 2 => Some((text(&pair[0]), text(&pair[1]))),
                _ => None,
            })
            .collect(),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.clone(), text(value)))
            .collect(),
        other => {
            let raw = text_or_empty(other);
            let raw = raw.strip_prefix('?').unwrap_or(raw.as_str());
            raw.split('&')
                .filter(|part| !part.is_empty())
                .map(|part| match part.split_once('=') {
                    Some((key, value)) => (decode_form(key), decode_form(value)),
                    None => (decode_form(part), String::new()),
                })
                .collect()
        }
    }
}

/// `URLSearchParams.toString()`.
fn encode_query(entries: &Value) -> String {
    query_entries(entries)
        .iter()
        .map(|(key, value)| format!("{}={}", encode_form(key), encode_form(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Form decoding (`+` is a space); an invalid `%` escape or invalid UTF-8 leaves the text with only `+` changed.
fn decode_form(value: &str) -> String {
    let plain = || value.replace('+', " ");
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let Some(byte) = bytes
                    .get(i + 1..i + 3)
                    .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                    .and_then(|hex| std::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                else {
                    return plain();
                };
                out.push(byte);
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| plain())
}

/// Form encoding: `A-Z a-z 0-9 * - . _` as they are, a space as `+`, the rest `%XX`.
fn encode_form(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte))
            }
            b' ' => out.push('+'),
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

// ----- matching.* -----

/// Lowercase, credits (`(feat. …)`, `[Remastered]`, `(Official …)`) dropped, only `a-z 0-9` and single spaces.
fn normalize_string(value: &str) -> String {
    let chars: Vec<char> = value.to_lowercase().chars().collect();
    let mut kept = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(end) = credit_at(&chars, i) {
            i = end;
            continue;
        }
        kept.push(chars[i]);
        i += 1;
    }
    let letters: String = kept
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == ' ')
        .collect();
    letters.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Where a credit that starts at `start` ends (`\s*[(\[](?:feat\.?|ft\.?|with|remaster(?:ed)?|official).*?[)\]]`).
fn credit_at(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if !matches!(chars.get(i), Some('(' | '[')) {
        return None;
    }
    let after = i + 1;
    let word = ["feat", "ft", "with", "remaster", "official"]
        .iter()
        .find(|word| {
            chars[after..]
                .iter()
                .take(word.len())
                .copied()
                .eq(word.chars())
        })?;
    for (j, c) in chars.iter().enumerate().skip(after + word.len()) {
        match c {
            ')' | ']' => return Some(j + 1),
            // A regular expression's `.` stops at a line's end.
            '\n' | '\r' | '\u{2028}' | '\u{2029}' => return None,
            _ => {}
        }
    }
    None
}

/// Levenshtein similarity (0 to 1) of the trimmed, lowercased strings' UTF-8 bytes.
fn compare_strings(left: &str, right: &str) -> Result<f64, String> {
    let a = left.trim().to_lowercase().into_bytes();
    let b = right.trim().to_lowercase().into_bytes();
    if a.is_empty() || b.is_empty() {
        return Ok(0.0);
    }
    if a.len() > 2000 || b.len() > 2000 {
        return Err("Matching input is too long".into());
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut next = vec![0; b.len() + 1];
        next[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            next[j] = (next[j - 1] + 1).min(row[j] + 1).min(row[j - 1] + cost);
        }
        row = next;
    }
    Ok(1.0 - row[b.len()] as f64 / a.len().max(b.len()) as f64)
}

// ----- gobackend.* -----

/// `<>:"/\|?*` and control characters made `_`, trailing dots and spaces dropped, at most 180 UTF-16 units.
fn sanitize(value: &str) -> String {
    let replaced: String = value
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || (c as u32) < 0x20
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    let mut used = 0;
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        used += c.len_utf16();
        if used > 180 {
            break;
        }
        out.push(c);
    }
    out
}

/// `{key}` in `template` replaced with `values[key]` (or nothing), then the whole name sanitized.
fn build_filename(template: &str, values: Option<&Map<String, Value>>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if close > 0 => {
                let key = &after[..close];
                if let Some(value) = values.and_then(|values| values.get(key)) {
                    out.push_str(&text_or_empty(value));
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    sanitize(&out)
}

/// `{hour, minute, second, timezone, offset (seconds)}` of the computer's clock.
fn local_time() -> Value {
    use chrono::{Offset, Timelike};
    let now = chrono::Local::now();
    json!({
        "hour": now.hour(),
        "minute": now.minute(),
        "second": now.second(),
        "timezone": iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
        "offset": now.offset().fix().local_minus_utc(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up 128-bit value, for the tests only.
    const SAMPLE_HEX: &str = "00112233445566778899aabbccddeeff";
    use crate::net::Permissions;
    use crate::net::testing::{Reply, Seen, Server, network};
    use std::sync::atomic::AtomicUsize;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stash-host-{}-{}",
            std::process::id(),
            crate::hex(&crate::random_bytes(6))
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn host_in(dir: &Path, hooks: Hooks) -> Arc<Host> {
        let (network, _) = network(json!({}));
        let manifest = Manifest {
            raw: json!({}),
            name: "test".into(),
            display_name: "Test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            types: vec!["download_provider".into()],
            permissions: Permissions {
                network: vec!["127.0.0.1".into()],
                storage: false,
                file: true,
                allow_http: true,
            },
            signed_session: None,
            settings: Vec::new(),
            quality_options: Vec::new(),
            global_actions: Vec::new(),
            capabilities: json!({}),
        };
        fs::create_dir_all(dir.join("work")).unwrap();
        Host::new(HostSetup {
            key: "0123456789".into(),
            manifest: Arc::new(manifest),
            root: dir.join("files"),
            grants: vec![dir.join("work")],
            network: Arc::new(network),
            store: Arc::new(Store::new(dir.join("data"), [0; 32])),
            session: None,
            hooks,
            ffmpeg: None,
            app_version: "0.0.1".into(),
        })
    }

    // ----- the sandbox -----

    #[test]
    fn the_sandbox_keeps_paths_in_the_addons_folders() {
        let dir = scratch();
        let root = dir.join("files");
        let grant = dir.join("work");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&grant).unwrap();
        let bases = bases_of(&root, std::slice::from_ref(&grant));
        let check = |raw: Value| sandbox(true, &bases, &raw);
        let path = |p: &Path| json!(p.to_str().unwrap());

        assert_eq!(
            check(json!("a/b.flac")).unwrap(),
            bases[0].join("a").join("b.flac")
        );
        assert_eq!(
            check(json!("./a/./b")).unwrap(),
            bases[0].join("a").join("b")
        );
        assert_eq!(
            check(path(&root.join("x.flac"))).unwrap(),
            bases[0].join("x.flac")
        );
        assert_eq!(check(path(&root)).unwrap(), bases[0]);
        assert_eq!(
            check(path(&grant.join("audio.flac"))).unwrap(),
            bases[1].join("audio.flac")
        );
        // An absolute path may use `..` while it stays inside.
        assert_eq!(
            check(path(&root.join("a").join("..").join("b"))).unwrap(),
            bases[0].join("b")
        );

        for bad in [
            json!("a/../b"),
            json!("..\\b"),
            json!(".."),
            json!(""),
            json!("a\u{0}b"),
            json!(5),
            Value::Null,
        ] {
            assert_eq!(check(bad.clone()).unwrap_err(), INVALID_PATH, "{bad}");
        }
        assert_eq!(
            check(path(&root.join("..").join("elsewhere.flac"))).unwrap_err(),
            OUTSIDE
        );
        assert_eq!(check(path(&dir.join("other.flac"))).unwrap_err(), OUTSIDE);
        // A folder whose name only starts like the root's is not inside it.
        assert_eq!(
            check(path(&dir.join("files-2").join("x"))).unwrap_err(),
            OUTSIDE
        );
        assert_eq!(
            sandbox(false, &bases, &json!("a")).unwrap_err(),
            "File permission denied"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sandbox_refuses_links() {
        let dir = scratch();
        let root = dir.join("files");
        let outside = dir.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, root.join("link"));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&outside, root.join("link"));
        // Making a link may need rights the test lacks (Windows); nothing to check then.
        if linked.is_ok() {
            let bases = bases_of(&root, &[]);
            assert_eq!(
                sandbox(true, &bases, &json!("link/song.flac")).unwrap_err(),
                LINKS
            );
            assert_eq!(sandbox(true, &bases, &json!("link")).unwrap_err(), LINKS);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalizing_never_climbs_above_the_root() {
        let root = std::path::absolute(Path::new("/")).unwrap();
        assert_eq!(
            normalize(&root.join("..").join("a")),
            normalize(&root.join("a"))
        );
        assert_eq!(
            normalize(&root.join("a").join(".").join("b").join("..")),
            normalize(&root.join("a"))
        );
    }

    // ----- files through the host -----

    #[test]
    fn files_are_read_and_written_inside_the_folder() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        let call = |method: &str, args: Value| {
            host.call(method, args.as_array().unwrap().clone()).unwrap()
        };

        assert_eq!(
            call("file.write", json!(["notes/a.txt", "hello"])),
            json!({ "success": true, "bytes_written": 5 })
        );
        assert_eq!(call("file.exists", json!(["notes/a.txt"])), json!(true));
        // Written in place at an offset, the rest kept.
        assert_eq!(
            call(
                "file.writeBytes",
                json!(["notes/a.txt", "WOR", { "encoding": "utf8", "offset": 1 }])
            )["bytes_written"],
            3
        );
        call(
            "file.writeBytes",
            json!(["notes/a.txt", [33], { "append": true }]),
        );
        let read = call("file.read", json!(["notes/a.txt"]));
        assert_eq!(read["data"], "hWORo!");
        assert_eq!(read["size"], 6);
        assert_eq!(read["eof"], true);
        let bytes = call(
            "file.readBytes",
            json!(["notes/a.txt", { "offset": 1, "length": 3, "encoding": "hex" }]),
        );
        assert_eq!(bytes["data"], "574f52");
        assert_eq!(bytes["bytes_read"], 3);
        assert_eq!(bytes["eof"], false);
        assert_eq!(
            call("file.getSize", json!(["notes/a.txt"])),
            json!({ "success": true, "size": 6 })
        );
        // Emptied first, then written at the offset.
        call(
            "file.writeBytes",
            json!(["notes/a.txt", "eg==", { "truncate": true }]),
        );
        assert_eq!(call("file.read", json!(["notes/a.txt"]))["data"], "z");

        call("file.copy", json!(["notes/a.txt", "copies/b.txt"]));
        call("file.move", json!(["copies/b.txt", "moved/c.txt"]));
        assert_eq!(call("file.exists", json!(["copies/b.txt"])), json!(false));
        assert_eq!(call("file.read", json!(["moved/c.txt"]))["data"], "z");
        assert_eq!(
            call("file.delete", json!(["moved/c.txt"])),
            json!({ "success": true })
        );
        assert_eq!(call("file.exists", json!(["moved/c.txt"])), json!(false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_failures_are_answered_not_thrown() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        let call = |method: &str, args: Value| {
            host.call(method, args.as_array().unwrap().clone()).unwrap()
        };
        assert_eq!(
            call("file.read", json!(["../escape.txt"])),
            json!({ "success": false, "error": INVALID_PATH, "error_type": "download_error" })
        );
        assert_eq!(call("file.exists", json!(["../escape.txt"])), json!(false));
        assert_eq!(
            call("file.rename", json!(["a"]))["error"],
            "Unknown file API"
        );
        assert_eq!(
            call("file.readBytes", json!(["a", { "length": -1 }]))["error"],
            "Invalid binary read range"
        );
        assert_eq!(
            call("file.downloadSegments", json!([[], "x"]))["error"],
            "Invalid download segments"
        );
        assert_eq!(
            call(
                "file.transformPatternedBlocks",
                json!(["a", "b", { "segmentSize": 0 }])
            )["error"],
            "Invalid patterned transform options"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_kinds_are_dropped_from_messages() {
        assert_eq!(strip_error_kind("StateError: no room"), "no room");
        assert_eq!(strip_error_kind("FileSystemException: gone"), "gone");
        assert_eq!(strip_error_kind("Error: kept"), "Error: kept");
        assert_eq!(strip_error_kind("Bad state: kept"), "Bad state: kept");
        assert_eq!(strip_error_kind("plain"), "plain");
    }

    #[test]
    fn content_ranges_are_read_strictly() {
        assert_eq!(content_range("bytes 6-10/11"), Some((6, 10, 11)));
        assert_eq!(content_range("bytes 0-99/*"), None);
        assert_eq!(content_range("bytes -1-2/3"), None);
        assert_eq!(content_range("items 0-1/2"), None);
    }

    // ----- downloads -----

    #[test]
    fn a_download_is_retried_and_reported() {
        let dir = scratch();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let server = Server::start(move |_seen: &Seen| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                Reply::new(503, "busy")
            } else {
                Reply::new(200, "hello world").with("ETag", "\"v1\"")
            }
        });
        let seen: Arc<Mutex<Vec<Transfer>>> = Arc::default();
        let log = Arc::clone(&seen);
        let hooks = Hooks {
            transfer: Some(Arc::new(move |transfer: &Transfer| {
                log.lock().unwrap().push(transfer.clone())
            })),
            ..Hooks::default()
        };
        let host = host_in(&dir, hooks);
        let url = format!("{}/song", server.base());
        let answer = host
            .call(
                "file.download",
                vec![json!(url), json!("song.flac"), json!({})],
            )
            .unwrap();
        assert_eq!(
            answer,
            json!({ "success": true, "path": "song.flac", "size": 11, "attempts": 2 })
        );
        assert_eq!(
            fs::read(dir.join("files").join("song.flac")).unwrap(),
            b"hello world"
        );
        assert!(!dir.join("files").join("song.flac.part").exists());
        let seen = seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert!(last.done);
        assert_eq!((last.written, last.total), (11, 11));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_is_not_retried_after_a_404() {
        let dir = scratch();
        let server = Server::start(|_seen: &Seen| Reply::new(404, "no"));
        let host = host_in(&dir, Hooks::default());
        let answer = host
            .call(
                "file.download",
                vec![json!(format!("{}/x", server.base())), json!("x.flac")],
            )
            .unwrap();
        assert_eq!(answer["error"], "Download returned HTTP 404");
        assert_eq!(answer["error_type"], "download_error");
        assert_eq!(server.seen().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_resumes_from_its_checkpoint() {
        let dir = scratch();
        let server =
            Server::start(
                |seen: &Seen| match (seen.header("Range"), seen.header("If-Range")) {
                    (Some("bytes=6-"), Some("\"v1\"")) => Reply::new(206, "world")
                        .with("Content-Range", "bytes 6-10/11")
                        .with("ETag", "\"v1\""),
                    _ => Reply::new(200, "hello world").with("ETag", "\"v1\""),
                },
            );
        let host = host_in(&dir, Hooks::default());
        let url = format!("{}/song", server.base());
        let files = dir.join("files");
        fs::write(files.join("song.flac.part"), "hello ").unwrap();
        let checkpoint = json!({ "urlHash": crate::hex(&crypto::sha256(url.as_bytes())), "validator": "\"v1\"" });
        fs::write(files.join("song.flac.checkpoint"), checkpoint.to_string()).unwrap();
        let answer = host
            .call(
                "file.download",
                vec![
                    json!(url),
                    json!("song.flac"),
                    json!({ "persistentCheckpoint": true }),
                ],
            )
            .unwrap();
        assert_eq!(answer["resumed"], true, "{answer}");
        assert_eq!(answer["size"], 11);
        assert_eq!(fs::read(files.join("song.flac")).unwrap(), b"hello world");
        assert!(!files.join("song.flac.checkpoint").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chunked_download_asks_for_one_range_at_a_time() {
        // Three and a bit of the smallest pieces taken (64 KiB).
        let whole: Arc<Vec<u8>> = Arc::new((0..200 * 1024).map(|i| (i % 251) as u8).collect());
        let served = Arc::clone(&whole);
        let dir = scratch();
        let server = Server::start(move |seen: &Seen| {
            let whole = served.as_slice();
            let asked = seen.header("Range").and_then(|range| {
                let (from, to) = range.strip_prefix("bytes=")?.split_once('-')?;
                Some((from.parse::<usize>().ok()?, to.parse::<usize>().ok()?))
            });
            match asked {
                Some((from, to)) if from < whole.len() => {
                    let to = to.min(whole.len() - 1);
                    Reply::new(206, &whole[from..=to])
                        .with(
                            "Content-Range",
                            &format!("bytes {from}-{to}/{}", whole.len()),
                        )
                        .with("ETag", "\"v1\"")
                }
                _ => Reply::new(200, whole).with("ETag", "\"v1\""),
            }
        });
        let live: Arc<Mutex<Vec<(u64, u64)>>> = Arc::default();
        let heard = Arc::clone(&live);
        let hooks = Hooks {
            transfer: Some(Arc::new(move |step: &Transfer| {
                heard.lock().unwrap().push((step.written, step.total));
            })),
            ..Hooks::default()
        };
        let host = host_in(&dir, hooks);
        let url = format!("{}/song", server.base());
        let answer = host
            .call(
                "file.download",
                vec![
                    json!(url),
                    json!("song.opus"),
                    json!({ "chunked": true, "chunkSize": 1 }),
                ],
            )
            .unwrap();
        assert_eq!(answer["size"], whole.len(), "{answer}");
        assert_eq!(
            &fs::read(dir.join("files").join("song.opus")).unwrap(),
            whole.as_ref()
        );
        let ranges: Vec<String> = server
            .seen()
            .iter()
            .filter_map(|seen| seen.header("Range").map(str::to_string))
            .collect();
        let piece = 64 * 1024;
        let asked: Vec<String> = (0..4)
            .map(|n| format!("bytes={}-{}", n * piece, (n + 1) * piece - 1))
            .collect();
        assert_eq!(ranges, asked);
        // Live readers know the whole length from the first answer.
        assert!(
            live.lock()
                .unwrap()
                .iter()
                .all(|&(_, total)| total == whole.len() as u64)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn segments_are_joined_in_order() {
        let dir = scratch();
        let server = Server::start(|seen: &Seen| {
            let body = match seen.path() {
                "/s0" => "aa",
                "/s1" => "bb",
                "/s2" => "cc",
                _ => "",
            };
            Reply::new(200, body)
        });
        let host = host_in(&dir, Hooks::default());
        let base = server.base();
        let segments = json!([
            format!("{base}/s0"),
            { "url": format!("{base}/s1"), "headers": { "X-Piece": "1" } },
            format!("{base}/s2"),
        ]);
        let answer = host
            .call(
                "file.downloadSegments",
                vec![
                    segments,
                    json!("joined.m4a"),
                    json!({ "maxParallel": 2, "headers": { "X-All": "yes" } }),
                ],
            )
            .unwrap();
        assert_eq!(
            answer,
            json!({ "success": true, "path": "joined.m4a", "size": 6, "segments": 3 })
        );
        let files = dir.join("files");
        assert_eq!(fs::read(files.join("joined.m4a")).unwrap(), b"aabbcc");
        assert!(!files.join("joined.m4a.segments").exists());
        let seen = server.seen();
        assert!(
            seen.iter()
                .any(|s| s.path() == "/s1" && s.header("X-Piece") == Some("1"))
        );
        assert!(seen.iter().all(|s| s.header("X-All") == Some("yes")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn patterned_blocks_round_trip() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        let files = dir.join("files");
        let plain: Vec<u8> = (0..10_240u32).map(|i| (i % 251) as u8).collect();
        fs::write(files.join("in.bin"), &plain).unwrap();
        let options = |operation: &str| {
            json!({
                "operation": operation,
                "algorithm": "blowfish",
                "mode": "cbc",
                "key": SAMPLE_HEX,
                "keyEncoding": "hex",
                "iv": "0001020304050607",
                "ivEncoding": "hex",
                "padding": "none",
                "segmentSize": 2048,
                "transformEvery": 3,
                "transformOffset": 0,
            })
        };
        let sealed = host
            .call(
                "file.transformPatternedBlocks",
                vec![json!("in.bin"), json!("sealed.bin"), options("encrypt")],
            )
            .unwrap();
        assert_eq!(
            sealed,
            json!({
                "success": true,
                "path": "sealed.bin",
                "bytes_processed": 10_240,
                "segments_processed": 5,
                "segments_transformed": 2,
            })
        );
        let middle = fs::read(files.join("sealed.bin")).unwrap();
        assert_ne!(&middle[..2048], &plain[..2048]);
        assert_eq!(&middle[2048..6144], &plain[2048..6144]);
        assert_ne!(&middle[6144..8192], &plain[6144..8192]);
        host.call(
            "file.transformPatternedBlocks",
            vec![json!("sealed.bin"), json!("out.bin"), options("decrypt")],
        )
        .unwrap();
        assert_eq!(fs::read(files.join("out.bin")).unwrap(), plain);
        assert!(!files.join("out.bin.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ----- network, keeping, ffmpeg -----

    #[test]
    fn http_failures_are_answered_and_json_bodies_typed() {
        let dir = scratch();
        let server = Server::start(|_seen: &Seen| Reply::json(200, &json!({ "fine": true })));
        let host = host_in(&dir, Hooks::default());
        let refused = host
            .call(
                "http.request",
                vec![
                    json!("https://elsewhere.invalid/"),
                    json!({ "method": "GET" }),
                ],
            )
            .unwrap();
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["status"], 0);
        assert!(!refused["error"].as_str().unwrap().is_empty());
        let answer = host
            .call(
                "http.request",
                vec![
                    json!(format!("{}/p", server.base())),
                    json!({ "method": "POST", "body": "{}" }),
                ],
            )
            .unwrap();
        assert_eq!(answer["ok"], true);
        assert_eq!(
            server.seen()[0].header("Content-Type"),
            Some("application/json")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_addon_may_not_use_is_refused() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        assert_eq!(
            host.call("storage.get", vec![json!("k")]).unwrap_err(),
            "Storage permission denied"
        );
        assert_eq!(
            host.call("credentials.set", vec![json!("k"), json!(1)])
                .unwrap_err(),
            "Storage permission denied"
        );
        assert_eq!(
            host.call("session.status", vec![]).unwrap_err(),
            "Signed sessions are not configured"
        );
        assert_eq!(
            host.call("nothing.here", vec![]).unwrap_err(),
            "Unsupported host API: nothing.here"
        );
        assert_eq!(
            host.call("gobackend.unknown", vec![]).unwrap_err(),
            "Unsupported host API: gobackend.unknown"
        );
        assert_eq!(
            host.call("url.other", vec![]).unwrap_err(),
            "Unsupported host API: url.other"
        );
        assert_eq!(
            host.call("callback", vec![]).unwrap_err(),
            "Unsupported host API: callback"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ffmpeg_answers_its_failures() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        fs::write(dir.join("files").join("in.m4a"), b"not empty").unwrap();
        let call = |args: Vec<Value>| host.call("ffmpeg.convert", args).unwrap();
        assert_eq!(
            call(vec![json!("in.m4a")])["error"],
            "Expected input and output paths"
        );
        assert_eq!(
            call(vec![json!("in.m4a"), json!("out.flac")]),
            json!({ "success": false, "error": "FFmpeg is not available" })
        );
        assert_eq!(
            call(vec![
                json!("in.m4a"),
                json!("out.mp3"),
                json!({ "codec": "mp3" })
            ])["error"],
            "Unsupported audio conversion"
        );
        assert_eq!(
            host.call("ffmpeg.execute", vec![]).unwrap(),
            json!({ "success": false, "error": "Unsupported FFmpeg operation" })
        );
        assert_eq!(
            host.call("ffmpeg.getInfo", vec![json!("in.m4a")]).unwrap()["success"],
            false
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // ----- cancelling -----

    #[test]
    fn a_cancelled_host_stops_waiting() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        assert_eq!(
            host.call("utils.sleep", vec![json!(1)]).unwrap(),
            json!(true)
        );
        assert_eq!(
            host.call("utils.isDownloadCancelled", vec![]).unwrap(),
            json!(false)
        );
        let stopper = Arc::clone(&host);
        let started = Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            stopper.abort();
        });
        assert_eq!(
            host.call("utils.sleep", vec![json!(5_000)]).unwrap_err(),
            CANCELLED
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(host.call("log", vec![]).unwrap_err(), CANCELLED);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn utils_that_describe_the_host() {
        let dir = scratch();
        let host = host_in(&dir, Hooks::default());
        let call = |method: &str| host.call(method, vec![]).unwrap();
        assert_eq!(call("utils.appVersion"), json!(crate::COMPAT_VERSION));
        assert_eq!(
            call("utils.appUserAgent"),
            json!("Stash/0.0.1 Morneye-compatible/5.3.0")
        );
        assert_eq!(call("utils.randomUserAgent"), json!(RANDOM_USER_AGENT));
        assert!(call("utils.getResolutionRemainingMs").as_u64().unwrap() <= 60_000);
        assert_eq!(
            host.call(
                "gobackend.checkISRCExists",
                vec![json!("dir"), json!("USRC17607839")]
            )
            .unwrap(),
            json!({ "exists": false, "file_path": "" })
        );
        assert_eq!(
            call("gobackend.getLyricsLRC"),
            json!({ "lyrics": "", "lrc": "" })
        );
        let time = call("gobackend.getLocalTime");
        assert!(time["hour"].as_i64().unwrap() < 24);
        assert!(time["minute"].as_i64().unwrap() < 60);
        assert!(time["offset"].is_i64());
        assert!(time["timezone"].is_string());
        assert_eq!(call("progress"), json!(true));
        assert_eq!(call("log"), json!(true));
        let _ = fs::remove_dir_all(&dir);
    }

    // ----- calls that need nothing of the host -----

    fn pure(method: &str, args: Value) -> Value {
        stateless(method, args.as_array().unwrap())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn urls_are_parsed_like_whatwg() {
        let parsed = parse_url("https://user:pw@Example.com:8443/a/b?x=1&y=2#frag", None).unwrap();
        assert_eq!(parsed["protocol"], "https:");
        assert_eq!(parsed["hostname"], "example.com");
        assert_eq!(parsed["port"], "8443");
        assert_eq!(parsed["host"], "example.com:8443");
        assert_eq!(parsed["origin"], "https://example.com:8443");
        assert_eq!(parsed["pathname"], "/a/b");
        assert_eq!(parsed["search"], "?x=1&y=2");
        assert_eq!(parsed["hash"], "#frag");
        assert_eq!(parsed["username"], "user");
        assert_eq!(parsed["password"], "pw");
        let plain = parse_url("https://example.com:443", None).unwrap();
        assert_eq!(
            (plain["port"].as_str(), plain["host"].as_str()),
            (Some(""), Some("example.com"))
        );
        assert_eq!(
            (plain["pathname"].as_str(), plain["search"].as_str()),
            (Some("/"), Some(""))
        );
        let joined = parse_url("../c?q", Some("https://example.com/a/b/")).unwrap();
        assert_eq!(joined["href"], "https://example.com/a/c?q");
        assert_eq!(
            parse_url("spotify:track:1", None).unwrap()["origin"],
            "null"
        );
        assert_eq!(
            parse_url("no scheme here", None).unwrap_err(),
            "Invalid URL"
        );
        assert_eq!(
            pure("url.parse", json!(["https://example.com/x", null]))["pathname"],
            "/x"
        );
    }

    #[test]
    fn query_strings_parse_and_encode() {
        let pairs = |init: Value| {
            query_entries(&init)
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            pairs(json!("?a=1&b=x+y&c=%E2%9C%93&d&=e&f=%zz+1&&g=%41%2")),
            ["a=1", "b=x y", "c=✓", "d=", "=e", "f=%zz 1", "g=%41%2"]
        );
        assert_eq!(
            pairs(json!([["k", 1], ["only"], ["x", null]])),
            ["k=1", "x=null"]
        );
        assert_eq!(pairs(json!({ "a": "b" })), ["a=b"]);
        assert_eq!(pairs(Value::Null), Vec::<String>::new());
        assert_eq!(
            encode_query(&json!([["a b", "c&d"], ["é", "*-._~"]])),
            "a+b=c%26d&%C3%A9=*-._%7E"
        );
        assert_eq!(pure("url.encode", json!(["x=1 2"])), json!("x=1+2"));
        assert_eq!(pure("url.query", json!(["a=1"])), json!([["a", "1"]]));
    }

    #[test]
    fn matching_normalizes_and_compares() {
        assert_eq!(
            normalize_string("Song Name (feat. Someone) [Remastered 2011]  - Live!"),
            "song name live"
        );
        assert_eq!(normalize_string("Track (Official Video)"), "track");
        assert_eq!(normalize_string("A (Live) [with Band]"), "a live");
        // A credit must close on its line.
        assert_eq!(
            normalize_string("Mix (with friends\n) end"),
            "mix with friends end"
        );
        assert_eq!(compare_strings(" Hello", "hello ").unwrap(), 1.0);
        assert!((compare_strings("abc", "abd").unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(compare_strings("", "x").unwrap(), 0.0);
        assert_eq!(
            compare_strings(&"x".repeat(2001), "x").unwrap_err(),
            "Matching input is too long"
        );
        assert_eq!(
            pure("matching.normalizeString", json!(["A (ft. B)"])),
            json!("a")
        );
        assert_eq!(
            pure("matching.compareStrings", json!(["a", "a"])),
            json!(1.0)
        );
    }

    #[test]
    fn file_names_are_sanitized_and_built() {
        assert_eq!(
            sanitize("a<b>:c\"d/e\\f|g?h*i\u{1}j. . "),
            "a_b__c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize(&"x".repeat(200)).len(), 180);
        assert_eq!(sanitize(&"😀".repeat(100)).chars().count(), 90);
        let values = json!({ "artist": "AC/DC", "title": "T", "ext": "flac", "n": 3 });
        assert_eq!(
            build_filename("{artist} - {title}{missing}.{ext}", values.as_object()),
            "AC_DC - T.flac"
        );
        assert_eq!(build_filename("{n}{}{open", values.as_object()), "3{}{open");
        assert_eq!(pure("gobackend.sanitizeFilename", json!([null])), json!(""));
        assert_eq!(
            pure("gobackend.buildFilename", json!(["{a}?", { "a": "x" }])),
            json!("x_")
        );
    }

    #[test]
    fn hmac_argument_orders_differ() {
        let message = "what do ya want for nothing?";
        // hmacSHA1(key, data), raw bytes back.
        let sha1 = pure("utils.hmacSHA1", json!(["Jefe", message]));
        assert_eq!(sha1, json!(crypto::hmac_sha1(b"Jefe", message.as_bytes())));
        let from_bytes = pure(
            "utils.hmacSHA1",
            json!([b"Jefe".to_vec(), { "__bytes": message.as_bytes() }]),
        );
        assert_eq!(from_bytes, sha1);
        // hmacSHA256(message, key), hex.
        assert_eq!(
            pure("utils.hmacSHA256", json!([message, "Jefe"])),
            json!("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        assert_eq!(
            pure("utils.hmacSHA256Base64", json!([message, "Jefe"])),
            json!(bytes::base64(&crypto::hmac_sha256(
                b"Jefe",
                message.as_bytes()
            )))
        );
    }

    #[test]
    fn small_utils() {
        assert_eq!(pure("utils.base64Encode", json!(["é"])), json!("w6k="));
        assert_eq!(pure("utils.base64Decode", json!(["w6k="])), json!("é"));
        assert_eq!(pure("utils.base64Decode", json!(["%%%"])), json!(""));
        assert_eq!(
            pure("utils.md5", json!(["abc"])),
            json!("900150983cd24fb0d6963f7d28e17f72")
        );
        assert_eq!(
            pure("encode", json!(["hé"])),
            json!({ "__bytes": [104, 195, 169] })
        );
        assert_eq!(pure("decode", json!([[104, 195, 169]])), json!("hé"));
        assert_eq!(
            pure("base64Bytes", json!([{ "__bytes": [1, 2] }])),
            json!("AQI=")
        );
        assert_eq!(pure("decodeBase64Bytes", json!(["AQI="])), json!([1, 2]));
        let key = pure("utils.generateKey", json!([16]));
        assert_eq!(key["hex"].as_str().unwrap().len(), 32);
        assert_eq!(
            pure("utils.generateKey", json!([0]))["error"],
            "Invalid key length"
        );
        let sealed = pure("utils.encrypt", json!(["secret", "k"]));
        assert_eq!(
            pure("utils.decrypt", json!([sealed["data"], "k"])),
            json!({ "success": true, "data": "secret" })
        );
        assert_eq!(
            pure("utils.decrypt", json!([sealed["data"], "other"]))["success"],
            false
        );
    }

    #[test]
    fn block_ciphers_through_utils() {
        let options = json!({
            "algorithm": "aes",
            "key": "0123456789abcdef",
            "iv": "fedcba9876543210",
            "padding": "pkcs7",
            "inputEncoding": "utf8",
        });
        let sealed = pure("utils.encryptBlockCipher", json!(["a secret", options]));
        assert_eq!(sealed["block_size"], 16);
        let mut back = options.clone();
        back["inputEncoding"] = json!("base64");
        back["outputEncoding"] = json!("utf8");
        let opened = pure("utils.decryptBlockCipher", json!([sealed["data"], back]));
        assert_eq!(
            opened,
            json!({ "success": true, "data": "a secret", "block_size": 16 })
        );
        let failed = pure(
            "utils.encryptBlockCipher",
            json!(["x", { "key": "k", "iv": "i" }]),
        );
        assert_eq!(failed["success"], false);

        // CTR pieces of a buffer, each from its own short IV padded with zeros.
        let key = [7u8; 16];
        let short_iv = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut padded = short_iv.to_vec();
        padded.resize(16, 0);
        let plain: Vec<u8> = (0..32).collect();
        let mut mixed = plain.clone();
        let sealed_piece =
            crypto::block_cipher(true, "aes", "ctr", &key, &padded, false, &plain[4..20]).unwrap();
        mixed[4..20].copy_from_slice(&sealed_piece);
        let answer = pure(
            "utils.decryptCTRSegments",
            json!([
                bytes::base64(&mixed),
                {
                    "key": crate::hex(&key),
                    "segments": [
                        { "offset": 4, "size": 16, "iv": bytes::base64(&short_iv) },
                        { "offset": 0, "size": 0, "iv": "" },
                    ],
                },
            ]),
        );
        assert_eq!(answer["segments_processed"], 1);
        assert_eq!(bytes::decode(&answer["data"], "base64").unwrap(), plain);
        let outside = pure(
            "utils.decryptCTRSegments",
            json!([bytes::base64(&mixed), { "key": "00", "segments": [{ "offset": 30, "size": 4 }] }]),
        );
        assert_eq!(outside["error"], "Cipher segment is out of bounds");
        let missing = pure(
            "utils.decryptCTRSegments",
            json!([bytes::base64(&mixed), {}]),
        );
        assert_eq!(missing["error"], "Segments array is required");
    }
}
