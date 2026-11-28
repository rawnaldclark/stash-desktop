//! Sessions (live listening: a host plays on a station, others tune in), for the desktop window. The window runs the web
//! player's Sessions code; these are the routes it calls on this computer in place of the web player's own Worker.
//!
//! - `GET /api/community` → `{ "key": "…" | null }`: this computer's Community key, which names its station on the share
//!   service (one station a key). It is this computer's own, apart from any phone's. `POST /api/community/join` makes it
//!   when there is none (it never replaces one) → `{ "key": "…" }`.
//! - `/api/sessions/*`: the share service's station and room routes, passed on server to server. The list and its rules
//!   are the web player's (`player-sessions/worker/sessions.ts`):
//!
//! ```text
//!   GET    /api/sessions/status                (answered here)                       { on, rooms, offline }
//!   GET    /api/sessions/live                  GET  /v1/community/live?limit=50
//!   GET    /api/sessions/upcoming              GET  /v1/community/upcoming?limit=20
//!   GET    /api/sessions/following             GET  /v1/community/stations/following key required
//!   GET    /api/sessions/stations/mine         GET  /v1/community/stations/mine      key required
//!   POST   /api/sessions/stations              POST /v1/community/stations           key required, the station
//!   GET    /api/sessions/stations/{id}         GET  /v1/community/stations/{id}
//!   PATCH  /api/sessions/stations/{id}         PATCH                                 key required, the changes
//!   DELETE /api/sessions/stations/{id}         DELETE                                key required
//!   PUT    /api/sessions/stations/{id}/heard   PUT  …/heard                          key required
//!   PUT    /api/sessions/stations/{id}/vote    PUT  …/vote { value }                 key required
//!   PUT    /api/sessions/stations/{id}/follow  PUT  …/follow { on }                  key required
//!   POST   /api/sessions/rooms                 POST /v1/rooms { public: true, stationId, hostName? }   key required
//! ```
//!
//!   The window's Community key (`X-Stash-Community-Key`) goes with each call that has one, and nothing else of the
//!   window's: no cookies, no other headers. The service's status and JSON body come back as they are (the window checks
//!   every answer itself, as the web page does); a refusal made here has the service's own shape, `{ "error": "<code>" }`.
//!   A room's socket is not relayed: the window opens it itself, straight to the share service (`rooms` in the status), as
//!   the phones do. At most [`MOST_AT_ONCE`] calls are under way at once; past that, 503 `busy`.
//! - `POST /api/sessions/listening {on}` → 204: the window tuned in to a session, or left it (`GET` → `{on}`). While tuned in the DJ chooses
//!   what plays: the core refuses a skip, other songs and edits of the queue whoever asks ([`the_djs`]), and autoplay
//!   adds nothing. Leaving gives the queue back to the listener, autoplay too.
//! - `POST /api/player/songs`: songs someone else describes (the room's) played on this computer's player, found the way
//!   the phone's songs are ([`crate::remote::items_for`]). Autoplay adds nothing to the queue it makes (the room's next
//!   song follows), until that queue is replaced or added to.
//!
//! These routes are reached only by the window, behind the checks every `/api/` route has (see `serve::answer`).

use std::io::Read;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::link::crypto::{b64u, random_bytes};
use stash_core::link::remote::{Command, MAX_SONGS, PhoneSong, read_song};
use stash_core::player::Repeat;
use tiny_http::{Method, Request};
use url::Url;

use crate::serve::{Reply, State, header, refuse, reply, reply_json};

/// Where the share service is: a test's, or one run locally (`http://127.0.0.1:8811`). `off`, or empty: Sessions are
/// off on this computer.
const SHARE_ENV: &str = "STASH_SHARE_URL";
const SHARE_DEFAULT: &str = "https://stashfm.app";
/// The setting that keeps this computer's Community key.
const KEY_SETTING: &str = "community.key";
/// The header the key travels in, to this core and on to the share service.
const KEY_HEADER: &str = "X-Stash-Community-Key";
/// How long the share service is waited for, and how much of its answer is read.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ANSWER_CAP: u64 = 1_000_000;
/// A write's body: a station with a week of hours is well under this.
const WRITE_CAP: u64 = 4096;
/// How many rows Live now and Upcoming ask for (the service's cap is 50).
const LIVE_LIMIT: u32 = 50;
const UPCOMING_LIMIT: u32 = 20;
/// The longest DJ name a room is opened with, in UTF-16 units (as the web page and the service count it).
const MAX_HOST_NAME: usize = 40;
/// The furthest into a song a start may be (a day: no song is longer).
const POSITION_CAP_MS: f64 = 24.0 * 3600.0 * 1000.0;

