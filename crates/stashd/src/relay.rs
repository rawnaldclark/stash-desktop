//! The relay: the engine reads an add-on's stream of a library song through the core (`/relay/{token}`), and the
//! core keeps a copy of what passes. The next time the song is played it comes from that copy: no connection is
//! needed and the add-on is not asked (its allowance is the listener's). Nothing is fetched twice: a copy is written
//! only from the engine's own reading of the stream, from its start to its end; a stream the engine seeks in before
//! it has read it all is relayed and not kept.
//!
//! A stream that breaks off (a dropped connection, a reset server) is picked up again where it stopped, so the song
//! does not end early; one whose length is known goes to the engine with that length, so an end that comes too soon
//! is never taken for the song's end.
//!
//! Also the routes for the copies: `GET /api/played` (how many, how much room), `POST /api/played` (`cap_mb`: how
//! much room they may take, 0 for none) and `POST /api/played/clear`.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::path::PathBuf;
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
/// How many times a stream that broke off is picked up again.
const PICK_UPS: u32 = 3;
/// What a playlist of pieces (HLS, DASH) is called: the engine is sent to it, as its pieces are named from where it is.
const PLAYLISTS: [&str; 5] = [
    "application/vnd.apple.mpegurl",
    "application/x-mpegurl",
    "audio/mpegurl",
    "audio/x-mpegurl",
    "application/dash+xml",
];

/// The streams being relayed, and the folder the copies go to.
pub(crate) struct Relays {
    links: Mutex<HashMap<String, Arc<Link>>>,
    folder: PathBuf,
    keys: RandomState,
    made: AtomicU64,
    /// Counts the times every copy was cleared: a copy that was coming in then is not kept.
    cleared: Arc<AtomicU64>,
    /// Copies' files that could not be deleted (the engine had one open): deleted later.
    undeleted: Arc<Mutex<Vec<String>>>,
    /// The songs whose copy was the last thing handed to the engine for them: one the engine fails on is let go.
    handed: Mutex<HashMap<i64, Instant>>,
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
    /// The relays, keeping copies in `folder`. What a run before left half written there is deleted (unless the
    /// library cannot say which copies it keeps: then nothing is).
    pub(crate) fn new(folder: PathBuf, library: &Library) -> Self {
        if let Ok(kept) = library.played_files() {
            played::tidy(&folder, &kept);
        }
        Self {
            links: Mutex::new(HashMap::new()),
            folder,
            keys: RandomState::new(),
            made: AtomicU64::new(0),
            cleared: Arc::new(AtomicU64::new(0)),
            undeleted: Arc::new(Mutex::new(Vec::new())),
            handed: Mutex::new(HashMap::new()),
        }
    }

