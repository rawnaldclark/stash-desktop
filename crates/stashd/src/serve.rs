//! The core's local API: what every shell (the desktop window, later a phone remote) talks to.
//!
//! It listens on this computer only. A request must name this server as its Host (so a web page cannot reach it by
//! pointing a domain name at 127.0.0.1) and, when it comes from a page, that page must be the UI this server serves
//! or the desktop window. For now the API reads the library: counts, a page of tracks or albums at any position, and
//! search.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::json;
use stash_core::Library;
use stash_core::audio::{Output, find_library};
use stash_core::player::Player;
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

/// The largest page a list may ask for.
const PAGE_CAP: usize = 500;
/// The largest body the API reads.
const BODY_CAP: u64 = 256 * 1024;

pub struct Options {
    pub db: PathBuf,
    /// A folder holding the built UI (`index.html` and its files), served at `/`.
    pub ui: Option<PathBuf>,
    pub port: u16,
    /// Where a measurement the UI posts to `/api/bench-result` is written; the server then stops.
    pub bench_out: Option<PathBuf>,
    /// Where the player sends its sound; `None` runs the core without a player.
    pub audio: Option<Output>,
    /// The libmpv file for the player; without it, the one `STASH_MPV_LIBRARY` names or one beside the program.
    pub mpv: Option<PathBuf>,
}

type Reply = Response<std::io::Cursor<Vec<u8>>>;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
    Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
}

fn reply_json(status: u16, value: serde_json::Value) -> Reply {
    reply(status, "application/json", value.to_string().into_bytes())
}

fn refuse(status: u16, why: &str) -> Reply {
    reply_json(status, json!({ "error": { "message": why } }))
}

fn find<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// The origins a page may call the API from: this server's own, and the desktop window's.
fn allowed_origin(origin: &str, port: u16) -> bool {
    origin == format!("http://127.0.0.1:{port}")
        || origin == format!("http://localhost:{port}")
        || origin == "http://tauri.localhost"
        || origin == "https://tauri.localhost"
        || origin == "tauri://localhost"
}

/// The core, listening but not yet answering: the port is known, so a window can be pointed at it.
pub struct Core {
    library: Library,
    server: Server,
    options: Options,
    port: u16,
    /// The player, or the reason there is none (no audio engine found, or it would not start).
    player: std::result::Result<Player, String>,
}