/// Sessions on this computer: where the share service is, the queue autoplay leaves alone, and whether this computer
/// is tuned in to one.
pub(crate) struct Sessions {
    /// The share service's origin; `None`: Sessions are off here.
    share: Option<Url>,
    /// The id ([`stash_core::player::PlayerState::queue_id`]) of the queue `POST /api/player/songs` made for a session,
    /// which autoplay adds nothing to; 0 for none. A queue that replaces it has another id, so it is autoplay's again.
    quiet: AtomicU64,
    /// Tuned in to a session (the window says so, `POST /api/sessions/listening`): the DJ chooses what plays, so
    /// nothing here skips, starts other songs, adds to the queue or changes its order or speed, whoever asks (the
    /// window, the mini player, the tray, media keys, shortcuts, the phone). Kept only while Stash runs.
    listening: AtomicBool,
    /// Calls to the share service under way.
    asking: AtomicUsize,
}

/// What a control refused while tuned in says.
pub(crate) const TUNED_IN: &str = "You are tuned in to a session: the DJ plays";
/// How many calls to the share service may be under way at once: one that hangs holds that many of the core's request
/// workers, never all of them.
const MOST_AT_ONCE: usize = 4;

impl Sessions {
    /// As `STASH_SHARE_URL` says, else stashfm.app.
    pub(crate) fn from_env() -> Sessions {
        Sessions::at(std::env::var(SHARE_ENV).ok().as_deref())
    }

    /// With the share service at `named` (`None`: stashfm.app; `off` or empty: Sessions are off).
    pub(crate) fn at(named: Option<&str>) -> Sessions {
        Sessions {
            share: share_base(named),
            quiet: AtomicU64::new(0),
            listening: AtomicBool::new(false),
            asking: AtomicUsize::new(0),
        }
    }

    /// Whether this computer is tuned in to a session (see [`Sessions::listening`]'s field).
    pub(crate) fn listening(&self) -> bool {
        self.listening.load(Ordering::SeqCst)
    }

    /// Tuned in, or not any more. Leaving, what is left of the room's queue is the listener's own: autoplay too.
    fn set_listening(&self, on: bool) {
        self.listening.store(on, Ordering::SeqCst);
        if !on {
            self.hold_autoplay(None);
        }
    }

    /// A place among the calls to the share service under way, given back when dropped; `None` when they are all taken.
    fn asking(&self) -> Option<Asking<'_>> {
        if self.asking.fetch_add(1, Ordering::SeqCst) >= MOST_AT_ONCE {
            self.asking.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Asking(&self.asking))
    }

    /// Whether autoplay leaves the queue `queue_id` alone (a session's).
    pub(crate) fn holds_autoplay(&self, queue_id: u64) -> bool {
        queue_id != 0 && self.quiet.load(Ordering::SeqCst) == queue_id
    }

    /// Autoplay leaves the queue `queue_id` alone; `None`: any queue is autoplay's again.
    pub(crate) fn hold_autoplay(&self, queue_id: Option<u64>) {
        self.quiet.store(queue_id.unwrap_or(0), Ordering::SeqCst);
    }
}

/// A call to the share service under way (see [`Sessions::asking`]).
struct Asking<'a>(&'a AtomicUsize);

impl Drop for Asking<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether a control of the player is the DJ's while tuned in to a session: a skip, a jump, other songs started or
/// added, the queue edited or its order changed, a speed other than the song's own, a station started. Pausing,
/// seeking, the volume and the sound's own settings stay the listener's, and so do the room's own songs
/// (`/api/player/songs`). Setting shuffle off, repeat off or the speed back to 1 is never refused.
pub(crate) fn the_djs(path: &str, body: &Value) -> bool {
    match path {
        "/api/player/play"
        | "/api/player/enqueue"
        | "/api/player/next"
        | "/api/player/previous"
        | "/api/player/jump"
        | "/api/player/remove"
        | "/api/player/move"
        | "/api/radio/start" => true,
        "/api/player/shuffle" => body.get("on").and_then(Value::as_bool).unwrap_or(false),
        "/api/player/repeat" => {
            Repeat::from_word(body.get("mode").and_then(Value::as_str).unwrap_or("off")) != Repeat::Off
        }
        "/api/player/speed" => body.get("speed").and_then(Value::as_f64).is_some_and(|s| (s - 1.0).abs() > 1e-9),
        _ => false,
    }
}

/// [`the_djs`] for a command from the phone: it may pause and play, stop, seek and set the volume while tuned in, and
/// set shuffle or repeat off. Every command is named, so a new one is decided on here.
pub(crate) fn the_djs_command(command: &Command) -> bool {
    match command {
        Command::Play | Command::Pause | Command::Stop | Command::Seek { .. } | Command::Volume { .. } => false,
        Command::Shuffle { on } => *on,
        Command::Repeat { mode } => Repeat::from_word(mode) != Repeat::Off,
        Command::Next
        | Command::Prev
        | Command::Start { .. }
        | Command::Add { .. }
        | Command::Remove { .. }
        | Command::Move { .. }
        | Command::SkipTo { .. } => true,
    }
}

