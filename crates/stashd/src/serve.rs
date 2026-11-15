//! The core's local API: what every shell (the desktop window, later a phone remote) talks to.
//!
//! It listens on this computer only. A request must name this server as its Host (so a web page cannot reach it by
//! pointing a domain name at 127.0.0.1) and, when it comes from a page, that page must be the UI this server serves
//! or the desktop window.
//!
//! The API reads the library (counts, a page of tracks or albums at any position, search), drives the player, and
//! keeps the listener's add-ons: adding one from a link, their order, searching them, and playing what they find.
//! Several requests are answered at once, so an add-on that is slow to answer holds up its own answer and nothing
//! else.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use stash_core::addons::{Addon, AddonError, Code, StreamRequest, Track};
use stash_core::audio::{Output, find_library};
use stash_core::player::{AddonTrack, Failure, Located, Player, QueueItem, Resolver, Settings};
use stash_core::{Library, StoredAddon};
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

/// The largest page a list may ask for.
const PAGE_CAP: usize = 500;
/// The largest body the API reads.
const BODY_CAP: u64 = 256 * 1024;
/// How many requests are answered at once.
const WORKERS: usize = 8;
/// The longest text taken from an add-on's answer, or from the window about an add-on's track.
const TEXT_CAP: usize = 300;
/// The most tracks of an add-on one search shows, and one play request queues.
const ADDON_TRACKS_CAP: usize = 200;

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

