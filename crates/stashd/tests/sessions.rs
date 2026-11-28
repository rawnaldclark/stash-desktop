//! Sessions from the window's side: this computer's Community key, the share service's routes passed on to a stand-in
//! for it (what goes, what comes back, what is refused here), and a room's songs played on this computer's player. The
//! player part needs libmpv; without it that test says so and passes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::Library;
use stash_core::audio::Output;
use stash_core::scan::scan_folder;
use stashd::serve::{Core, Options};

fn engine() -> Option<PathBuf> {
    if let Some(path) = stash_core::audio::find_library() {
        return Some(path);
    }
    let name = if cfg!(windows) {
        "libmpv-2.dll"
    } else if cfg!(target_os = "macos") {
        "libmpv.2.dylib"
    } else {
        "libmpv.so.2"
    };
    let bundled = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/mpv")
        .join(name);
    bundled.is_file().then_some(bundled)
}

/// A WAV of `seconds` of silence, 16-bit stereo at 44.1 kHz, with no tags: the scan names it after the file.
fn silence_for(path: &Path, seconds: u32) {
    let data: u32 = 44_100 * 4 * seconds;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&44_100u32.to_le_bytes());
    bytes.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    bytes.resize(44 + data as usize, 0);
    std::fs::write(path, bytes).unwrap();
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stash-sessions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A core on the library at `db` (no player unless `audio`), running on a thread of its own; `share` is where its
/// Sessions go. Answers its address.
fn core(db: &Path, engine: Option<PathBuf>, share: &str) -> String {
    let core = Core::bind(Options {
        db: db.to_path_buf(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: engine.is_some().then_some(Output::Silent),
        mpv: engine,
        play_counts_between_seconds: None,
    })
    .unwrap()
    .with_share(share);
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    base
}

/// An answer, whatever its status.
struct Answer {
    status: u16,
    text: String,
    headers: ureq::http::HeaderMap,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

struct Client {
    agent: ureq::Agent,
    base: String,
}

impl Client {
    fn new(base: &str) -> Client {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Client { agent, base: base.to_string() }
    }

    fn ask(&self, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> Answer {
        let mut asked = ureq::http::Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base));
        for (name, value) in headers {
            asked = asked.header(*name, *value);
        }
        let mut answer = match body {
            Some(body) => self.agent.run(asked.body(body.to_string()).unwrap()),
            None => self.agent.run(asked.body(()).unwrap()),
        }
        .unwrap();
        let status = answer.status().as_u16();
        let headers = answer.headers().clone();
        let text = answer.body_mut().read_to_string().unwrap_or_default();
        Answer { status, text, headers }
    }

    fn get(&self, path: &str) -> Answer {
        self.ask("GET", path, &[], None)
    }

    /// A JSON body sent with `method`, and the window's Community key when there is one.
    fn send(&self, method: &str, path: &str, key: Option<&str>, body: &Value) -> Answer {
        let text = body.to_string();
        let mut headers = vec![("Content-Type", "application/json")];
        if let Some(key) = key {
            headers.push(("X-Stash-Community-Key", key));
        }
        self.ask(method, path, &headers, Some(&text))
    }
}

/// A request the stand-in for the share service took.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// What the stand-in answers next.
#[derive(Clone)]
struct Canned {
    status: u16,
    content_type: Option<&'static str>,
    body: String,
    retry_after: Option<&'static str>,
}

/// A stand-in for the share service: it keeps every request and answers each with what it was last told to, one at a
/// time, after `slow`.
struct Upstream {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    next: Arc<Mutex<Canned>>,
    slow: Arc<Mutex<Duration>>,
}

impl Upstream {
    fn start() -> Upstream {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let next = Arc::new(Mutex::new(Canned {
            status: 200,
            content_type: Some("application/json"),
            body: "{}".into(),
            retry_after: None,
        }));
        let slow = Arc::new(Mutex::new(Duration::ZERO));
        let (kept, told, waits) = (Arc::clone(&seen), Arc::clone(&next), Arc::clone(&slow));
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let wait = *waits.lock().unwrap();
                std::thread::sleep(wait);
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                kept.lock().unwrap().push(Seen {
                    method: request.method().as_str().to_string(),
                    url: request.url().to_string(),
                    headers: request
                        .headers()
                        .iter()
                        .map(|h| (h.field.to_string(), h.value.as_str().to_string()))
                        .collect(),
                    body,
                });
                let canned = told.lock().unwrap().clone();
                let mut response =
                    tiny_http::Response::from_data(canned.body.into_bytes()).with_status_code(canned.status);
                if let Some(kind) = canned.content_type {
                    response.add_header(tiny_http::Header::from_bytes("Content-Type", kind).unwrap());
                }
                if let Some(wait) = canned.retry_after {
                    response.add_header(tiny_http::Header::from_bytes("Retry-After", wait).unwrap());
                }
                let _ = request.respond(response);
            }
        });
        Upstream { url, seen, next, slow }
    }

    /// Each answer waits `wait` from now on.
    fn be_slow(&self, wait: Duration) {
        *self.slow.lock().unwrap() = wait;
    }

    fn answer(&self, status: u16, body: Value) {
        self.answer_as(Canned {
            status,
            content_type: Some("application/json"),
            body: if body.is_null() { String::new() } else { body.to_string() },
            retry_after: None,
        });
    }

    fn answer_as(&self, canned: Canned) {
        *self.next.lock().unwrap() = canned;
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().expect("the share service was asked")
    }
}