/// The share service's origin: `named`, else stashfm.app. `off` or empty is none. Only https, or http on this computer
/// (the share service run locally to try it), with no user name or password; any path is dropped (the routes are the
/// service's own, from its root).
fn share_base(named: Option<&str>) -> Option<Url> {
    let text = named.map(str::trim).unwrap_or(SHARE_DEFAULT);
    if text.is_empty() || text.eq_ignore_ascii_case("off") {
        return None;
    }
    let usable = Url::parse(text).ok().filter(|url| {
        let reach = match url.scheme() {
            "https" => url.host_str().is_some(),
            "http" => loopback(url),
            _ => false,
        };
        reach && url.username().is_empty() && url.password().is_none()
    });
    let Some(mut base) = usable else {
        eprintln!("{SHARE_ENV} is not an https address (or http on this computer): Sessions are off");
        return None;
    };
    base.set_path("/");
    base.set_query(None);
    base.set_fragment(None);
    Some(base)
}

/// Whether `url` names this computer.
fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Where the window opens a room's socket: the base it appends `/{code}/ws` (and `?r=1`) to.
fn rooms_socket(base: &Url) -> String {
    let scheme = if base.scheme() == "https" { "wss" } else { "ws" };
    let host = base.host_str().unwrap_or_default();
    match base.port() {
        Some(port) => format!("{scheme}://{host}:{port}/v1/rooms"),
        None => format!("{scheme}://{host}/v1/rooms"),
    }
}

