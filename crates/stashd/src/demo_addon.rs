//! A demo add-on: a folder of your own music files, served on this computer the way an add-on serves a catalogue.
//!
//! It is for trying the window and for the tests, and it is the smallest whole example of what Stash asks of an
//! add-on: a manifest, a search, an answer to "do you have this song", and for each track a place to play it from.
//! It answers on this computer only.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use stash_core::identity::words_key;
use stash_core::scan::scan_folder;
use stash_core::{Library, TrackRow};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use url::Url;

/// The most files the demo reads from its folder.
const TRACKS_CAP: usize = 5_000;
const TIERS: [&str; 4] = ["low", "high", "lossless", "hires"];

pub struct DemoAddon {
    /// The server, on a port of its own behind the add-on's (see [`door`]).
    server: Server,
    inside: SocketAddr,
    /// The add-on's own port, until it runs.
    listener: Option<TcpListener>,
    port: u16,
    library: Library,
    rows: HashMap<i64, TrackRow>,
    paths: HashMap<i64, String>,
    /// How long search and stream answers are held back, to see the window wait.
    delay: Duration,
    /// Answer in the looser shapes of an add-on made for Eclipse and its family, to try that kind with.
    eclipse: bool,
    /// Answer "do you have this song". Without it the add-on only searches and streams, as many do.
    resolves: bool,
    /// The listener's allowance is spent: every stream and download is refused as the contract says (to try what Stash
    /// does then). Shared, so it can be switched while the add-on runs.
    spent: Arc<AtomicBool>,
    /// Out of reach: nothing answers on its port at all (to try what Stash does with no connection, or the add-on's
    /// server down). Shared, like `spent`.
    reach: Reach,
    /// What it was asked, as `path` or `path?q=…` (a search's words), in the order asked. Shared, like `spent`.
    asked: Arc<Mutex<Vec<String>>>,
}

/// Switches a [`DemoAddon`] out of reach and back, also once it runs.
#[derive(Clone, Default)]
pub struct Reach {
    door: Arc<(Mutex<Door>, Condvar)>,
}

/// The door in front of the add-on's server (see [`door`]).
#[derive(Default)]
struct Door {
    /// Asked for: out of reach.
    gone: bool,
    /// As it is: the add-on's port is shut.
    shut: bool,
    /// The add-on's own port.
    port: u16,
    /// Why the port could not be listened on again, when it could not.
    trouble: Option<String>,
}

impl Reach {
    /// Out of reach (true) or back (false). Answers once it is so (the add-on runs), or why not after a few seconds.
    pub fn set(&self, gone: bool) -> Result<(), String> {
        let (door, bell) = &*self.door;
        let port = {
            let mut door = door.lock().unwrap();
            door.gone = gone;
            door.trouble = None;
            bell.notify_all();
            door.port
        };
        // The door waits for a connection: one wakes it, to shut.
        if gone && port != 0 {
            let _ = TcpStream::connect(("127.0.0.1", port));
        }
        let (door, waited) = bell
            .wait_timeout_while(door.lock().unwrap(), Duration::from_secs(5), |d| {
                d.shut != gone && d.trouble.is_none()
            })
            .unwrap();
        match &door.trouble {
            Some(why) => Err(why.clone()),
            None if waited.timed_out() => {
                Err("the add-on's port did not open or shut in time".into())
            }
            None => Ok(()),
        }
    }
}

/// The connections open through the door, to cut when it shuts.
type Open = Arc<Mutex<HashMap<u64, TcpStream>>>;

