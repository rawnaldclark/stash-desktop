//! The relay: the engine reads an add-on's stream of a library song through the core (`/relay/{token}`), and the
//! core keeps a copy of what passes. The next time the song is played it comes from that copy: no connection is
//! needed and the add-on is not asked (its allowance is the listener's). Nothing is fetched twice: a copy is written
//! only from the engine's own reading of the stream, from its start to its end; a stream the engine seeks in before
//! it has read it all is relayed and not kept.
//!
//! Also the routes for the copies: `GET /api/played` (how many, how much room), `POST /api/played` (`cap_mb`: how
//! much room they may take, 0 for none) and `POST /api/played/clear`.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::downloads;
use stash_core::played::{self, PlayedCopy};
use stash_core::player::Located;
use tiny_http::{Header, Method, Request, Response, StatusCode};

use crate::serve::{Reply, State, body_json, now_ms, reply_json};

/// How long a relay link is answered: the engine opens it again to seek, until the song is over.
const ANSWERED_FOR: Duration = Duration::from_secs(12 * 3600);
/// How many links are kept at most (the oldest go first).
const MOST_LINKS: usize = 64;

/// The streams being relayed, and the folder the copies go to.
pub(crate) struct Relays {
    links: Mutex<HashMap<String, Arc<Link>>>,
    folder: PathBuf,
    keys: RandomState,
    made: AtomicU64,
}

/// A stream the engine may ask for: where it is, which song it is, and where it came from.
struct Link {
    url: String,
    id: i64,
    tier: Option<String>,
    addon: Option<String>,
    at: Instant,
    /// A copy is being written from one of the link's answers (one at a time).
    copying: Arc<AtomicBool>,
}

impl Relays {
    /// The relays, keeping copies in `folder`. What a run before left half written there is deleted.
    pub(crate) fn new(folder: PathBuf, library: &Library) -> Self {
        played::tidy(&folder, &library.played_files().unwrap_or_default());
        Self {
            links: Mutex::new(HashMap::new()),
            folder,
            keys: RandomState::new(),
            made: AtomicU64::new(0),
        }
    }

    /// The song `id`'s copy and its file: `None` when none is kept (one whose file is gone is forgotten).
    pub(crate) fn copy(&self, library: &Mutex<Library>, id: i64) -> Option<(PathBuf, PlayedCopy)> {
        let mut library = library.lock().unwrap();
        let copy = library.played_copy(id, now_ms()).ok().flatten()?;
        let path = self.folder.join(&copy.file);
        if path.is_file() {
            return Some((path, copy));
        }
        let _ = library.drop_played_copy(id);
        None
    }

    /// The song `id`'s copy, to be played.
    pub(crate) fn copy_of(&self, library: &Mutex<Library>, id: i64) -> Option<Located> {
        self.copy(library, id).map(|(path, copy)| Located {
            locator: path.to_string_lossy().into_owned(),
            tier: copy.tier,
            addon: None,
        })
    }