/// A Community key as the share service takes one: base64url of 32 bytes, 43 characters.
fn is_key(text: &str) -> bool {
    text.len() == 43 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A station's id: the service's 8 letters or digits.
fn is_station_id(text: &str) -> bool {
    text.len() == 8 && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A refusal in the share service's own shape.
fn fail(status: u16, code: &str) -> Reply {
    reply_json(status, json!({ "error": code }))
}

fn header_of(request: &Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

// ------------------------------------------------------------------------------------------------ routes

/// `/api/community*` and `/api/sessions/*`. `None`: the path is not one of them.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if path == "/api/community" || path == "/api/community/join" {
        return community(state, request, path).map(Some);
    }
    if !path.starts_with("/api/sessions/") {
        return Ok(None);
    }
    let method = request.method().clone();
    let read = matches!(method, Method::Get | Method::Head);
    if path == "/api/sessions/status" {
        if !read {
            return Ok(Some(not_allowed(&["GET", "HEAD"])));
        }
        let offline = crate::serve::offline(state);
        let base = state.sessions.share.as_ref().filter(|_| !offline);
        return Ok(Some(reply_json(
            200,
            json!({ "on": base.is_some(), "rooms": base.map(rooms_socket), "offline": offline }),
        )));
    }
    if path == "/api/sessions/listening" {
        return listening_route(state, request).map(Some);
    }
    let Some(asked) = Asked::of(path) else {
        return Ok(Some(fail(404, "not_found")));
    };
    let allowed = asked.methods();
    if !allowed.contains(&method.as_str()) {
        return Ok(Some(not_allowed(allowed)));
    }
    let Some(base) = state.sessions.share.as_ref() else {
        return Ok(Some(fail(503, "sessions_off")));
    };
    // Stash set to work offline asks nothing of the network.
    if crate::serve::offline(state) {
        return Ok(Some(fail(503, "offline")));
    }
    // No query but the window's own key for this core (`k`): the service's queries are set here.
    let url = Url::parse(&format!("http://local{}", request.url()))?;
    if url.query_pairs().any(|(name, _)| name != "k") {
        return Ok(Some(fail(400, "bad_request")));
    }
    let key = header_of(request, KEY_HEADER);
    if key.as_deref().is_some_and(|k| !is_key(k)) || (asked.keyed(read) && key.is_none()) {
        return Ok(Some(fail(401, "bad_key")));
    }
    let body = match asked.body(request, read) {
        Ok(body) => body,
        Err(refusal) => return Ok(Some(refusal)),
    };
    // A share service that hangs holds a few of the core's request workers, never all of them.
    let Some(_asking) = state.sessions.asking() else {
        return Ok(Some(fail(503, "busy").with_header(header("Retry-After", "1"))));
    };
    let method = if read { "GET" } else { method.as_str() };
    Ok(Some(pass(base, method, &asked.upstream(), key.as_deref(), body)))
}

fn not_allowed(allowed: &[&str]) -> Reply {
    fail(405, "method_not_allowed").with_header(header("Allow", &allowed.join(", ")))
}

/// `POST /api/sessions/listening {on: bool}` → 204: the window tuned in to a session, or left it. `GET` → `{ on }`.
/// Nothing is asked of the share service.
fn listening_route(state: &State, request: &mut Request) -> Result<Reply> {
    match request.method() {
        Method::Get | Method::Head => {}
        Method::Post => {
            let Ok(body) = crate::serve::body_json(request) else {
                return Ok(refuse(400, "the body is not JSON"));
            };
            let Some(on) = body.get("on").and_then(Value::as_bool) else {
                return Ok(refuse(400, "on is true or false"));
            };
            state.sessions.set_listening(on);
            return Ok(reply(204, "text/plain", Vec::new()));
        }
        _ => return Ok(refuse(405, "listening is read with a GET and set with a POST")),
    }
    Ok(reply_json(200, json!({ "on": state.sessions.listening() })))
}

/// `GET /api/community`, `POST /api/community/join`.
fn community(state: &State, request: &Request, path: &str) -> Result<Reply> {
    let method = request.method();
    let join = path == "/api/community/join";
    if join && *method != Method::Post {
        return Ok(refuse(405, "joining is a POST"));
    }
    if !join && !matches!(method, Method::Get | Method::Head) {
        return Ok(refuse(405, "the key is read with a GET"));
    }
    // Read and made under one hold of the library: two joins at once make one key.
    let mut library = state.library.lock().unwrap();
    let kept = library.setting(KEY_SETTING)?.filter(|k| is_key(k));
    let key = match kept {
        Some(key) => Some(key),
        None if join => {
            let key = b64u(&random_bytes(32));
            library.set_setting(KEY_SETTING, &key)?;
            Some(key)
        }
        None => None,
    };
    Ok(reply_json(200, json!({ "key": key })))
}

/// One of the share service's routes, as the window asked for it (its id checked).
enum Asked {
    Live,
    Upcoming,
    Following,
    Mine,
    Create,
    Open,
    Station(String),
    Heard(String),
    Vote(String),
    Follow(String),
}

impl Asked {
    fn of(path: &str) -> Option<Asked> {
        let rest = path.strip_prefix("/api/sessions/")?;
        Some(match rest {
            "live" => Asked::Live,
            "upcoming" => Asked::Upcoming,
            "following" => Asked::Following,
            "stations/mine" => Asked::Mine,
            "stations" => Asked::Create,
            "rooms" => Asked::Open,
            _ => {
                let mut parts = rest.strip_prefix("stations/")?.split('/');
                let id = parts.next().filter(|id| is_station_id(id))?.to_string();
                let asked = match parts.next() {
                    None => Asked::Station(id),
                    Some("heard") => Asked::Heard(id),
                    Some("vote") => Asked::Vote(id),
                    Some("follow") => Asked::Follow(id),
                    Some(_) => return None,
                };
                if parts.next().is_some() {
                    return None;
                }
                asked
            }
        })
    }

    fn methods(&self) -> &'static [&'static str] {
        match self {
            Asked::Live | Asked::Upcoming | Asked::Following | Asked::Mine => &["GET", "HEAD"],
            Asked::Create | Asked::Open => &["POST"],
            Asked::Station(_) => &["GET", "HEAD", "PATCH", "DELETE"],
            Asked::Heard(_) | Asked::Vote(_) | Asked::Follow(_) => &["PUT"],
        }
    }

    /// Whether the route needs the window's key: all but the lists anyone may read and a station's page.
    fn keyed(&self, read: bool) -> bool {
        !matches!(self, Asked::Live | Asked::Upcoming) && !(matches!(self, Asked::Station(_)) && read)
    }

    /// The service's path (and query).
    fn upstream(&self) -> String {
        match self {
            Asked::Live => format!("/v1/community/live?limit={LIVE_LIMIT}"),
            Asked::Upcoming => format!("/v1/community/upcoming?limit={UPCOMING_LIMIT}"),
            Asked::Following => "/v1/community/stations/following".into(),
            Asked::Mine => "/v1/community/stations/mine".into(),
            Asked::Create => "/v1/community/stations".into(),
            Asked::Open => "/v1/rooms".into(),
            Asked::Station(id) => format!("/v1/community/stations/{id}"),
            Asked::Heard(id) => format!("/v1/community/stations/{id}/heard"),
            Asked::Vote(id) => format!("/v1/community/stations/{id}/vote"),
            Asked::Follow(id) => format!("/v1/community/stations/{id}/follow"),
        }
    }

    /// The body the service is sent, from the window's: none for a read, a heard mark or a delete; a vote, a follow and
    /// a room's opening rebuilt from their known fields, as the web player does; a station as the window wrote it (the
    /// service checks every field of it). A refusal when the window's body is not one.
    fn body(&self, request: &mut Request, read: bool) -> std::result::Result<Option<Vec<u8>>, Reply> {
        if read || matches!(self, Asked::Heard(_)) || *request.method() == Method::Delete {
            return Ok(None);
        }
        let sent = written(request)?;
        let rebuilt = match self {
            // -1, 0 or 1 (a whole number: 1.0 is not a vote, as on the web).
            Asked::Vote(_) => match sent.get("value").and_then(Value::as_i64) {
                Some(value @ -1..=1) => json!({ "value": value }),
                _ => return Err(fail(400, "bad_request")),
            },
            Asked::Follow(_) => match sent.get("on").and_then(Value::as_bool) {
                Some(on) => json!({ "on": on }),
                None => return Err(fail(400, "bad_request")),
            },
            Asked::Open => {
                let Some(station) = sent.get("stationId").and_then(Value::as_str).filter(|id| is_station_id(id)) else {
                    return Err(fail(400, "bad_request"));
                };
                let host_name = match sent.get("hostName") {
                    Some(Value::String(name)) => name.trim(),
                    _ => "",
                };
                if host_name.encode_utf16().count() > MAX_HOST_NAME {
                    return Err(fail(400, "bad_request"));
                }
                let mut opening = json!({ "public": true, "stationId": station });
                if !host_name.is_empty() {
                    opening["hostName"] = json!(host_name);
                }
                opening
            }
            _ if sent.is_object() => sent,
            _ => return Err(fail(400, "bad_request")),
        };
        Ok(Some(rebuilt.to_string().into_bytes()))
    }
}