fn reply_json(status: u16, value: Value) -> Reply {
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

fn capped(text: &str) -> String {
    text.chars().take(TEXT_CAP).collect()
}

/// An add-on the listener added, as the core holds it while it runs.
struct Loaded {
    stored: StoredAddon,
    /// Ready to be asked, or why it cannot be (what was kept no longer passes the checks).
    addon: std::result::Result<Addon, String>,
}

impl Loaded {
    fn usable(&self) -> Option<&Addon> {
        self.addon.as_ref().ok().filter(|_| self.stored.enabled)
    }

    /// The add-on as the window lists it. Its link is not part of it: a link can carry the listener's own key.
    fn listed(&self) -> Value {
        let kept: Value = serde_json::from_str(&self.stored.manifest).unwrap_or(Value::Null);
        let text = |name: &str| kept.get(name).and_then(Value::as_str).map(capped);
        let host = Url::parse(&self.stored.link)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        json!({
            "id": self.stored.id,
            "name": text("name").unwrap_or_else(|| self.stored.id.clone()),
            "version": text("version"),
            "description": text("description"),
            "host": host,
            "resources": kept.get("resources").cloned().unwrap_or_else(|| json!([])),
            "enabled": self.stored.enabled,
            "problem": self.addon.as_ref().err(),
        })
    }
}

/// The listener's add-ons, in the listener's order.
type Registry = Arc<RwLock<Vec<Arc<Loaded>>>>;

fn load_addons(library: &Library) -> Result<Vec<Arc<Loaded>>> {
    Ok(library
        .addons()?
        .into_iter()
        .map(|stored| {
            let addon =
                Addon::from_stored(&stored.link, &stored.manifest).map_err(|e| e.in_words());
            Arc::new(Loaded { stored, addon })
        })
        .collect())
}

/// How the player asks an add-on where a track plays from.
fn resolver(registry: Registry) -> Resolver {
    Arc::new(move |wanted: &AddonTrack| {
        let gone = |why: &str| Failure {
            message: why.to_string(),
            miss: true,
        };
        let loaded = registry
            .read()
            .unwrap()
            .iter()
            .find(|a| a.stored.id == wanted.addon)
            .cloned()
            .ok_or_else(|| gone("its add-on was removed"))?;
        if !loaded.stored.enabled {
            return Err(gone("its add-on is switched off"));
        }
        let addon = loaded.addon.as_ref().map_err(|why| gone(why))?;
        match addon.stream(&wanted.track, &StreamRequest::default()) {
            Ok(info) => Ok(Located {
                locator: info.url,
                tier: Some(info.tier.as_str().to_string()),
            }),
            Err(e) => Err(Failure {
                message: e.in_words(),
                miss: e.is_miss(),
            }),
        }
    })
}

fn addon_error(e: &AddonError) -> Value {
    json!({ "code": e.code.as_str(), "message": e.in_words(), "retry_after": e.retry_after })
}

/// A track an add-on found, as the window shows it. What an add-on sends is cut to size; a track without a usable
/// id is left out.
fn found_track(track: &Track) -> Option<Value> {
    if track.id.is_empty() || track.id.len() > 512 {
        return None;
    }
    Some(json!({
        "id": track.id,
        "title": capped(&track.title),
        "artist": capped(&track.artist),
        "album": track.album.as_deref().map(capped),
        "duration_ms": track.duration_ms,
        "max_tier": track.max_tier.map(|t| t.as_str()),
        "explicit": track.explicit,
    }))
}

/// What everything that answers requests shares.
struct State {
    library: Mutex<Library>,
    options: Options,
    port: u16,
    /// The player, or the reason there is none (no audio engine found, or it would not start).
    player: std::result::Result<Player, String>,
    addons: Registry,
}

impl State {
    fn reload_addons(&self) -> Result<()> {
        let loaded = load_addons(&self.library.lock().unwrap())?;
        *self.addons.write().unwrap() = loaded;
        Ok(())
    }

    fn addons_reply(&self) -> Reply {
        let listed: Vec<Value> = self
            .addons
            .read()
            .unwrap()
            .iter()
            .map(|a| a.listed())
            .collect();
        reply_json(200, json!({ "addons": listed }))
    }
}

/// The core, listening but not yet answering: the port is known, so a window can be pointed at it.
pub struct Core {
    server: Server,
    state: State,
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
        let addons: Registry = Arc::new(RwLock::new(load_addons(&library)?));
        let player = match &options.audio {
            None => Err("this core runs without a player".to_string()),
            Some(output) => match options.mpv.clone().or_else(find_library) {
                None => Err("the audio engine (libmpv) was not found".to_string()),
                Some(engine) => {
                    let settings = Settings {
                        resolver: Some(resolver(Arc::clone(&addons))),
                        ..Settings::default()
                    };
                    Player::start_with(&engine, output, settings).map_err(|e| e.to_string())
                }
            },
        };
        Ok(Self {
            server,
            state: State {
                library: Mutex::new(library),
                options,
                port,
                player,
                addons,
            },
        })
    }

    pub fn port(&self) -> u16 {
        self.state.port
    }

    /// Answers requests until a measurement arrives (when one is being collected), else for good.
    pub fn run(self) -> Result<()> {
        let Core { server, state } = self;
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        match server.recv_timeout(Duration::from_millis(250)) {
                            Ok(Some(request)) => {
                                if answer(&state, request) {
                                    stop.store(true, Ordering::Relaxed);
                                }
                            }
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
        });
        Ok(())
    }
}

/// Binds and runs in one go, saying where it listens.
pub fn serve(options: Options) -> Result<()> {
    let core = Core::bind(options)?;
    println!("listening on http://127.0.0.1:{}", core.port());
    core.run()
}