    /// The song `id`'s copy and its file: `None` when none is kept. One whose file is gone, or is not the size it
    /// was kept at, is let go.
    pub(crate) fn copy(&self, library: &Mutex<Library>, id: i64) -> Option<(PathBuf, PlayedCopy)> {
        let mut library = library.lock().unwrap();
        let copy = library.played_copy(id, now_ms()).ok().flatten()?;
        let path = self.folder.join(&copy.file);
        if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == copy.bytes) {
            return Some((path, copy));
        }
        if let Ok(Some(file)) = library.drop_played_copy(id) {
            self.delete(vec![file]);
        }
        None
    }

    /// The song `id`'s copy, to be played.
    pub(crate) fn copy_of(&self, library: &Mutex<Library>, id: i64) -> Option<Located> {
        let (path, copy) = self.copy(library, id)?;
        self.handed.lock().unwrap().insert(id, Instant::now());
        Some(Located {
            locator: path.to_string_lossy().into_owned(),
            tier: copy.tier,
            addon: None,
        })
    }

    /// The song `id` was given to the engine from somewhere other than its copy.
    pub(crate) fn not_handed(&self, id: i64) {
        self.handed.lock().unwrap().remove(&id);
    }

    /// The engine could not play the song `id`: when what it was given was the song's copy, the copy is let go (the
    /// player asks for the song once more, and it comes from an add-on).
    pub(crate) fn failed(&self, library: &Mutex<Library>, id: i64) {
        if self.handed.lock().unwrap().remove(&id).is_none() {
            return;
        }
        let dropped = library.lock().unwrap().drop_played_copy(id);
        if let Ok(Some(file)) = dropped {
            self.delete(vec![file]);
        }
    }

    /// Deletes copies' files; one that cannot be deleted now (the engine has it open) is tried again later.
    pub(crate) fn delete(&self, files: Vec<String>) {
        let left = played::delete_files(&self.folder, &files);
        self.undeleted.lock().unwrap().extend(left);
    }

    /// Tries again to delete the files that could not be deleted, unless one is a kept copy again.
    fn delete_left(&self, library: &Mutex<Library>) {
        let left = std::mem::take(&mut *self.undeleted.lock().unwrap());
        if left.is_empty() {
            return;
        }
        let Ok(kept) = library.lock().unwrap().played_files() else {
            self.undeleted.lock().unwrap().extend(left);
            return;
        };
        let gone: Vec<String> = left.into_iter().filter(|f| !kept.contains(f)).collect();
        self.delete(gone);
    }

    /// Every copy is let go.
    pub(crate) fn clear(&self, library: &Mutex<Library>) -> Result<()> {
        // Counted first: a copy that comes in from now on sees it, and is not kept.
        self.cleared.fetch_add(1, Ordering::AcqRel);
        let gone = library.lock().unwrap().clear_played_copies()?;
        self.handed.lock().unwrap().clear();
        self.delete(gone);
        Ok(())
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
        self.delete_left(library);
        if *request.method() != Method::Get {
            let _ = request.respond(plain(405, "only GET"));
            return;
        }
        // The engine sends none of these; a web page does.
        let from_a_page = request.headers().iter().any(|h| {
            let name = h.field.as_str().as_str().to_ascii_lowercase();
            name == "origin" || name == "referer" || name.starts_with("sec-fetch-")
        });
        if from_a_page {
            let _ = request.respond(plain(403, "for the player only"));
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
                    downloads::Trouble::Failed(why)
                    | downloads::Trouble::Held(why, _)
                    | downloads::Trouble::Away(why)
                    | downloads::Trouble::Busy(why, _)
                    | downloads::Trouble::Later(why, _)
                    | downloads::Trouble::Refused(why, _) => why,
                    downloads::Trouble::Cancelled => "cancelled".into(),
                };
                let _ = request.respond(plain(502, &why));
                return;
            }
        };
        // A playlist of pieces names them from where it is: the engine is sent there.
        let kind = upstream.content_type.as_deref().unwrap_or_default();
        let kind = kind
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if PLAYLISTS.contains(&kind.as_str()) {
            if let Ok(to) = Header::from_bytes(&b"Location"[..], link.url.as_bytes()) {
                let _ = request.respond(Response::empty(StatusCode(302)).with_header(to));
            } else {
                let _ = request.respond(plain(502, "the stream's address cannot be given"));
            }
            return;
        }
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
        // Where this answer is in the whole stream: a part ("bytes 100-999/1000"), or all of it.
        let span = upstream.range.as_deref().and_then(span);
        let (at, end) = match (upstream.status, span) {
            (206, Some((start, end, _))) => (start, Some(end)),
            (206, None) => (0, None),
            _ => (0, upstream.length),
        };
        // Its length: as told, or, for a part told without one, from where the part is.
        let told = upstream.length.or(match (upstream.status, span) {
            (206, Some((start, end, _))) => Some(end - start),
            _ => None,
        });
        // A copy is written from an answer that holds the whole stream from its start, when there is room for it. A
        // part whose length is told otherwise than its range is not trusted to be whole.
        let from_start = range.as_deref().is_none_or(|r| r.trim() == "bytes=0-");
        let whole = upstream.status == 200
            || (upstream.status == 206
                && span.is_some_and(|(start, end, total)| {
                    start == 0 && total == Some(end) && upstream.length.is_none_or(|n| n == end)
                }));
        let cap = library.lock().unwrap().played_cap_bytes().unwrap_or(0);
        let copying = from_start
            && whole
            && !upstream.encoded
            && cap > 0
            && told.is_none_or(|n| n <= cap)
            && !link.copying.swap(true, Ordering::AcqRel);
        let length = told.and_then(|n| usize::try_from(n).ok());
        let status = StatusCode(upstream.status);
        let body = Resume {
            url: link.url.clone(),
            body: upstream.body,
            at,
            end,
            tries: 0,
        };
        let respond = |body: Box<dyn Read + Send>| {
            let response = Response::new(status, headers, body, length, None);
            // A known length is sent as it is, so an end that comes too soon is not taken for the end.
            let response = match length {
                Some(_) => response.with_chunked_threshold(usize::MAX),
                None => response,
            };
            let _ = request.respond(response);
        };
        if copying {
            let part = self.folder.join(format!(
                "{}-{:x}.part",
                link.id,
                self.made.fetch_add(1, Ordering::Relaxed)
            ));
            respond(Box::new(Tee::new(
                Box::new(body),
                part,
                told,
                Keep {
                    library: Arc::clone(library),
                    folder: self.folder.clone(),
                    link: Arc::clone(&link),
                    cleared: Arc::clone(&self.cleared),
                    generation: self.cleared.load(Ordering::Acquire),
                    undeleted: Arc::clone(&self.undeleted),
                },
            )));
        } else {
            respond(Box::new(body));
        }
    }
}