    /// The address the engine is given for `located`, the stream of the song `id`: the relay's, on this computer.
    pub(crate) fn link(&self, port: u16, id: i64, located: Located) -> Located {
        // Hard to guess (the hasher's keys are random for each run), and never the same twice.
        let token = {
            let mut hasher = self.keys.build_hasher();
            hasher.write_u64(self.made.fetch_add(1, Ordering::Relaxed));
            hasher.write_i64(id);
            let first = hasher.finish();
            hasher.write_i64(now_ms());
            format!("{first:016x}{:016x}", hasher.finish())
        };
        let mut links = self.links.lock().unwrap();
        links.retain(|_, link| link.at.elapsed() < ANSWERED_FOR);
        while links.len() >= MOST_LINKS {
            let Some(oldest) = links
                .iter()
                .min_by_key(|(_, link)| link.at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            links.remove(&oldest);
        }
        links.insert(
            token.clone(),
            Arc::new(Link {
                url: located.locator,
                id,
                tier: located.tier.clone(),
                addon: located.addon.clone(),
                at: Instant::now(),
                copying: Arc::new(AtomicBool::new(false)),
            }),
        );
        Located {
            locator: format!("http://127.0.0.1:{port}/relay/{token}"),
            ..located
        }
    }

    /// Answers the engine's request for a relay link, on a thread of its own (a stream stays open while it plays).
    pub(crate) fn answer(self: &Arc<Self>, library: Arc<Mutex<Library>>, request: Request) {
        let relays = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("stash-relay".into())
            .spawn(move || relays.relay(&library, request));
        if let Err(e) = spawned {
            eprintln!("the relay could not start: {e}");
        }
    }

    fn relay(&self, library: &Arc<Mutex<Library>>, request: Request) {
        let plain = |status: u16, text: &str| {
            Response::from_string(text).with_status_code(StatusCode(status))
        };
        if *request.method() != Method::Get {
            let _ = request.respond(plain(405, "only GET"));
            return;
        }
        let link = request
            .url()
            .strip_prefix("/relay/")
            .and_then(|token| self.links.lock().unwrap().get(token).cloned());
        let Some(link) = link else {
            let _ = request.respond(plain(404, "no such stream"));
            return;
        };
        let range = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Range"))
            .map(|h| h.value.as_str().to_string());
        let upstream = match downloads::open_stream(&link.url, range.as_deref()) {
            Ok(upstream) => upstream,
            Err(trouble) => {
                let why = match trouble {
                    downloads::Trouble::Failed(why) => why,
                    downloads::Trouble::Cancelled => "cancelled".into(),
                };
                let _ = request.respond(plain(502, &why));
                return;
            }
        };
        let mut headers = Vec::new();
        let mut add = |name: &str, value: &str| {
            if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                headers.push(h);
            }
        };
        if let Some(kind) = &upstream.content_type {
            add("Content-Type", kind);
        }
        if let Some(range) = &upstream.range {
            add("Content-Range", range);
        }
        if upstream.ranges {
            add("Accept-Ranges", "bytes");
        }
        // A copy is written from an answer that holds the whole stream from its start, when there is room for it.
        let from_start = range.as_deref().is_none_or(|r| r.trim() == "bytes=0-");
        let whole = upstream.status == 200
            || (upstream.status == 206 && upstream.range.as_deref().is_some_and(holds_all));
        let cap = library.lock().unwrap().played_cap_bytes().unwrap_or(0);
        let copying = from_start
            && whole
            && cap > 0
            && upstream.length.is_none_or(|n| n <= cap)
            && !link.copying.swap(true, Ordering::AcqRel);
        let length = upstream.length.and_then(|n| usize::try_from(n).ok());
        let status = StatusCode(upstream.status);
        if copying {
            let part = self.folder.join(format!(
                "{}-{:x}.part",
                link.id,
                self.made.fetch_add(1, Ordering::Relaxed)
            ));
            let body = Tee::new(
                upstream.body,
                part,
                upstream.length,
                Keep {
                    library: Arc::clone(library),
                    folder: self.folder.clone(),
                    link: Arc::clone(&link),
                },
            );
            let _ = request.respond(Response::new(status, headers, body, length, None));
        } else {
            let _ = request.respond(Response::new(status, headers, upstream.body, length, None));
        }
    }
}

/// Whether a `Content-Range` answer ("bytes 0-999/1000") holds the whole stream.
fn holds_all(range: &str) -> bool {
    let Some(rest) = range.trim().strip_prefix("bytes 0-") else {
        return false;
    };
    let Some((end, total)) = rest.split_once('/') else {
        return false;
    };
    match (end.parse::<u64>(), total.parse::<u64>()) {
        (Ok(end), Ok(total)) => end + 1 == total,
        _ => false,
    }
}

/// What keeping a finished copy needs.
struct Keep {
    library: Arc<Mutex<Library>>,
    folder: PathBuf,
    link: Arc<Link>,
}

/// The stream as the engine reads it, written to a file as it passes. Whole, the file is kept as the song's copy;
/// cut short (the engine stopped reading, or the server stopped answering), it is deleted.
struct Tee {
    body: Box<dyn Read + Send>,
    out: Option<BufWriter<File>>,
    part: PathBuf,
    head: Vec<u8>,
    got: u64,
    expected: Option<u64>,
    keep: Keep,
}