/// The add-on's own port, in front of its server: what comes in is passed through. Out of reach, the door is shut:
/// nothing listens on the port, so a connection is refused, and the connections open are cut, as when an add-on's
/// server goes down. (The server cannot do that itself: it keeps every connection it has answered on.)
fn door(mut listener: Option<TcpListener>, inside: SocketAddr, reach: Reach) {
    let open: Open = Default::default();
    let mut next = 0u64;
    let (state, bell) = &*reach.door;
    let port = state.lock().unwrap().port;
    let mut tries = 0u32;
    loop {
        {
            let mut door = state.lock().unwrap();
            if door.gone {
                if listener.take().is_some() {
                    for (_, line) in open.lock().unwrap().drain() {
                        let _ = line.shutdown(Shutdown::Both);
                    }
                }
                door.shut = true;
                bell.notify_all();
                // Shut until it is asked to open.
                drop(bell.wait_while(door, |d| d.gone).unwrap());
                continue;
            }
        }
        let listening = match listener.as_ref() {
            Some(listening) => listening,
            // Open again, on the same port.
            None => match TcpListener::bind(("127.0.0.1", port)) {
                Ok(again) => {
                    tries = 0;
                    listener.insert(again)
                }
                Err(e) => {
                    tries += 1;
                    if tries == 200 {
                        state.lock().unwrap().trouble = Some(format!(
                            "the add-on's port could not be listened on again: {e}"
                        ));
                        bell.notify_all();
                    }
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
            },
        };
        {
            let mut door = state.lock().unwrap();
            if door.shut {
                door.shut = false;
                bell.notify_all();
            }
        }
        let Ok((outside, _)) = listening.accept() else {
            continue;
        };
        // Woken to shut: the connection that woke it is let go.
        if state.lock().unwrap().gone {
            continue;
        }
        let Ok(within) = TcpStream::connect(inside) else {
            continue;
        };
        let (Ok(cut), Ok(from_outside), Ok(from_within)) =
            (outside.try_clone(), outside.try_clone(), within.try_clone())
        else {
            continue;
        };
        next += 1;
        open.lock().unwrap().insert(next, cut);
        pass(from_outside, within, None);
        pass(from_within, outside, Some((open.clone(), next)));
    }
}

/// Passes on what comes from `from` to `to` until either ends, then ends both (and forgets the connection).
fn pass(mut from: TcpStream, mut to: TcpStream, done: Option<(Open, u64)>) {
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from, &mut to);
        let _ = from.shutdown(Shutdown::Both);
        let _ = to.shutdown(Shutdown::Both);
        if let Some((open, id)) = done {
            open.lock().unwrap().remove(&id);
        }
    });
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn answer(status: u16, value: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(value.to_string().into_bytes())
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json"))
}

fn refusal(status: u16, code: &str, message: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    answer(
        status,
        json!({ "error": { "code": code, "message": message } }),
    )
}

/// A track as a search or resolve answer gives it: the contract's shape, or Eclipse's (a number for an id, the length
/// in seconds).
fn row_json(row: &TrackRow, eclipse: bool) -> Value {
    if eclipse {
        json!({ "id": row.id, "title": row.title, "artist": row.artist, "album": row.album, "duration": row.duration_ms as f64 / 1000.0 })
    } else {
        json!({
            "id": row.id.to_string(),
            "title": row.title,
            "artist": row.artist,
            "album": row.album,
            "durationMs": row.duration_ms,
            "maxTier": quality(row).0,
        })
    }
}

/// The tier a file is, and its codec in the contract's words (left out where the contract has no word for it).
fn quality(row: &TrackRow) -> (&'static str, Option<&'static str>) {
    let codec = row.codec.as_deref().unwrap_or("");
    let lossless = matches!(codec, "flac" | "alac" | "pcm" | "ape" | "wavpack");
    if !lossless {
        let named = match codec {
            "mp3" => Some("mp3"),
            "aac" => Some("aac"),
            "opus" => Some("opus"),
            "vorbis" => Some("vorbis"),
            _ => None,
        };
        return ("high", named);
    }
    let above_cd =
        row.bit_depth.is_some_and(|b| b > 16) || row.sample_rate.is_some_and(|r| r > 48_000);
    let named = match codec {
        "flac" => Some("flac"),
        "alac" => Some("alac"),
        "pcm" => Some("pcm"),
        _ => None,
    };
    (if above_cd { "hires" } else { "lossless" }, named)
}