/// Where a `Content-Range` answer ("bytes 100-999/1000") is in the whole stream: its start, its end (after its last
/// byte), and the whole length when it is told.
fn span(range: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = range.trim().strip_prefix("bytes ")?;
    let (part, total) = rest.split_once('/')?;
    let (start, last) = part.split_once('-')?;
    let (start, last) = (
        start.trim().parse::<u64>().ok()?,
        last.trim().parse::<u64>().ok()?,
    );
    let end = last.checked_add(1).filter(|end| *end > start)?;
    let total = match total.trim() {
        "*" => None,
        n => Some(n.parse::<u64>().ok()?),
    };
    Some((start, end, total))
}

/// The stream from the add-on's server, picked up again where it broke off (a dropped connection, a reset) a few
/// times, so the engine reading through the relay is not told of it. Only a stream whose end is known can be.
struct Resume {
    url: String,
    body: Box<dyn Read + Send>,
    /// Where the stream is in the whole of it.
    at: u64,
    /// Where it ends (after its last byte).
    end: Option<u64>,
    tries: u32,
}

impl Resume {
    fn pick_up(&mut self) -> bool {
        let Some(end) = self.end else {
            return false;
        };
        if self.tries >= PICK_UPS || self.at >= end {
            return false;
        }
        self.tries += 1;
        std::thread::sleep(Duration::from_millis(400 * u64::from(self.tries)));
        let asked = format!("bytes={}-{}", self.at, end - 1);
        match downloads::open_stream(&self.url, Some(&asked)) {
            Ok(upstream)
                if upstream.status == 206
                    && !upstream.encoded
                    && upstream
                        .range
                        .as_deref()
                        .and_then(span)
                        .map(|(start, _, _)| start)
                        == Some(self.at) =>
            {
                self.body = upstream.body;
                true
            }
            _ => false,
        }
    }
}

impl Read for Resume {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let broke = match self.body.read(buf) {
                Ok(0) if self.end.is_some_and(|end| self.at < end) => {
                    std::io::Error::new(ErrorKind::UnexpectedEof, "the stream ended too soon")
                }
                Ok(n) => {
                    self.at += n as u64;
                    return Ok(n);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => e,
            };
            if !self.pick_up() {
                return Err(broke);
            }
        }
    }
}