/// The window's JSON body of a write, at most [`WRITE_CAP`] bytes; or the refusal.
fn written(request: &mut Request) -> std::result::Result<Value, Reply> {
    let json = header_of(request, "Content-Type")
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("application/json"));
    if !json {
        return Err(fail(415, "bad_request"));
    }
    if request.body_length().is_some_and(|length| length as u64 > WRITE_CAP) {
        return Err(fail(413, "too_large"));
    }
    let mut bytes = Vec::new();
    if request.as_reader().take(WRITE_CAP + 1).read_to_end(&mut bytes).is_err() {
        return Err(fail(400, "bad_request"));
    }
    if bytes.len() as u64 > WRITE_CAP {
        return Err(fail(413, "too_large"));
    }
    serde_json::from_slice(&bytes).map_err(|_| fail(400, "bad_request"))
}

/// The agent the share service is asked with: no redirect is followed (an answer that is one is not JSON), and every
/// call is over in [`UPSTREAM_TIMEOUT`]. The base it is pointed at is https, or http on this computer ([`share_base`]).
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(UPSTREAM_TIMEOUT))
            .user_agent(concat!("Stash-desktop/", env!("CARGO_PKG_VERSION")))
            .accept("application/json")
            .build()
            .into()
    })
}

/// Asks the share service and hands its answer back as it is: its status and its JSON body (and, for a 429 or a 503,
/// how long to wait). Not reached: 502 `unreachable`. An answer that is not JSON, too long, or a redirect: 502
/// `bad_answer`.
fn pass(base: &Url, method: &str, path: &str, key: Option<&str>, body: Option<Vec<u8>>) -> Reply {
    let Ok(url) = base.join(path) else {
        return fail(502, "unreachable");
    };
    let mut asked = ureq::http::Request::builder().method(method).uri(url.as_str());
    if let Some(key) = key {
        asked = asked.header(KEY_HEADER, key);
    }
    let sent = match body {
        Some(bytes) => asked
            .header("Content-Type", "application/json")
            .body(bytes)
            .map(|request| agent().run(request)),
        None => asked.body(()).map(|request| agent().run(request)),
    };
    let mut answer = match sent {
        Ok(Ok(answer)) => answer,
        _ => return fail(502, "unreachable"),
    };
    let status = answer.status().as_u16();
    let said = |name: &str| {
        answer
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let is_json = said("content-type")
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("application/json"));
    let wait = said("retry-after")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|s| (1..=3600).contains(s));
    if !(200..600).contains(&status) || (300..400).contains(&status) {
        return fail(502, "bad_answer");
    }
    let bytes = match answer.body_mut().with_config().limit(ANSWER_CAP).read_to_vec() {
        Ok(bytes) => bytes,
        Err(ureq::Error::BodyExceedsLimit(_)) => return fail(502, "bad_answer"),
        Err(_) => return fail(502, "unreachable"),
    };
    let mut reply = if bytes.is_empty() && matches!(status, 204 | 205) {
        reply(status, "application/json", Vec::new())
    } else if is_json && serde_json::from_slice::<Value>(&bytes).is_ok() {
        reply(status, "application/json", bytes)
    } else {
        return fail(502, "bad_answer");
    };
    if let Some(seconds) = wait.filter(|_| matches!(status, 429 | 503)) {
        reply = reply.with_header(header("Retry-After", &seconds.to_string()));
    }
    reply
}

// ------------------------------------------------------------------------------------------------ the room's songs

/// What `POST /api/player/songs` asks, read and checked.
struct Play {
    songs: Vec<PhoneSong>,
    at: usize,
    position_ms: f64,
    play: bool,
    autoplay: bool,
    from: Option<String>,
}