/// Answers one request. True when the server is done (a measurement arrived).
fn answer(state: &State, mut request: Request) -> bool {
    let port = state.port;
    let host_ok = find(&request, "Host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    let origin = find(&request, "Origin").map(str::to_string);
    let origin_ok = origin.as_deref().is_none_or(|o| allowed_origin(o, port));

    let mut done = false;
    let mut response = if !host_ok {
        refuse(403, "this server answers only as 127.0.0.1 or localhost")
    } else if !origin_ok {
        refuse(403, "this page may not use the Stash core")
    } else if *request.method() == Method::Options {
        reply(204, "text/plain", Vec::new())
    } else {
        match route(state, &mut request) {
            Ok((response, finished)) => {
                done = finished;
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
    done
}

/// The body of a request as JSON (an empty body is `{}`).
fn body_json(request: &mut Request) -> Result<Value> {
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

/// The queue for tracks an add-on found: the window hands back what the search gave it.
fn addon_queue(addon: &str, tracks: Option<&Value>) -> Vec<QueueItem> {
    let text = |track: &Value, name: &str| track.get(name).and_then(Value::as_str).map(capped);
    tracks
        .and_then(Value::as_array)
        .map(|tracks| {
            tracks
                .iter()
                .take(ADDON_TRACKS_CAP)
                .filter_map(|track| {
                    let id = track.get("id").and_then(Value::as_str)?;
                    if id.is_empty() || id.len() > 512 {
                        return None;
                    }
                    Some(QueueItem {
                        id: None,
                        title: text(track, "title").unwrap_or_default(),
                        artist: text(track, "artist").unwrap_or_default(),
                        album: text(track, "album"),
                        addon: Some(AddonTrack {
                            addon: addon.to_string(),
                            track: id.to_string(),
                        }),
                        locator: String::new(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One of the player's routes. `None`: the path is not one of them.
fn player_route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let player = &state.player;
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
        // What to play: `{"ids": [...]}` (library tracks, in that order), `{"offset": n}` (the library's rows from
        // there on), or `{"addon": id, "tracks": [...]}` (tracks an add-on found, in that order).
        "/api/player/play" => {
            let items = if let Some(addon) = body.get("addon").and_then(Value::as_str) {
                addon_queue(addon, body.get("tracks"))
            } else {
                let library = state.library.lock().unwrap();
                let ids: Vec<i64> = match body.get("ids").and_then(Value::as_array) {
                    Some(ids) => ids
                        .iter()
                        .filter_map(Value::as_i64)
                        .take(PAGE_CAP)
                        .collect(),
                    None => {
                        let offset = body.get("offset").and_then(Value::as_u64).unwrap_or(0);
                        let count = body.get("count").and_then(Value::as_u64).unwrap_or(200);
                        library
                            .track_ids_at(offset as usize, (count as usize).clamp(1, PAGE_CAP))?
                    }
                };
                library.queue_items(&ids)?
            };
            playing.play(items)?;
        }
        "/api/player/pause" => {
            playing.set_paused(body.get("paused").and_then(Value::as_bool).unwrap_or(true))?
        }
        "/api/player/next" => playing.next()?,
        "/api/player/previous" => playing.previous()?,
        "/api/player/seek" => {
            playing.seek_to(body.get("seconds").and_then(Value::as_f64).unwrap_or(0.0))?
        }
        "/api/player/stop" => playing.stop()?,
        _ => return Ok(None),
    }
    Ok(Some(player_reply(player)))
}

/// Asks `asked` for tracks matching `query`, all at once; one group of results (or one reason) per add-on, in the
/// order given.
fn search_addons(asked: &[Arc<Loaded>], query: &str, limit: usize) -> Vec<Value> {
    let one = |loaded: &Loaded| {
        let who = json!({ "id": loaded.stored.id, "name": loaded.listed()["name"] });
        let Some(addon) = loaded.usable() else {
            return json!({ "addon": who, "tracks": [], "error": null });
        };
        match addon.search(query, limit as u32) {
            Ok(tracks) => {
                let tracks: Vec<Value> =
                    tracks.iter().filter_map(found_track).take(limit).collect();
                json!({ "addon": who, "tracks": tracks, "error": null })
            }
            Err(e) => json!({ "addon": who, "tracks": [], "error": addon_error(&e) }),
        }
    };
    std::thread::scope(|scope| {
        let asking: Vec<_> = asked
            .iter()
            .map(|loaded| scope.spawn(|| one(loaded)))
            .collect();
        asking
            .into_iter()
            .zip(asked)
            .map(|(asking, loaded)| {
                asking.join().unwrap_or_else(|_| {
                    json!({
                        "addon": { "id": loaded.stored.id, "name": loaded.stored.id },
                        "tracks": [],
                        "error": { "code": "unavailable", "message": "the search failed", "retry_after": null },
                    })
                })
            })
            .collect()
    })
}

/// One of the add-on routes. `None`: the path is not one of them.
fn addons_route(
    state: &State,
    request: &mut Request,
    path: &str,
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Reply>> {
    if !path.starts_with("/api/addons") {
        return Ok(None);
    }
    if *request.method() == Method::Get {
        return Ok(match path {
            "/api/addons" => Some(state.addons_reply()),
            // `q`, and optionally `addon` (ask only that one) and `limit`.
            "/api/addons/search" => {
                let query = param("q").unwrap_or_default();
                let only = param("addon");
                let limit = param("limit")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(25)
                    .clamp(1, ADDON_TRACKS_CAP);
                let asked: Vec<Arc<Loaded>> = state
                    .addons
                    .read()
                    .unwrap()
                    .iter()
                    .filter(|a| {
                        a.usable()
                            .is_some_and(|addon| addon.offers_resource("search"))
                    })
                    .filter(|a| only.as_deref().is_none_or(|id| id == a.stored.id))
                    .cloned()
                    .collect();
                let groups = if query.trim().is_empty() {
                    Vec::new()
                } else {
                    search_addons(&asked, query.trim(), limit)
                };
                Some(reply_json(200, json!({ "groups": groups })))
            }
            _ => None,
        });
    }
    if *request.method() != Method::Post {
        return Ok(None);
    }
    let body = body_json(request)?;
    let id = body.get("id").and_then(Value::as_str).unwrap_or_default();
    match path {
        // Adds the add-on at `link` (the address of its manifest); one that is there already is read again.
        "/api/addons" => {
            let link = body
                .get("link")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            // The add-on is asked for its manifest here, with nothing else waiting on it.
            let addon = match Addon::from_link(link) {
                Ok(addon) => addon,
                Err(e) => {
                    let status = match e.code {
                        Code::BadRequest | Code::InvalidResponse => 400,
                        _ => 502,
                    };
                    return Ok(Some(reply_json(
                        status,
                        json!({ "error": addon_error(&e) }),
                    )));
                }
            };
            state.library.lock().unwrap().save_addon(
                &addon.manifest.id,
                addon.link(),
                &addon.manifest_json(),
            )?;
        }
        "/api/addons/remove" => {
            state.library.lock().unwrap().remove_addon(id)?;
        }
        "/api/addons/enabled" => {
            let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            state
                .library
                .lock()
                .unwrap()
                .set_addon_enabled(id, enabled)?;
        }
        "/api/addons/order" => {
            let ids: Vec<String> = body
                .get("ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            state.library.lock().unwrap().order_addons(&ids)?;
        }
        _ => return Ok(None),
    }
    state.reload_addons()?;
    Ok(Some(state.addons_reply()))
}

/// The answer to one request, and whether the server is done (a measurement arrived).
fn route(state: &State, request: &mut Request) -> Result<(Reply, bool)> {
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
    if let Some(response) = player_route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = addons_route(state, request, &path, &param)? {
        return Ok((response, false));
    }
    let library = || state.library.lock().unwrap();
    let response = match (request.method(), url.path()) {
        (Method::Get, "/api/counts") => reply_json(200, json!(library().counts()?)),
        (Method::Get, "/api/tracks") => reply_json(
            200,
            json!({ "rows": library().tracks_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/albums") => reply_json(
            200,
            json!({ "rows": library().albums_at(number("offset", 0), limit(120))? }),
        ),
        (Method::Get, "/api/search") => reply_json(
            200,
            json!({ "rows": library().search(&param("q").unwrap_or_default(), limit(50))? }),
        ),
        (Method::Post, "/api/bench-result") => {
            let mut body = String::new();
            request
                .as_reader()
                .take(BODY_CAP)
                .read_to_string(&mut body)?;
            let value: Value =
                serde_json::from_str(&body).context("the measurement is not JSON")?;
            let Some(out) = &state.options.bench_out else {
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
        (Method::Get, path) if !path.starts_with("/api/") => match &state.options.ui {
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