/// What keeping a finished copy needs.
struct Keep {
    library: Arc<Mutex<Library>>,
    folder: PathBuf,
    link: Arc<Link>,
    /// The relays' count of clears, and what it was when the copy began: cleared since, it is not kept.
    cleared: Arc<AtomicU64>,
    generation: u64,
    undeleted: Arc<Mutex<Vec<String>>>,
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
        let Some(out) = self.out.take() else {
            return;
        };
        self.keep.link.copying.store(false, Ordering::Release);
        // On the disk before it is named: a power cut never leaves a copy kept that is not all there.
        let written = out.into_inner().is_ok_and(|file| file.sync_all().is_ok());
        let kind = downloads::kind_of(&self.head);
        let whole = self.expected.is_none_or(|n| n == self.got) && self.got > 0;
        let (Some(kind), true, true) = (kind, whole, written) else {
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
        let kept = {
            let mut library = self.keep.library.lock().unwrap();
            // Every copy was cleared while this one came in: it goes too.
            if self.keep.cleared.load(Ordering::Acquire) != self.keep.generation {
                drop(library);
                let _ = std::fs::remove_file(&file);
                return;
            }
            library.keep_played_copy(
                link.id,
                &name,
                self.got,
                link.tier.as_deref(),
                link.addon.as_deref(),
                now_ms(),
            )
        };
        match kept {
            Ok((_, gone)) => {
                let left = played::delete_files(&self.keep.folder, &gone);
                self.keep.undeleted.lock().unwrap().extend(left);
            }
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
            state.relays.delete(gone);
            Ok(Some(reply_json(200, room(state)?)))
        }
        (Method::Post, "/api/played/clear") => {
            state.relays.clear(&state.library)?;
            Ok(Some(reply_json(200, room(state)?)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_read_whole_or_not_at_all() {
        assert_eq!(span("bytes 0-999/1000"), Some((0, 1000, Some(1000))));
        assert_eq!(span("bytes 100-199/*"), Some((100, 200, None)));
        assert_eq!(span(" bytes 5-5/6 "), Some((5, 6, Some(6))));
        // An end past what a number holds, an end before the start, and words.
        assert_eq!(span(&format!("bytes 0-{}/1", u64::MAX)), None);
        assert_eq!(span("bytes 9-3/10"), None);
        assert_eq!(span("bytes */1000"), None);
        assert_eq!(span("items 0-9/10"), None);
    }

    /// A body that gives `first` and then breaks off.
    struct Breaks(Vec<u8>);

    impl Read for Breaks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Err(std::io::Error::new(ErrorKind::ConnectionReset, "reset"));
            }
            let n = buf.len().min(self.0.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn a_stream_that_breaks_off_is_picked_up_where_it_stopped() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let served = data.clone();
        // A server that answers ranges ("bytes=S-E") with that part of the data.
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let asked = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Range"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let (start, last) = asked
                    .trim_start_matches("bytes=")
                    .split_once('-')
                    .map(|(s, e)| (s.parse::<usize>().unwrap(), e.parse::<usize>().unwrap()))
                    .unwrap();
                let range = format!("bytes {start}-{last}/{}", served.len());
                let response = Response::from_data(served[start..=last].to_vec())
                    .with_status_code(StatusCode(206))
                    .with_header(
                        Header::from_bytes(&b"Content-Range"[..], range.as_bytes()).unwrap(),
                    );
                let _ = request.respond(response);
            }
        });
        let mut stream = Resume {
            url: format!("http://127.0.0.1:{port}/song"),
            body: Box::new(Breaks(data[..40_000].to_vec())),
            at: 0,
            end: Some(data.len() as u64),
            tries: 0,
        };
        let mut got = Vec::new();
        stream.read_to_end(&mut got).unwrap();
        assert!(
            got == data,
            "{} bytes, not the {} sent",
            got.len(),
            data.len()
        );
        assert_eq!(stream.tries, 1);
    }

    #[test]
    fn a_stream_that_cannot_be_picked_up_says_so_and_does_not_end_quietly() {
        // Its end is known and it stops short: an error, never an end. (The address cannot be reached, so it is not
        // picked up.)
        let mut stream = Resume {
            url: "https://127.0.0.1:9/never".into(),
            body: Box::new(Breaks(vec![1, 2, 3])),
            at: 0,
            end: Some(10),
            tries: PICK_UPS,
        };
        let mut got = Vec::new();
        let said = stream.read_to_end(&mut got);
        assert_eq!(got, [1, 2, 3]);
        assert_eq!(said.unwrap_err().kind(), ErrorKind::ConnectionReset);

        // A body that ends quietly before its known end is an error too.
        let mut short = Resume {
            url: "https://127.0.0.1:9/never".into(),
            body: Box::new(&[1u8, 2][..]),
            at: 0,
            end: Some(4),
            tries: PICK_UPS,
        };
        let mut got = Vec::new();
        assert_eq!(
            short.read_to_end(&mut got).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
        assert_eq!(got, [1, 2]);
    }
}