impl Core {
    /// Opens the library and takes the port (`0` asks the system for a free one).
    pub fn bind(options: Options) -> Result<Self> {
        let library = Library::open(&options.db)
            .with_context(|| format!("opening {}", options.db.display()))?;
        let server = Server::http(("127.0.0.1", options.port))
            .map_err(|e| anyhow!("cannot listen on 127.0.0.1:{}: {e}", options.port))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .unwrap_or(options.port);
        let player = match &options.audio {
            None => Err("this core runs without a player".to_string()),
            Some(output) => match options.mpv.clone().or_else(find_library) {
                None => Err("the audio engine (libmpv) was not found".to_string()),
                Some(engine) => Player::start(&engine, output).map_err(|e| e.to_string()),
            },
        };
        Ok(Self {
            library,
            server,
            options,
            port,
            player,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Answers requests until a measurement arrives (when one is being collected), else for good.
    pub fn run(self) -> Result<()> {
        let Core {
            library,
            server,
            options,
            port,
            player,
        } = self;
        for mut request in server.incoming_requests() {
            let host_ok = find(&request, "Host").is_some_and(|h| {
                h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}")
            });
            let origin = find(&request, "Origin").map(str::to_string);
            let origin_ok = origin.as_deref().is_none_or(|o| allowed_origin(o, port));

            let mut stop = false;
            let mut response = if !host_ok {
                refuse(403, "this server answers only as 127.0.0.1 or localhost")
            } else if !origin_ok {
                refuse(403, "this page may not use the Stash core")
            } else if *request.method() == Method::Options {
                reply(204, "text/plain", Vec::new())
            } else {
                match route(&library, &options, &player, &mut request) {
                    Ok((response, finished)) => {
                        stop = finished;
                        response
                    }
                    Err(e) => refuse(500, &format!("{e:#}")),
                }
            };
            // The desktop window is another origin than this server: it is told it may read the answer.
            if let Some(origin) = origin.filter(|o| allowed_origin(o, port)) {
                response = response
                    .with_header(header("Access-Control-Allow-Origin", &origin))
                    .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
                    .with_header(header("Vary", "Origin"));
            }
            let _ = request.respond(response);
            if stop {
                break;
            }
        }
        Ok(())
    }
}

/// Binds and runs in one go, saying where it listens.
pub fn serve(options: Options) -> Result<()> {
    let core = Core::bind(options)?;
    println!("listening on http://127.0.0.1:{}", core.port());
    core.run()
}

/// The answer to one request, and whether the server is done (a measurement arrived).
/// The body of a request as JSON (an empty body is `{}`).
fn body_json(request: &mut Request) -> Result<serde_json::Value> {
    let mut body = String::new();
    request
        .as_reader()
        .take(BODY_CAP)
        .read_to_string(&mut body)?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).context("the request's body is not JSON")
}

/// The player's state as the window reads it. Without a player: `available: false` and the reason.
fn player_reply(player: &std::result::Result<Player, String>) -> Reply {
    match player {
        Ok(player) => {
            let mut state = json!(player.state());
            state["available"] = json!(true);
            reply_json(200, state)
        }
        Err(why) => reply_json(
            200,
            json!({ "available": false, "reason": why, "status": "idle" }),
        ),
    }
}

/// One of the player's routes. `None`: the path is not one of them.
fn player_route(
    library: &Library,
    player: &std::result::Result<Player, String>,
    request: &mut Request,
    path: &str,
) -> Result<Option<Reply>> {
    if *request.method() == Method::Get && path == "/api/player" {
        return Ok(Some(player_reply(player)));
    }
    if *request.method() != Method::Post || !path.starts_with("/api/player/") {
        return Ok(None);
    }
    let Ok(playing) = player else {
        return Ok(Some(player_reply(player)));
    };
    let body = body_json(request)?;
    match path {
        // What to play: `{"ids": [...]}` in that order, or `{"offset": n}` for the library's rows from there on.
        "/api/player/play" => {
            let ids: Vec<i64> = match body.get("ids").and_then(|v| v.as_array()) {
                Some(ids) => ids
                    .iter()
                    .filter_map(|v| v.as_i64())
                    .take(PAGE_CAP)
                    .collect(),
                None => {
                    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    let count = body.get("count").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
                    library.track_ids_at(offset, count.clamp(1, PAGE_CAP))?
                }
            };
            playing.play(library.queue_items(&ids)?)?;
        }
        "/api/player/pause" => {
            playing.set_paused(body.get("paused").and_then(|v| v.as_bool()).unwrap_or(true))?
        }
        "/api/player/next" => playing.next()?,
        "/api/player/previous" => playing.previous()?,
        "/api/player/seek" => {
            playing.seek_to(body.get("seconds").and_then(|v| v.as_f64()).unwrap_or(0.0))?
        }
        "/api/player/stop" => playing.stop()?,
        _ => return Ok(None),
    }
    Ok(Some(player_reply(player)))
}

fn route(
    library: &Library,
    options: &Options,
    player: &std::result::Result<Player, String>,
    request: &mut Request,
) -> Result<(Reply, bool)> {
    let url =
        Url::parse(&format!("http://local{}", request.url())).context("the request's path")?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let number = |name: &str, default: usize| {
        param(name)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    };
    let limit = |default: usize| number("limit", default).clamp(1, PAGE_CAP);

    let path = url.path().to_string();
    if let Some(response) = player_route(library, player, request, &path)? {
        return Ok((response, false));
    }
    let response = match (request.method(), url.path()) {
        (Method::Get, "/api/counts") => reply_json(200, json!(library.counts()?)),
        (Method::Get, "/api/tracks") => reply_json(
            200,
            json!({ "rows": library.tracks_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/albums") => reply_json(
            200,
            json!({ "rows": library.albums_at(number("offset", 0), limit(120))? }),
        ),
        (Method::Get, "/api/search") => reply_json(
            200,
            json!({ "rows": library.search(&param("q").unwrap_or_default(), limit(50))? }),
        ),
        (Method::Post, "/api/bench-result") => {
            let mut body = String::new();
            request
                .as_reader()
                .take(BODY_CAP)
                .read_to_string(&mut body)?;
            let value: serde_json::Value =
                serde_json::from_str(&body).context("the measurement is not JSON")?;
            let Some(out) = &options.bench_out else {
                return Ok((
                    refuse(404, "this server is not collecting a measurement"),
                    false,
                ));
            };
            fs::write(out, serde_json::to_string_pretty(&value)?)
                .with_context(|| format!("writing {}", out.display()))?;
            println!("measurement written to {}", out.display());
            return Ok((reply_json(200, json!({ "ok": true })), true));
        }
        (Method::Get, path) if !path.starts_with("/api/") => match &options.ui {
            Some(root) => ui_file(root, path),
            None => refuse(404, "no UI is served here"),
        },
        _ => refuse(404, "no such route"),
    };
    Ok((response, false))
}

/// A file of the built UI. Only plain names below the UI folder: no `..`, no drive letters, nothing absolute.
fn ui_file(root: &Path, path: &str) -> Reply {
    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let plain = relative
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains([':', '\\']));
    if !plain {
        return refuse(400, "not a file of the UI");
    }
    let file = root.join(relative);
    let content_type = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    match fs::read(&file) {
        Ok(bytes) => reply(200, content_type, bytes),
        Err(_) => refuse(404, "no such file"),
    }
}