impl Play {
    /// `{ songs: [{ title, artist, album?, isrc?, duration_ms?, art?, youtube? }], at?, position_ms?, play?, autoplay?,
    /// from? }`; why it is not one.
    fn read(body: &Value) -> std::result::Result<Play, String> {
        if !body.is_object() {
            return Err("the body is an object".into());
        }
        let given = |name: &str| body.get(name).filter(|v| !v.is_null());
        let songs = read_songs(body)?;
        let at = match given("at") {
            None => 0,
            Some(at) => at
                .as_u64()
                .and_then(|at| usize::try_from(at).ok())
                .filter(|at| *at < songs.len())
                .ok_or("at is the place of one of the songs")?,
        };
        let position_ms = match given("position_ms") {
            None => 0.0,
            Some(ms) => ms
                .as_f64()
                .filter(|ms| ms.is_finite() && (0.0..=POSITION_CAP_MS).contains(ms))
                .ok_or("position_ms is a place in the song, in milliseconds")?,
        };
        let flag = |name: &str, default: bool| match given(name) {
            None => Ok(default),
            Some(v) => v.as_bool().ok_or(format!("{name} is true or false")),
        };
        let from = match given("from") {
            None => None,
            Some(v) => Some(v.as_str().ok_or("from is words")?),
        };
        Ok(Play {
            songs,
            at,
            position_ms,
            play: flag("play", true)?,
            autoplay: flag("autoplay", false)?,
            from: from.map(crate::serve::capped).filter(|f| !f.is_empty()),
        })
    }
}

/// The `songs` of a body: 1 to [`MAX_SONGS`] songs someone else describes (`{ title, artist, album?, isrc?,
/// duration_ms?, art?, youtube? }`), each read by [`song_of`]; why they are not. Shared by `POST /api/player/songs` and
/// `POST /api/player/enqueue` (a listener's request the DJ took).
pub(crate) fn read_songs(body: &Value) -> std::result::Result<Vec<PhoneSong>, String> {
    let listed = body
        .get("songs")
        .filter(|v| !v.is_null())
        .and_then(Value::as_array)
        .ok_or("songs is a list")?;
    if listed.is_empty() || listed.len() > MAX_SONGS {
        return Err(format!("songs holds 1 to {MAX_SONGS} songs"));
    }
    listed
        .iter()
        .enumerate()
        .map(|(n, song)| song_of(song).ok_or_else(|| format!("song {n} needs a title and an artist")))
        .collect()
}

/// One song as the window describes it, read by the rules a song from the phone is read by (`read_song`, the library
/// file's Song): a title and an artist; the rest is left out when it is not what it should be.
fn song_of(v: &Value) -> Option<PhoneSong> {
    if !v.is_object() {
        return None;
    }
    let field = |name: &str| v.get(name).cloned().unwrap_or(Value::Null);
    read_song(&json!({
        "title": field("title"),
        "artist": field("artist"),
        "album": field("album"),
        "isrc": field("isrc"),
        "durationMs": field("duration_ms"),
        "refs": { "youtube": field("youtube") },
        "artwork": v.get("art").map(|url| json!([{ "url": url }])).unwrap_or(Value::Null),
    }))
}