impl Tee {
    fn new(body: Box<dyn Read + Send>, part: PathBuf, expected: Option<u64>, keep: Keep) -> Self {
        let out = std::fs::create_dir_all(&keep.folder)
            .and_then(|_| File::create(&part))
            .ok()
            .map(|file| BufWriter::with_capacity(256 * 1024, file));
        if out.is_none() {
            keep.link.copying.store(false, Ordering::Release);
        }
        Self {
            body,
            out,
            part,
            head: Vec::with_capacity(64),
            got: 0,
            expected,
            keep,
        }
    }

    /// Stops writing and deletes what was written.
    fn abandon(&mut self) {
        if self.out.take().is_some() {
            let _ = std::fs::remove_file(&self.part);
            self.keep.link.copying.store(false, Ordering::Release);
        }
    }

    /// The stream ended: a whole audio file is kept as the song's copy, room is made, and anything else is deleted.
    fn finish(&mut self) {
        let Some(mut out) = self.out.take() else {
            return;
        };
        self.keep.link.copying.store(false, Ordering::Release);
        let flushed = out.flush().is_ok();
        drop(out);
        let kind = downloads::kind_of(&self.head);
        let whole = self.expected.is_none_or(|n| n == self.got) && self.got > 0;
        let (Some(kind), true, true) = (kind, whole, flushed) else {
            let _ = std::fs::remove_file(&self.part);
            return;
        };
        let link = &self.keep.link;
        let name = format!("{}.{kind}", link.id);
        let file = self.keep.folder.join(&name);
        if std::fs::rename(&self.part, &file).is_err() {
            let _ = std::fs::remove_file(&self.part);
            return;
        }
        let kept = self.keep.library.lock().unwrap().keep_played_copy(
            link.id,
            &name,
            self.got,
            link.tier.as_deref(),
            link.addon.as_deref(),
            now_ms(),
        );
        match kept {
            Ok((_, gone)) => played::delete_files(&self.keep.folder, &gone),
            Err(_) => {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
}

impl Read for Tee {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = match self.body.read(buf) {
            Ok(n) => n,
            Err(e) => {
                self.abandon();
                return Err(e);
            }
        };
        if n == 0 {
            self.finish();
            return Ok(0);
        }
        if let Some(out) = self.out.as_mut() {
            if self.head.len() < 64 {
                let take = (64 - self.head.len()).min(n);
                self.head.extend_from_slice(&buf[..take]);
            }
            // Not audio (a web page, an error): passed on to the engine, not kept.
            let not_audio = self.head.len() >= 12 && downloads::kind_of(&self.head).is_none();
            if not_audio || out.write_all(&buf[..n]).is_err() {
                self.abandon();
            } else {
                self.got += n as u64;
                if self.expected == Some(self.got) {
                    self.finish();
                }
            }
        }
        Ok(n)
    }
}

impl Drop for Tee {
    fn drop(&mut self) {
        // The engine stopped reading before the end: what was written is not a whole song.
        self.abandon();
    }
}

/// The copies' routes. `None`: the path is not one of them.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if path != "/api/played" && path != "/api/played/clear" {
        return Ok(None);
    }
    let room = |state: &State| -> Result<Value> {
        Ok(json!(state.library.lock().unwrap().played_room()?))
    };
    match (request.method(), path) {
        (Method::Get, "/api/played") => Ok(Some(reply_json(200, room(state)?))),
        (Method::Post, "/api/played") => {
            let body = body_json(request)?;
            let Some(cap_mb) = body.get("cap_mb").and_then(Value::as_u64) else {
                return Ok(Some(crate::serve::refuse(400, "cap_mb is needed")));
            };
            let gone = state
                .library
                .lock()
                .unwrap()
                .set_played_cap(cap_mb.min(1 << 20))?;
            played::delete_files(state.relays.folder(), &gone);
            Ok(Some(reply_json(200, room(state)?)))
        }
        (Method::Post, "/api/played/clear") => {
            let gone = state.library.lock().unwrap().clear_played_copies()?;
            played::delete_files(state.relays.folder(), &gone);
            Ok(Some(reply_json(200, room(state)?)))
        }
        _ => Ok(None),
    }
}

impl Relays {
    /// Where the copies are kept.
    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }
}