impl DemoAddon {
    /// Reads the music under `folder` and takes `port` on this computer (`0` asks the system for a free one).
    pub fn bind(folder: &Path, port: u16, delay: Duration) -> Result<Self> {
        Self::bind_as(folder, port, delay, false)
    }

    /// As [`Self::bind`]; with `eclipse`, the add-on answers as one made for Eclipse does: no `stash` in its manifest,
    /// ids as numbers, lengths in seconds, a quality word instead of a tier.
    pub fn bind_as(folder: &Path, port: u16, delay: Duration, eclipse: bool) -> Result<Self> {
        Self::bind_with(folder, port, delay, eclipse, true)
    }

    /// As [`Self::bind_as`]; without `resolves`, the add-on offers search and streams only.
    pub fn bind_with(
        folder: &Path,
        port: u16,
        delay: Duration,
        eclipse: bool,
        resolves: bool,
    ) -> Result<Self> {
        let mut library = Library::open_in_memory()?;
        scan_folder(&mut library, folder, |_| {})
            .with_context(|| format!("reading {}", folder.display()))?;
        let rows = library.tracks_at(0, TRACKS_CAP)?;
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let paths = library
            .queue_items(&ids)?
            .into_iter()
            .filter_map(|item| Some((item.id?, item.locator)))
            .collect();
        let server = Server::http(("127.0.0.1", 0))
            .map_err(|e| anyhow!("cannot listen on this computer: {e}"))?;
        let inside = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| anyhow!("the server has no port"))?;
        let listener = TcpListener::bind(("127.0.0.1", port))
            .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            server,
            inside,
            listener: Some(listener),
            port,
            library,
            rows: rows.into_iter().map(|r| (r.id, r)).collect(),
            paths,
            delay,
            eclipse,
            resolves,
            spent: Default::default(),
            reach: {
                let reach = Reach::default();
                reach.door.0.lock().unwrap().port = port;
                reach
            },
            asked: Default::default(),
        })
    }

    /// What the add-on was asked, also once it runs: each request's path, a search's with its words (`/search?q=…`).
    pub fn asked(&self) -> Arc<Mutex<Vec<String>>> {
        self.asked.clone()
    }

    /// Switches the allowance spent (true) or back (false), also once the add-on runs.
    pub fn allowance_spent(&self) -> Arc<AtomicBool> {
        self.spent.clone()
    }

    /// Switches the add-on out of reach and back, also once it runs.
    pub fn out_of_reach(&self) -> Reach {
        self.reach.clone()
    }

    pub fn tracks(&self) -> usize {
        self.rows.len()
    }

    /// The link to add this add-on by.
    pub fn link(&self) -> String {
        format!("http://127.0.0.1:{}/manifest.json", self.port)
    }

    /// Answers until the program ends.
    pub fn run(mut self) {
        let (listener, inside, reach) = (self.listener.take(), self.inside, self.reach.clone());
        std::thread::spawn(move || door(listener, inside, reach));
        for request in self.server.incoming_requests() {
            self.handle(request);
        }
    }

    fn handle(&self, request: Request) {
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str().to_string());
        let here = [
            format!("127.0.0.1:{}", self.port),
            format!("localhost:{}", self.port),
        ];
        if !host.is_some_and(|h| here.contains(&h)) || *request.method() != Method::Get {
            let _ = request.respond(refusal(400, "bad_request", "not a request for this add-on"));
            return;
        }
        let Ok(url) = Url::parse(&format!("http://local{}", request.url())) else {
            let _ = request.respond(refusal(400, "bad_request", "not a path"));
            return;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let path = url.path();
        self.asked.lock().unwrap().push(match (path, param("q")) {
            ("/search", Some(q)) => format!("/search?q={q}"),
            _ => path.to_string(),
        });

        if path == "/manifest.json" {
            if self.eclipse {
                let _ = request.respond(answer(
                    200,
                    json!({
                        "id": "Demo.Folder.Eclipse",
                        "name": "Demo folder (Eclipse)",
                        "version": env!("CARGO_PKG_VERSION"),
                        "description": "A folder of your own files, served the way an Eclipse add-on serves them.",
                        "resources": if self.resolves { json!(["search", "stream", "resolve"]) } else { json!(["search", "stream"]) },
                        "types": ["track"],
                    }),
                ));
                return;
            }
            let _ = request.respond(answer(
                200,
                json!({
                    "stash": 1,
                    "id": "demo.folder",
                    "name": "Demo folder",
                    "version": env!("CARGO_PKG_VERSION"),
                    "kind": "server",
                    "description": "A folder of your own files, served as an add-on on this computer.",
                    "resources": ["search", "resolve", "stream", "download", "lyrics"],
                    "stream": { "cors": false, "manifests": ["none"], "tiers": ["high", "lossless", "hires"] },
                }),
            ));
        } else if path == "/search" {
            std::thread::sleep(self.delay);
            let limit = param("limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(25)
                .clamp(1, 200);
            let query = param("q").unwrap_or_default();
            let found = if query.trim().is_empty() {
                self.library.tracks_at(0, limit)
            } else {
                self.library.search(&query, limit)
            };
            let tracks: Vec<Value> = found
                .unwrap_or_default()
                .iter()
                .map(|row| row_json(row, self.eclipse))
                .collect();
            let _ = request.respond(answer(200, json!({ "tracks": tracks })));
        } else if path == "/resolve" && self.resolves {
            std::thread::sleep(self.delay);
            // The same song here is the same words, and a length within the contract's bounds when one is asked.
            let (Some(title), Some(artist)) = (param("title"), param("artist")) else {
                let _ =
                    request.respond(refusal(400, "bad_request", "title and artist are required"));
                return;
            };
            let words = words_key(&title, &artist);
            let asked = param("durationMs").and_then(|v| v.parse::<u64>().ok());
            let mut same: Vec<&TrackRow> = self
                .rows
                .values()
                .filter(|row| words_key(&row.title, &row.artist) == words)
                .filter(|row| {
                    asked.is_none_or(|ms| {
                        row.duration_ms == 0
                            || ms.abs_diff(row.duration_ms) <= 8_000.max(ms * 3 / 100)
                    })
                })
                .collect();
            same.sort_by_key(|row| row.id);
            let item = same.first().map(|row| row_json(row, self.eclipse));
            let _ = request.respond(answer(200, json!({ "item": item })));
        } else if let Some(id) = path
            .strip_prefix("/stream/")
            .or_else(|| path.strip_prefix("/download/").filter(|_| !self.eclipse))
        {
            std::thread::sleep(self.delay);
            if self.spent.load(Ordering::Relaxed) {
                let _ = request.respond(refusal(
                    429,
                    "quota_exceeded",
                    "your allowance for today is used up",
                ));
                return;
            }
            let Some(row) = id.parse::<i64>().ok().and_then(|id| self.rows.get(&id)) else {
                let _ = request.respond(refusal(404, "not_found", "no such track"));
                return;
            };
            if self.eclipse {
                // What an Eclipse add-on says of a stream: a codec word and a quality word, no tier.
                let (tier, codec) = quality(row);
                let _ = request.respond(answer(
                    200,
                    json!({
                        "url": format!("http://127.0.0.1:{}/media/{}", self.port, row.id),
                        "codec": if codec == Some("pcm") { Some("wav") } else { codec },
                        "quality": if tier == "high" { "HIGH" } else { "LOSSLESS" },
                        "bitDepth": row.bit_depth,
                        "sampleRate": row.sample_rate,
                    }),
                ));
                return;
            }
            let (tier, codec) = quality(row);
            let place = |name: &str| TIERS.iter().position(|t| *t == name);
            let wanted = param("tier").as_deref().and_then(place).unwrap_or(3);
            let least = param("minTier").as_deref().and_then(place).unwrap_or(0);
            let have = place(tier).unwrap_or(1);
            if have > wanted || have < least {
                let _ = request.respond(refusal(
                    404,
                    "quality_unavailable",
                    "this file is not in a quality that was asked for",
                ));
                return;
            }
            let _ = request.respond(answer(
                200,
                json!({
                    "url": format!("http://127.0.0.1:{}/media/{}", self.port, row.id),
                    "manifest": "none",
                    "tier": tier,
                    "codec": codec,
                    "bitDepth": row.bit_depth,
                    "sampleRate": row.sample_rate,
                    "durationMs": row.duration_ms,
                    "cors": false,
                }),
            ));
        } else if path == "/lyrics" && !self.eclipse {
            // A song's lyrics are the .lrc file beside it, as a player reads them (contract section 4.6): asked by
            // the add-on's own id, or by title and artist.
            let row = match param("id") {
                Some(id) => id.parse::<i64>().ok().and_then(|id| self.rows.get(&id)),
                None => {
                    let words = words_key(
                        &param("title").unwrap_or_default(),
                        &param("artist").unwrap_or_default(),
                    );
                    self.rows
                        .values()
                        .find(|row| words_key(&row.title, &row.artist) == words)
                }
            };
            let lyrics = row
                .and_then(|row| self.paths.get(&row.id))
                .and_then(|file| stash_core::lyrics::of_file(Path::new(file)))
                .filter(|(_, from)| *from == "the .lrc file")
                .map(|(lyrics, _)| lyrics);
            let _ = request.respond(match lyrics {
                Some(lyrics) => answer(200, json!(lyrics)),
                None => refusal(404, "not_found", "no lyrics for that song"),
            });
        } else if let Some(id) = path.strip_prefix("/media/") {
            let range = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Range"))
                .map(|h| h.value.as_str().to_string());
            match id.parse::<i64>().ok().and_then(|id| self.paths.get(&id)) {
                // The file goes out on a thread of its own: a player reads it as slowly as it plays.
                Some(file) => {
                    let file = file.clone();
                    std::thread::spawn(move || send_file(request, &file, range.as_deref()));
                }
                None => {
                    let _ = request.respond(refusal(404, "not_found", "no such file"));
                }
            }
        } else {
            let _ = request.respond(refusal(404, "not_found", "no such route"));
        }
    }
}