/// `POST /api/player/songs`: plays songs someone else describes (a room's) on this computer's player, from the song at
/// `at`, `position_ms` into it, paused when `play` is false. Autoplay adds nothing to that queue unless `autoplay` is
/// true. Answers `{ queued, qids, start_qid }`: how many songs went in the queue, their queue numbers in its order, and
/// the number of the one it starts on.
pub(crate) fn play_songs(state: &State, request: &mut Request) -> Result<Reply> {
    let player = match &state.player {
        Ok(player) => player,
        Err(why) => return Ok(refuse(503, why)),
    };
    let Ok(body) = crate::serve::body_json(request) else {
        return Ok(refuse(400, "the body is not JSON"));
    };
    let asked = match Play::read(&body) {
        Ok(asked) => asked,
        Err(why) => return Ok(refuse(400, &why)),
    };
    let (items, start) = crate::remote::items_for(&state.library.lock().unwrap(), &asked.songs, asked.at);
    if items.is_empty() {
        return Ok(refuse(409, crate::blocking::ALL_BLOCKED));
    }
    let Some((items, start)) = crate::serve::offline_queue(state, items, start)? else {
        return Ok(refuse(409, crate::serve::NOT_HERE_OFFLINE));
    };
    let queued = player.play_from_at(items, start, asked.from, asked.position_ms / 1000.0, asked.play)?;
    state.sessions.hold_autoplay((!asked.autoplay).then_some(queued.queue_id));
    Ok(reply_json(
        200,
        json!({ "queued": queued.qids.len(), "qids": queued.qids, "start_qid": queued.start }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuned_in_what_plays_is_the_djs_and_pausing_seeking_and_the_volume_are_the_listeners() {
        for (path, body) in [
            ("/api/player/next", json!({})),
            ("/api/player/previous", json!({})),
            ("/api/player/jump", json!({ "index": 2 })),
            ("/api/player/play", json!({ "items": [] })),
            ("/api/player/enqueue", json!({ "songs": [] })),
            ("/api/player/remove", json!({ "index": 1 })),
            ("/api/player/move", json!({ "from": 1, "to": 2 })),
            ("/api/radio/start", json!({ "playing": true })),
            ("/api/player/shuffle", json!({ "on": true })),
            ("/api/player/repeat", json!({ "mode": "all" })),
            ("/api/player/repeat", json!({ "mode": "one" })),
            ("/api/player/speed", json!({ "speed": 0.75 })),
        ] {
            assert!(the_djs(path, &body), "{path} {body}");
        }
        for (path, body) in [
            ("/api/player/songs", json!({ "songs": [] })),
            ("/api/player/pause", json!({ "paused": true })),
            ("/api/player/seek", json!({ "seconds": 3 })),
            ("/api/player/volume", json!({ "percent": 40 })),
            ("/api/player/stop", json!({})),
            ("/api/player/shuffle", json!({ "on": false })),
            ("/api/player/shuffle", json!({})),
            ("/api/player/repeat", json!({ "mode": "off" })),
            ("/api/player/repeat", json!({ "mode": "sideways" })),
            ("/api/player/speed", json!({ "speed": 1 })),
            // Not a speed: the route itself refuses it.
            ("/api/player/speed", json!({ "speed": "fast" })),
            ("/api/player/tuning", json!({ "hz": 432 })),
            ("/api/player/replaygain", json!({ "mode": "track" })),
            ("/api/player", json!({})),
        ] {
            assert!(!the_djs(path, &body), "{path} {body}");
        }

        let mine = [
            Command::Play,
            Command::Pause,
            Command::Stop,
            Command::Seek { position_ms: 1000 },
            Command::Volume { volume: 0.5 },
            Command::Shuffle { on: false },
            Command::Repeat { mode: "off".into() },
        ];
        assert!(mine.iter().all(|c| !the_djs_command(c)));
        let theirs = [
            Command::Next,
            Command::Prev,
            Command::Start { songs: Vec::new(), at: 0, position_ms: 0, play: true, from: None },
            Command::Add { songs: Vec::new(), next: true },
            Command::Remove { iid: "a".into() },
            Command::Move { iid: "a".into(), to: 0 },
            Command::SkipTo { iid: "a".into() },
            Command::Shuffle { on: true },
            Command::Repeat { mode: "one".into() },
        ];
        assert!(theirs.iter().all(the_djs_command));
    }

    #[test]
    fn calls_to_the_share_service_take_a_place_and_give_it_back() {
        let sessions = Sessions::at(Some("off"));
        let held: Vec<_> = (0..MOST_AT_ONCE).map(|_| sessions.asking().expect("a place")).collect();
        assert!(sessions.asking().is_none(), "more than {MOST_AT_ONCE} at once");
        drop(held);
        assert!(sessions.asking().is_some(), "the places were not given back");
        assert_eq!(sessions.asking.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn leaving_a_session_lets_autoplay_add_to_the_rooms_queue_again() {
        let sessions = Sessions::at(Some("off"));
        sessions.hold_autoplay(Some(7));
        sessions.set_listening(true);
        assert!(sessions.listening() && sessions.holds_autoplay(7));
        sessions.set_listening(false);
        assert!(!sessions.listening() && !sessions.holds_autoplay(7));
    }

    #[test]
    fn the_share_service_is_https_or_on_this_computer_and_off_is_off() {
        let base = |named: Option<&str>| share_base(named).map(|u| u.to_string());
        assert_eq!(base(None).as_deref(), Some("https://stashfm.app/"));
        assert_eq!(base(Some("https://share.example/some/path?x=1#y")).as_deref(), Some("https://share.example/"));
        assert_eq!(base(Some("http://127.0.0.1:8811")).as_deref(), Some("http://127.0.0.1:8811/"));
        assert_eq!(base(Some(" http://localhost:8811/ ")).as_deref(), Some("http://localhost:8811/"));
        assert_eq!(base(Some("http://[::1]:8811")).as_deref(), Some("http://[::1]:8811/"));
        for off in ["off", "OFF", "", "  "] {
            assert_eq!(base(Some(off)), None, "{off:?}");
        }
        for wrong in ["http://stashfm.app", "ftp://stashfm.app", "https://user:pw@stashfm.app", "stashfm.app"] {
            assert_eq!(base(Some(wrong)), None, "{wrong}");
        }
    }

    #[test]
    fn a_rooms_socket_is_wss_for_https_and_ws_on_this_computer() {
        let socket = |text: &str| rooms_socket(&share_base(Some(text)).unwrap());
        assert_eq!(socket("https://stashfm.app"), "wss://stashfm.app/v1/rooms");
        assert_eq!(socket("https://share.example:8443"), "wss://share.example:8443/v1/rooms");
        assert_eq!(socket("http://127.0.0.1:8811"), "ws://127.0.0.1:8811/v1/rooms");
        assert_eq!(socket("http://[::1]:8811"), "ws://[::1]:8811/v1/rooms");
    }

    #[test]
    fn the_routes_and_their_ids_are_the_web_players() {
        let upstream = |path: &str| Asked::of(path).map(|a| a.upstream());
        assert_eq!(upstream("/api/sessions/live").as_deref(), Some("/v1/community/live?limit=50"));
        assert_eq!(upstream("/api/sessions/upcoming").as_deref(), Some("/v1/community/upcoming?limit=20"));
        assert_eq!(upstream("/api/sessions/stations/mine").as_deref(), Some("/v1/community/stations/mine"));
        assert_eq!(upstream("/api/sessions/rooms").as_deref(), Some("/v1/rooms"));
        assert_eq!(upstream("/api/sessions/stations/abcd1234").as_deref(), Some("/v1/community/stations/abcd1234"));
        assert_eq!(upstream("/api/sessions/stations/abcd1234/vote").as_deref(), Some("/v1/community/stations/abcd1234/vote"));
        for wrong in [
            "/api/sessions/stations/abc",
            "/api/sessions/stations/abcd-234",
            "/api/sessions/stations/abcd12345",
            "/api/sessions/stations/abcd1234/other",
            "/api/sessions/stations/abcd1234/vote/again",
            "/api/sessions/rooms/ABCDEFGH/ws",
            "/api/sessions/nothing",
        ] {
            assert!(Asked::of(wrong).is_none(), "{wrong}");
        }
        // A key: everywhere but the lists anyone reads and a station's page.
        let keyed = |path: &str, read: bool| Asked::of(path).unwrap().keyed(read);
        assert!(!keyed("/api/sessions/live", true));
        assert!(!keyed("/api/sessions/upcoming", true));
        assert!(!keyed("/api/sessions/stations/abcd1234", true));
        assert!(keyed("/api/sessions/stations/abcd1234", false));
        assert!(keyed("/api/sessions/following", true));
        assert!(keyed("/api/sessions/stations/mine", true));
        assert!(keyed("/api/sessions/stations/abcd1234/heard", false));
        assert!(keyed("/api/sessions/rooms", false));
    }

    #[test]
    fn a_community_key_is_43_base64url_letters() {
        assert!(is_key(&b64u(&random_bytes(32))));
        assert!(!is_key(&"a".repeat(42)));
        assert!(!is_key(&format!("{}+", "a".repeat(42))));
        assert!(!is_key(&format!("{}=", "a".repeat(42))));
    }

    #[test]
    fn the_songs_to_play_are_read_and_checked() {
        let read = |body: Value| Play::read(&body);
        let song = json!({ "title": " Song ", "artist": "Band", "album": "Album", "isrc": "usabc1234567",
            "duration_ms": 215000, "art": "https://img.example/a.jpg", "youtube": "dQw4w9WgXcQ" });
        let asked = read(json!({ "songs": [song, { "title": "Two", "artist": "Band" }], "at": 1, "position_ms": 1500.5,
            "play": false, "autoplay": true, "from": "A session" }))
        .unwrap();
        assert_eq!(
            asked.songs[0],
            PhoneSong {
                title: "Song".into(),
                artist: "Band".into(),
                album: Some("Album".into()),
                duration_ms: Some(215_000),
                isrc: Some("USABC1234567".into()),
                youtube: Some("dQw4w9WgXcQ".into()),
                art: Some("https://img.example/a.jpg".into()),
            }
        );
        assert_eq!((asked.at, asked.position_ms, asked.play, asked.autoplay), (1, 1500.5, false, true));
        assert_eq!(asked.from.as_deref(), Some("A session"));
        // As little as a title and an artist: from the top, playing, autoplay held; what is not right is left out.
        let plain = read(json!({ "songs": [{ "title": "One", "artist": "Band", "art": "http://img.example/a.jpg",
            "youtube": "no spaces", "from": null }], "from": null, "at": null })).unwrap();
        assert_eq!((plain.at, plain.position_ms, plain.play, plain.autoplay, plain.from), (0, 0.0, true, false, None));
        assert_eq!((plain.songs[0].art.as_ref(), plain.songs[0].youtube.as_ref()), (None, None));
        for wrong in [
            json!([]),
            json!({}),
            json!({ "songs": [] }),
            json!({ "songs": "one" }),
            json!({ "songs": [{ "title": "No artist" }] }),
            json!({ "songs": [{ "title": "", "artist": "Band" }] }),
            json!({ "songs": ["Song"] }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "at": 1 }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "at": -1 }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "position_ms": -5 }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "position_ms": "10" }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "play": "yes" }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "autoplay": 1 }),
            json!({ "songs": [{ "title": "One", "artist": "Band" }], "from": 3 }),
            json!({ "songs": vec![json!({ "title": "One", "artist": "Band" }); MAX_SONGS + 1] }),
        ] {
            assert!(read(wrong.clone()).is_err(), "{wrong}");
        }
    }
}