const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA_";

#[test]
fn sessions_routes_go_to_the_share_service_with_the_windows_key_and_nothing_else() {
    let dir = scratch("proxy");
    let share = Upstream::start();
    let client = Client::new(&core(&dir.join("library.db"), None, &share.url));
    let port = share.url.rsplit(':').next().unwrap().to_string();

    // On, and where the window opens a room's socket (it opens it itself: nothing is relayed here).
    let status = client.get("/api/sessions/status");
    assert_eq!(
        (status.status, status.json()),
        (200, json!({ "on": true, "rooms": format!("ws://127.0.0.1:{port}/v1/rooms"), "offline": false }))
    );
    assert_eq!(client.get("/api/sessions/rooms/ABCDEFGH/ws").status, 404);
    assert_eq!(share.count(), 0, "the status is answered here");

    // Live now needs no key; the window's own headers stay here, and the service's answer comes back as it is.
    let live = r#"{"sessions":[{"room":"ABCDEFGH","listeners":3,"extra":"kept as it is"}]}"#;
    share.answer_as(Canned { status: 200, content_type: Some("application/json"), body: live.into(), retry_after: None });
    let answer = client.ask(
        "GET",
        "/api/sessions/live",
        &[("Cookie", "session=private"), ("X-Other", "1"), ("Authorization", "Bearer private")],
        None,
    );
    assert_eq!((answer.status, answer.text.as_str()), (200, live));
    assert_eq!(answer.header("content-type"), Some("application/json"));
    let asked = share.last();
    assert_eq!((asked.method.as_str(), asked.url.as_str()), ("GET", "/v1/community/live?limit=50"));
    for private in ["Cookie", "X-Other", "Authorization", "X-Stash-Community-Key"] {
        assert_eq!(asked.header(private), None, "{private} went to the share service");
    }
    assert_eq!(asked.header("Accept"), Some("application/json"));
    assert!(asked.header("User-Agent").is_some_and(|ua| ua.starts_with("Stash-desktop/")), "{asked:?}");
    client.get("/api/sessions/upcoming");
    assert_eq!(share.last().url, "/v1/community/upcoming?limit=20");

    // A route that needs a key: refused here without one, or with one that isn't one; the key goes on when it is one.
    let before = share.count();
    let no_key = client.get("/api/sessions/following");
    assert_eq!((no_key.status, no_key.json()), (401, json!({ "error": "bad_key" })));
    let bad = client.ask("GET", "/api/sessions/stations/mine", &[("X-Stash-Community-Key", "short")], None);
    assert_eq!((bad.status, bad.json()), (401, json!({ "error": "bad_key" })));
    let bad = client.ask("GET", "/api/sessions/live", &[("X-Stash-Community-Key", "not+a+key")], None);
    assert_eq!(bad.status, 401, "a key that isn't one is refused even where none is needed");
    assert_eq!(share.count(), before, "nothing went to the share service");
    share.answer(200, json!({ "stations": [] }));
    let followed = client.ask("GET", "/api/sessions/following", &[("X-Stash-Community-Key", KEY)], None);
    assert_eq!((followed.status, followed.json()), (200, json!({ "stations": [] })));
    let asked = share.last();
    assert_eq!(asked.url, "/v1/community/stations/following");
    assert_eq!(asked.header("X-Stash-Community-Key"), Some(KEY));
    client.ask("GET", "/api/sessions/stations/mine", &[("X-Stash-Community-Key", KEY)], None);
    assert_eq!(share.last().url, "/v1/community/stations/mine");

    // A station's page needs none; an id that isn't one is no route.
    client.get("/api/sessions/stations/abcd1234");
    assert_eq!(
        (share.last().url.as_str(), share.last().header("X-Stash-Community-Key")),
        ("/v1/community/stations/abcd1234", None)
    );
    let before = share.count();
    for wrong in ["/api/sessions/stations/abc", "/api/sessions/stations/abcd-123", "/api/sessions/nothing"] {
        let answer = client.get(wrong);
        assert_eq!((answer.status, answer.json()), (404, json!({ "error": "not_found" })), "{wrong}");
    }
    // A method the route doesn't take, and a query the window may not add (its own `k` for this core is fine).
    let answer = client.ask("DELETE", "/api/sessions/live", &[], None);
    assert_eq!((answer.status, answer.header("allow")), (405, Some("GET, HEAD")));
    assert_eq!(client.get("/api/sessions/live?limit=500").status, 400);
    assert_eq!(share.count(), before);
    client.get("/api/sessions/live?k=the-windows-key");
    assert_eq!(share.last().url, "/v1/community/live?limit=50");

    // Refusals pass through as the service said them: the status, and the body.
    share.answer(409, json!({ "error": "have_station", "id": "abcd1234" }));
    let station = json!({ "name": "Night shift", "vibe": "slow", "schedule": null });
    let answer = client.send("POST", "/api/sessions/stations", Some(KEY), &station);
    assert_eq!((answer.status, answer.json()), (409, json!({ "error": "have_station", "id": "abcd1234" })));
    let asked = share.last();
    assert_eq!((asked.method.as_str(), asked.url.as_str()), ("POST", "/v1/community/stations"));
    assert_eq!((asked.json(), asked.header("Content-Type")), (station.clone(), Some("application/json")));
    share.answer(503, json!({ "error": "sessions_off" }));
    let off = client.get("/api/sessions/live");
    assert_eq!((off.status, off.json()), (503, json!({ "error": "sessions_off" })));
    share.answer_as(Canned {
        status: 429,
        content_type: Some("application/json"),
        body: json!({ "error": "rate_limited" }).to_string(),
        retry_after: Some("30"),
    });
    let slow = client.get("/api/sessions/live");
    assert_eq!((slow.status, slow.json(), slow.header("retry-after")), (429, json!({ "error": "rate_limited" }), Some("30")));

    // A change, a delete and a heard mark go with their methods; what has no body says so.
    share.answer(200, json!({ "station": { "id": "abcd1234" } }));
    let change = json!({ "vibe": "faster" });
    assert_eq!(client.send("PATCH", "/api/sessions/stations/abcd1234", Some(KEY), &change).status, 200);
    let asked = share.last();
    assert_eq!((asked.method.as_str(), asked.json()), ("PATCH", change));
    share.answer(204, Value::Null);
    let gone = client.ask("DELETE", "/api/sessions/stations/abcd1234", &[("X-Stash-Community-Key", KEY)], None);
    assert_eq!((gone.status, gone.text.as_str()), (204, ""));
    assert_eq!((share.last().method.as_str(), share.last().url.as_str()), ("DELETE", "/v1/community/stations/abcd1234"));
    let heard = client.ask("PUT", "/api/sessions/stations/abcd1234/heard", &[("X-Stash-Community-Key", KEY)], None);
    assert_eq!(heard.status, 204);
    let asked = share.last();
    assert_eq!((asked.method.as_str(), asked.url.as_str(), asked.body.as_str()), ("PUT", "/v1/community/stations/abcd1234/heard", ""));

    // A vote and a follow are rebuilt from their known fields, and checked here first.
    share.answer(200, json!({ "up": 1, "down": 0, "myVote": 1 }));
    let vote = client.send("PUT", "/api/sessions/stations/abcd1234/vote", Some(KEY), &json!({ "value": 1, "more": "x" }));
    assert_eq!((vote.status, vote.json()), (200, json!({ "up": 1, "down": 0, "myVote": 1 })));
    assert_eq!(share.last().json(), json!({ "value": 1 }));
    share.answer(200, json!({ "following": true, "followers": 4 }));
    client.send("PUT", "/api/sessions/stations/abcd1234/follow", Some(KEY), &json!({ "on": true }));
    assert_eq!((share.last().url.as_str(), share.last().json()), ("/v1/community/stations/abcd1234/follow", json!({ "on": true })));
    let before = share.count();
    for (path, body) in [
        ("/api/sessions/stations/abcd1234/vote", json!({ "value": 2 })),
        ("/api/sessions/stations/abcd1234/vote", json!({ "value": 1.5 })),
        ("/api/sessions/stations/abcd1234/follow", json!({ "on": "yes" })),
        ("/api/sessions/stations", json!(["not", "a", "station"])),
        ("/api/sessions/rooms", json!({ "stationId": "abc" })),
        ("/api/sessions/rooms", json!({ "stationId": "abcd1234", "hostName": "x".repeat(41) })),
    ] {
        let answer = client.send(if path.contains("/rooms") || path.ends_with("stations") { "POST" } else { "PUT" }, path, Some(KEY), &body);
        assert_eq!((answer.status, answer.json()), (400, json!({ "error": "bad_request" })), "{path} {body}");
    }
    // A write's body is JSON, and small.
    let answer = client.ask(
        "POST",
        "/api/sessions/stations",
        &[("Content-Type", "text/plain"), ("X-Stash-Community-Key", KEY)],
        Some("{}"),
    );
    assert_eq!(answer.status, 415);
    let big = json!({ "name": "x".repeat(5000) });
    assert_eq!(client.send("POST", "/api/sessions/stations", Some(KEY), &big).status, 413);
    assert_eq!(share.count(), before, "nothing refused here went to the share service");

    // Going live: the service is sent `{ public: true, stationId, hostName? }`, and its answer comes back.
    let opened = json!({ "code": "ABCDEFGH", "hostKey": KEY, "url": "https://stashfm.app/s/abcd1234" });
    share.answer(201, opened.clone());
    let answer = client.send(
        "POST",
        "/api/sessions/rooms",
        Some(KEY),
        &json!({ "stationId": "abcd1234", "hostName": "  DJ Night  ", "public": false, "more": 1 }),
    );
    assert_eq!((answer.status, answer.json()), (201, opened));
    let asked = share.last();
    assert_eq!((asked.method.as_str(), asked.url.as_str()), ("POST", "/v1/rooms"));
    assert_eq!(asked.json(), json!({ "public": true, "stationId": "abcd1234", "hostName": "DJ Night" }));
    client.send("POST", "/api/sessions/rooms", Some(KEY), &json!({ "stationId": "abcd1234", "hostName": "   " }));
    assert_eq!(share.last().json(), json!({ "public": true, "stationId": "abcd1234" }));
    let no_key = client.send("POST", "/api/sessions/rooms", None, &json!({ "stationId": "abcd1234" }));
    assert_eq!(no_key.status, 401);

    // An answer that isn't JSON is not passed on.
    share.answer_as(Canned { status: 200, content_type: Some("text/html"), body: "<html>".into(), retry_after: None });
    let answer = client.get("/api/sessions/live");
    assert_eq!((answer.status, answer.json()), (502, json!({ "error": "bad_answer" })));
    share.answer_as(Canned { status: 200, content_type: Some("application/json"), body: "{not json".into(), retry_after: None });
    assert_eq!(client.get("/api/sessions/live").json(), json!({ "error": "bad_answer" }));
    share.answer_as(Canned { status: 200, content_type: Some("application/json"), body: "x".repeat(1_000_001), retry_after: None });
    assert_eq!(client.get("/api/sessions/live").json(), json!({ "error": "bad_answer" }));
    share.answer_as(Canned { status: 302, content_type: Some("application/json"), body: "{}".into(), retry_after: None });
    assert_eq!(client.get("/api/sessions/live").json(), json!({ "error": "bad_answer" }));

    // The window may ask first for its key header and the methods Sessions use.
    let own = client.base.clone();
    let asked_first = client.ask(
        "OPTIONS",
        "/api/sessions/stations/abcd1234",
        &[("Origin", own.as_str()), ("Access-Control-Request-Method", "PATCH")],
        None,
    );
    assert_eq!(asked_first.status, 204);
    assert!(asked_first.header("access-control-allow-headers").is_some_and(|h| h.contains("X-Stash-Community-Key")));
    assert!(asked_first.header("access-control-allow-methods").is_some_and(|m| m.contains("PATCH") && m.contains("DELETE")));

    // Offline, Sessions are off and nothing is asked of the network.
    let before = share.count();
    let offline = client.send("POST", "/api/offline", None, &json!({ "offline": true }));
    assert_eq!(offline.status, 200);
    assert_eq!(client.get("/api/sessions/status").json(), json!({ "on": false, "rooms": null, "offline": true }));
    let answer = client.get("/api/sessions/live");
    assert_eq!((answer.status, answer.json()), (503, json!({ "error": "offline" })));
    assert_eq!(share.count(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sessions_off_here_or_out_of_reach_say_so() {
    let dir = scratch("off");
    // Off: the status says so, and the routes answer as the share service does when it is off.
    let off = Client::new(&core(&dir.join("off.db"), None, "off"));
    assert_eq!(off.get("/api/sessions/status").json(), json!({ "on": false, "rooms": null, "offline": false }));
    let answer = off.get("/api/sessions/live");
    assert_eq!((answer.status, answer.json()), (503, json!({ "error": "sessions_off" })));

    // A share service that isn't there: 502 unreachable.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", closed.local_addr().unwrap().port());
    drop(closed);
    let away = Client::new(&core(&dir.join("away.db"), None, &address));
    let answer = away.get("/api/sessions/live");
    assert_eq!((answer.status, answer.json()), (502, json!({ "error": "unreachable" })));

    // An address that is neither https nor on this computer: off.
    let plain = Client::new(&core(&dir.join("plain.db"), None, "http://share.example"));
    assert_eq!(plain.get("/api/sessions/status").json()["on"], json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn this_computers_community_key_is_made_once_and_kept() {
    let dir = scratch("key");
    let db = dir.join("library.db");
    let client = Client::new(&core(&db, None, "off"));
    assert_eq!(client.get("/api/community").json(), json!({ "key": null }));
    // Several joins at once make one key.
    let joins: Vec<_> = (0..4)
        .map(|_| {
            let base = client.base.clone();
            std::thread::spawn(move || Client::new(&base).send("POST", "/api/community/join", None, &json!({})).json())
        })
        .collect();
    let keys: Vec<Value> = joins.into_iter().map(|j| j.join().unwrap()["key"].clone()).collect();
    let key = keys[0].as_str().unwrap().to_string();
    assert!(keys.iter().all(|k| k == &keys[0]), "{keys:?}");
    assert_eq!(key.len(), 43);
    assert!(key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{key}");
    // Joining again never replaces it, and it is there the next time Stash opens.
    assert_eq!(client.send("POST", "/api/community/join", None, &json!({})).json(), json!({ "key": key }));
    assert_eq!(client.get("/api/community").json(), json!({ "key": key }));
    let again = Client::new(&core(&db, None, "off"));
    assert_eq!(again.get("/api/community").json(), json!({ "key": key }));
    assert_eq!(client.get("/api/community/join").status, 405);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rooms_songs_play_here_from_their_place_and_autoplay_waits_for_the_rooms_next() {
    let dir = scratch("songs");
    let db = dir.join("library.db");
    let Some(engine) = engine() else {
        // Without a player the route says so; the body is checked by the unit tests.
        let client = Client::new(&core(&db, None, "off"));
        let answer = client.send("POST", "/api/player/songs", None, &json!({ "songs": [{ "title": "a", "artist": "b" }] }));
        assert_eq!(answer.status, 503);
        eprintln!("SKIPPED the room's songs on the player: no libmpv found");
        return;
    };
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["one", "two", "three", "four"] {
        silence_for(&music.join(format!("{name}.wav")), 4);
    }
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
    }
    let client = Client::new(&core(&db, Some(engine), "off"));
    let row = |name: &str| client.get(&format!("/api/search?q={name}&limit=1")).json()["rows"][0].clone();
    let song = |name: &str| {
        let row = row(name);
        json!({ "title": row["title"], "artist": row["artist"], "duration_ms": 4000 })
    };
    let player = || client.get("/api/player").json();
    let until = |what: &str, wanted: &dyn Fn(&Value) -> bool| -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = player();
            if wanted(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "waited for {what}: {state}");
            std::thread::sleep(Duration::from_millis(40));
        }
    };
    let play = |body: Value| {
        let answer = client.send("POST", "/api/player/songs", None, &body);
        assert_eq!(answer.status, 200, "{}", answer.text);
        answer.json()
    };

    // The room's song is the library's file, played from the room's place in it.
    let one = row("one");
    let queued = play(json!({ "songs": [song("one")], "position_ms": 1500, "from": "Night shift" }));
    let qid = queued["start_qid"].as_u64().unwrap();
    assert_eq!((queued["queued"].as_u64(), queued["qids"].clone()), (Some(1), json!([qid])));
    let state = until("the room's song playing", &|s| s["status"] == "playing" && s["duration_ms"].as_u64() > Some(0));
    assert_eq!((state["track"]["id"].clone(), state["track"]["qid"].as_u64()), (one["id"].clone(), Some(qid)));
    assert_eq!(state["from"], "Night shift");
    std::thread::sleep(Duration::from_millis(300));
    assert!(player()["position_ms"].as_u64() >= Some(1500), "the song plays from the room's place: {}", player());
    // Autoplay adds nothing after it: the room's next song comes instead.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(player()["queue_len"].as_u64(), Some(1), "autoplay added to the room's queue");

    // Paused (cued), it is opened and waits at its place; play goes on from there.
    let queued = play(json!({ "songs": [song("two")], "position_ms": 2000, "play": false }));
    let state = until("the room's song open, paused at its place", &|s| {
        s["status"] == "paused" && s["duration_ms"].as_u64() > Some(0)
    });
    assert_eq!(state["track"]["qid"].as_u64(), queued["start_qid"].as_u64());
    let at = state["position_ms"].as_u64().unwrap_or(0);
    assert!((1950..2100).contains(&at), "cued at {at} ms");
    client.send("POST", "/api/player/pause", None, &json!({ "paused": false }));
    until("the song playing on from its place", &|s| s["status"] == "playing" && s["position_ms"].as_u64() >= Some(1950));

    // Songs the library doesn't have go in the queue as they are described; `at` is where it starts.
    let queued = play(json!({
        "songs": [song("three"), { "title": "Not here", "artist": "Nobody", "youtube": "dQw4w9WgXcQ" }],
        "at": 0, "play": false
    }));
    assert_eq!(queued["queued"].as_u64(), Some(2));
    let rows = client.get("/api/player/queue?offset=0&limit=5").json()["rows"].clone();
    assert_eq!(rows[1]["title"], "Not here");
    assert_eq!(rows[1]["addon"]["track"], "dQw4w9WgXcQ");
    assert_eq!(
        rows.as_array().unwrap().iter().map(|r| r["qid"].clone()).collect::<Vec<_>>(),
        queued["qids"].as_array().unwrap().clone()
    );

    // With `autoplay: true` the queue goes on as any other does.
    play(json!({ "songs": [song("one")], "autoplay": true }));
    until("autoplay to add after the song", &|s| s["queue_len"].as_u64() > Some(1));

    // A session's queue the listener adds to is theirs: autoplay goes on after what they added.
    play(json!({ "songs": [song("two")], "position_ms": 3500 }));
    let added = client.send("POST", "/api/player/enqueue", None, &json!({ "items": [{ "id": row("three")["id"] }] }));
    assert_eq!(added.status, 200);
    assert_eq!(added.json()["qids"].as_array().map(Vec::len), Some(1), "{}", added.text);
    until("autoplay to add after the song the listener added", &|s| s["queue_len"].as_u64() > Some(2));

    // A listener's request the DJ took (this computer hosting): songs described, not entries, added after the one
    // playing; the library's when it has them, as described when not. The answer is the player's, and what was added.
    let queued = play(json!({ "songs": [song("one")], "play": false }));
    let requested = client.send(
        "POST",
        "/api/player/enqueue",
        None,
        &json!({ "songs": [song("four"), { "title": "Asked for", "artist": "A listener", "youtube": "dQw4w9WgXcQ" }], "next": true }),
    );
    assert_eq!(requested.status, 200, "{}", requested.text);
    let answer = requested.json();
    assert_eq!((answer["available"].as_bool(), answer["queue_len"].as_u64()), (Some(true), Some(3)), "{answer}");
    let qids = answer["qids"].as_array().cloned().unwrap_or_default();
    assert_eq!(qids.len(), 2, "{answer}");
    let rows = client.get("/api/player/queue?offset=0&limit=5").json()["rows"].clone();
    assert_eq!(rows[0]["qid"], queued["start_qid"]);
    assert_eq!((rows[1]["id"].clone(), rows[1]["qid"].clone()), (row("four")["id"].clone(), qids[0].clone()));
    assert_eq!((rows[2]["title"].as_str(), rows[2]["qid"].clone()), (Some("Asked for"), qids[1].clone()));
    assert_eq!(rows[2]["addon"]["track"], "dQw4w9WgXcQ");
    for wrong in [
        json!({ "songs": [] }),
        json!({ "songs": [{ "title": "No artist" }] }),
        json!({ "songs": [song("one")], "items": [{ "id": one["id"] }] }),
    ] {
        assert_eq!(client.send("POST", "/api/player/enqueue", None, &wrong).status, 400, "{wrong}");
    }
    assert_eq!(player()["queue_len"].as_u64(), Some(3), "a refused request adds nothing");

    // What isn't a body is refused.
    for wrong in [
        json!({}),
        json!({ "songs": [] }),
        json!({ "songs": [{ "title": "No artist" }] }),
        json!({ "songs": [song("one")], "at": 3 }),
        json!({ "songs": [song("one")], "position_ms": -1 }),
    ] {
        assert_eq!(client.send("POST", "/api/player/songs", None, &wrong).status, 400, "{wrong}");
    }
    let not_json = client.ask("POST", "/api/player/songs", &[("Content-Type", "application/json")], Some("{songs"));
    assert_eq!(not_json.status, 400);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tuned_in_the_dj_plays_and_leaving_gives_the_queue_back() {
    let dir = scratch("listening");
    let db = dir.join("library.db");
    let tuned_in = json!({ "error": { "message": "You are tuned in to a session: the DJ plays" } });
    let Some(engine) = engine() else {
        // Without a player the switch still answers; what it refuses needs the player.
        let client = Client::new(&core(&db, None, "off"));
        assert_eq!(client.send("POST", "/api/sessions/listening", None, &json!({ "on": true })).status, 204);
        assert_eq!(client.get("/api/sessions/listening").json(), json!({ "on": true }));
        eprintln!("SKIPPED what tuning in refuses: no libmpv found");
        return;
    };
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["one", "two", "three"] {
        silence_for(&music.join(format!("{name}.wav")), 4);
    }
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
    }
    let client = Client::new(&core(&db, Some(engine), "off"));
    let row = |name: &str| client.get(&format!("/api/search?q={name}&limit=1")).json()["rows"][0].clone();
    let song = |name: &str| {
        let row = row(name);
        json!({ "title": row["title"], "artist": row["artist"], "duration_ms": 4000 })
    };
    let player = || client.get("/api/player").json();
    let until = |what: &str, wanted: &dyn Fn(&Value) -> bool| -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = player();
            if wanted(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "waited for {what}: {state}");
            std::thread::sleep(Duration::from_millis(40));
        }
    };
    let post = |path: &str, body: Value| client.send("POST", path, None, &body);
    let listen = |on: bool| assert_eq!(post("/api/sessions/listening", json!({ "on": on })).status, 204);

    // Not tuned in until the window says so; a body that isn't a switch is refused.
    assert_eq!(client.get("/api/sessions/listening").json(), json!({ "on": false }));
    for wrong in [json!({}), json!({ "on": "yes" })] {
        assert_eq!(post("/api/sessions/listening", wrong.clone()).status, 400, "{wrong}");
    }

    // Tuned in, the room's songs play (and the room's next ones replace them).
    listen(true);
    assert_eq!(client.get("/api/sessions/listening").json(), json!({ "on": true }));
    let queued = post("/api/player/songs", json!({ "songs": [song("one"), song("two")] }));
    assert_eq!(queued.status, 200, "{}", queued.text);
    let start = queued.json()["start_qid"].clone();
    let state = until("the room's song playing", &|s| s["status"] == "playing" && s["duration_ms"].as_u64() > Some(0));
    assert_eq!((state["track"]["qid"].clone(), state["listening"].clone()), (start.clone(), json!(true)));

    // What the DJ chooses is refused, whoever asks: the window, the mini player, the tray and media keys (these
    // routes), a shortcut.
    let three = row("three")["id"].clone();
    for (path, body) in [
        ("/api/player/next", json!({})),
        ("/api/player/previous", json!({})),
        ("/api/player/jump", json!({ "index": 1 })),
        ("/api/player/play", json!({ "items": [{ "id": three }] })),
        ("/api/player/enqueue", json!({ "items": [{ "id": three }] })),
        ("/api/player/enqueue", json!({ "songs": [song("three")], "next": true })),
        ("/api/player/remove", json!({ "index": 1 })),
        ("/api/player/move", json!({ "from": 1, "to": 0 })),
        ("/api/player/shuffle", json!({ "on": true })),
        ("/api/player/repeat", json!({ "mode": "one" })),
        ("/api/player/speed", json!({ "speed": 1.25 })),
        ("/api/radio/start", json!({ "playing": true })),
        ("/api/shortcut", json!({ "action": "next" })),
    ] {
        let answer = post(path, body.clone());
        assert_eq!((answer.status, answer.json()), (409, tuned_in.clone()), "{path} {body}");
    }
    let state = player();
    assert_eq!(
        (state["track"]["qid"].clone(), state["queue_len"].as_u64(), state["shuffle"].clone(), state["repeat"].clone()),
        (start.clone(), Some(2), json!(false), json!("off")),
        "a refused control changed the player"
    );

    // The listener's own: pause and play, the place, the volume, and the settings put back as they are.
    assert_eq!(post("/api/player/pause", json!({ "paused": true })).status, 200);
    until("paused", &|s| s["status"] == "paused");
    assert_eq!(post("/api/player/seek", json!({ "seconds": 1.0 })).status, 200);
    assert_eq!(post("/api/player/volume", json!({ "percent": 60 })).status, 200);
    assert_eq!(post("/api/shortcut", json!({ "action": "toggle" })).status, 200);
    until("playing again", &|s| s["status"] == "playing");
    for (path, body) in [
        ("/api/player/shuffle", json!({ "on": false })),
        ("/api/player/repeat", json!({ "mode": "off" })),
        ("/api/player/speed", json!({ "speed": 1.0 })),
    ] {
        assert_eq!(post(path, body.clone()).status, 200, "{path} {body}");
    }

    // Autoplay adds nothing while tuned in, even to a queue the room did not hold.
    let last = post("/api/player/songs", json!({ "songs": [song("three")], "autoplay": true }));
    assert_eq!(last.status, 200, "{}", last.text);
    until("the room's last song playing", &|s| s["status"] == "playing" && s["duration_ms"].as_u64() > Some(0));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(player()["queue_len"].as_u64(), Some(1), "autoplay added while tuned in");

    // Leaving gives the queue back: autoplay goes on after the room's last song, and the controls work again.
    listen(false);
    until("autoplay to add after the room's last song", &|s| s["queue_len"].as_u64() > Some(1));
    assert_eq!(player()["listening"], json!(false));
    assert_eq!(post("/api/player/shuffle", json!({ "on": true })).status, 200);
    assert_eq!(post("/api/player/next", json!({})).status, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_share_service_that_hangs_holds_only_a_few_of_the_cores_calls() {
    let dir = scratch("busy");
    let share = Upstream::start();
    share.answer(200, json!({ "sessions": [] }));
    share.be_slow(Duration::from_millis(700));
    let client = Client::new(&core(&dir.join("library.db"), None, &share.url));

    // Six at once: four are passed on, the others are told to come back.
    let asks: Vec<_> = (0..6u64)
        .map(|i| {
            let base = client.base.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30 * i));
                let answer = Client::new(&base).get("/api/sessions/live");
                (answer.status, answer.json(), answer.header("Retry-After").map(str::to_string))
            })
        })
        .collect();
    let answers: Vec<_> = asks.into_iter().map(|a| a.join().unwrap()).collect();
    let busy = json!({ "error": "busy" });
    let told_to_wait = answers
        .iter()
        .filter(|a| a.0 == 503 && a.1 == busy && a.2.as_deref() == Some("1"))
        .count();
    let passed = answers.iter().filter(|a| a.0 == 200).count();
    assert_eq!((passed, told_to_wait), (4, 2), "{answers:?}");

    // Once they are answered, there is room again.
    share.be_slow(Duration::ZERO);
    assert_eq!(client.get("/api/sessions/live").status, 200);
    let _ = std::fs::remove_dir_all(&dir);
}