/// Sends a file, or the part of it a `Range: bytes=from-to` asks for (a player seeks that way).
fn send_file(request: Request, path: &str, range: Option<&str>) {
    let opened = File::open(path).and_then(|file| Ok((file.metadata()?.len(), file)));
    let Ok((size, mut file)) = opened else {
        let _ = request.respond(refusal(404, "not_found", "the file is gone"));
        return;
    };
    let content_type = match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "mp4" | "aac") => "audio/mp4",
        Some("ogg" | "opus") => "audio/ogg",
        _ => "application/octet-stream",
    };
    let asked = range
        .and_then(|r| r.trim().strip_prefix("bytes="))
        .and_then(|r| {
            let (from, to) = r.split_once('-')?;
            let from = from.trim().parse::<u64>().ok()?;
            let to = to
                .trim()
                .parse::<u64>()
                .ok()
                .unwrap_or(size.saturating_sub(1))
                .min(size.saturating_sub(1));
            (from <= to).then_some((from, to))
        });
    let (status, from, length) = match asked {
        Some((from, to)) => (206, from, to - from + 1),
        None => (200, 0, size),
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        let _ = request.respond(refusal(500, "unavailable", "the file could not be read"));
        return;
    }
    let mut headers = vec![
        header("Content-Type", content_type),
        header("Accept-Ranges", "bytes"),
    ];
    if status == 206 {
        headers.push(header(
            "Content-Range",
            &format!("bytes {from}-{}/{size}", from + length - 1),
        ));
    }
    let _ = request.respond(Response::new(
        StatusCode(status),
        headers,
        file.take(length),
        Some(length as usize),
        None,
    ));
}
