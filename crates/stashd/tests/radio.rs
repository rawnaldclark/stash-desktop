//! Radio and autoplay that knows what is like what, from the outside: a stand-in for Stash's Last.fm proxy on this
//! computer answers from hand-written answers, the library has songs by some of the artists it names, and the demo
//! add-on has songs by others. No test asks Last.fm or the real proxy. (The few helpers shared with `api.rs` are
//! copied: each test file is a program of its own.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::Library;
use stash_core::audio::Output;
use stash_core::scan::scan_folder;
use stashd::demo_addon::DemoAddon;
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

/// A WAV of silence, 16-bit stereo at 44.1 kHz, tagged with its title and artist.
fn song(folder: &Path, title: &str, artist: &str) {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};
    let data: u32 = 44_100 * 4 * SECONDS;
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
    let path = folder.join(format!("{artist} - {title}.wav"));
    std::fs::write(&path, bytes).unwrap();
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.to_string());
    tag.set_artist(artist.to_string());
    tag.set_album(format!("{artist} songs"));
    tag.save_to_path(&path, WriteOptions::default()).unwrap();
}

/// Long enough that the song playing is still playing while a test looks.
const SECONDS: u32 = 20;

/// The library's songs, and the add-on's (written once for every test).
const LIBRARY: &[(&str, &[&str])] = &[
    (
        "Massive Attack",
        &[
            "Teardrop",
            "Angel",
            "Protection",
            "Karmacoma",
            "Unfinished Sympathy",
            "Safe from Harm",
            "Risingson",
        ],
    ),
    (
        "Portishead",
        &[
            "Glory Box",
            "Roads",
            "Sour Times",
            "Wandering Star",
            "Mysterons",
        ],
    ),
    ("Tricky", &["Overcome", "Aftermath", "Black Steel"]),
    ("Unknown artist", &["AUD-20231004-WA0012"]),
];
const ADDON: &[(&str, &[&str])] = &[
    (
        "Zero 7",
        &[
            "In the Waiting Line",
            "Destiny",
            "Polaris",
            "Destiny (Live)",
        ],
    ),
    ("Morcheeba", &["The Sea", "Rome Wasn't Built in a Day"]),
];

fn folders() -> &'static (PathBuf, PathBuf) {
    static FOLDERS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    FOLDERS.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("stash-radio-music-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (music, addon) = (root.join("Music"), root.join("Addon"));
        for (folder, songs) in [(&music, LIBRARY), (&addon, ADDON)] {
            std::fs::create_dir_all(folder).unwrap();
            for (artist, titles) in songs {
                for title in *titles {
                    song(folder, title, artist);
                }
            }
        }
        (music, addon)
    })
}

/// How the stand-in answers.
#[derive(Clone, Copy, PartialEq)]
enum Mood {
    Answering,
    /// 429, asking to wait this long.
    Busy(u32),
    /// 502, as the proxy answers when Last.fm is down.
    Away,
    /// Last.fm's "not found" (error 6).
    Unknown,
    /// Answering, a while later.
    Slow(Duration),
}

/// One request the stand-in had.
#[derive(Clone, Debug)]
struct Got {
    query: HashMap<String, String>,
    agent: String,
    version: String,
}

/// Stash's Last.fm proxy, standing in: hand-written answers about the test's artists.
struct LastFm {
    url: String,
    port: u16,
    mood: Arc<Mutex<Mood>>,
    got: Arc<Mutex<Vec<Got>>>,
}

fn similar_tracks() -> Value {
    let track = |name: &str, artist: &str, score: f64| json!({ "name": name, "match": score, "artist": { "name": artist } });
    json!({ "similartracks": { "track": [
        track("Glory Box", "Portishead", 1.0),
        track("Overcome", "Tricky", 0.9),
        track("In the Waiting Line", "Zero 7", 0.85),
        track("Angel", "Massive Attack", 0.8),
        track("The Sea", "Morcheeba", 0.7),
        track("Roads", "Portishead", 0.6),
    ], "@attr": { "artist": "Massive Attack" } } })
}

fn similar_artists() -> Value {
    let artist = |name: &str, score: &str| json!({ "name": name, "match": score });
    json!({ "similarartists": { "artist": [
        artist("Portishead", "1"),
        artist("Tricky", "0.9"),
        artist("Zero 7", "0.8"),
        artist("Morcheeba", "0.7"),
        artist("Lamb", "0.6"),
        artist("Hooverphonic", "0.5"),
        artist("Sneaker Pimps", "0.4"),
    ] } })
}

impl LastFm {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let mood = Arc::new(Mutex::new(Mood::Answering));
        let got = Arc::new(Mutex::new(Vec::new()));
        let (moods, kept) = (Arc::clone(&mood), Arc::clone(&got));
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let header = |name: &'static str| {
                    request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv(name))
                        .map(|h| h.value.as_str().to_string())
                        .unwrap_or_default()
                };
                let url = url::Url::parse(&format!("http://local{}", request.url())).unwrap();
                let query: HashMap<String, String> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                // Refused as the proxy refuses.
                let refused = if *request.method() != tiny_http::Method::Get {
                    Some(405)
                } else if url.path() != "/lastfm" {
                    Some(404)
                } else if ["api_key", "user", "username", "sk", "api_sig"]
                    .iter()
                    .any(|k| query.contains_key(*k))
                {
                    Some(400)
                } else {
                    None
                };
                kept.lock().unwrap().push(Got {
                    query: query.clone(),
                    agent: header("User-Agent"),
                    version: header("X-Stash-Version"),
                });
                let mood = *moods.lock().unwrap();
                if let Mood::Slow(wait) = mood {
                    std::thread::sleep(wait);
                }
                let get = |k: &str| query.get(k).map(|v| v.to_lowercase()).unwrap_or_default();
                let known = match get("method").as_str() {
                    "track.getsimilar"
                        if get("artist") == "massive attack" && get("track") == "teardrop" =>
                    {
                        Some(similar_tracks())
                    }
                    "artist.getsimilar" if get("artist") == "massive attack" => {
                        Some(similar_artists())
                    }
                    _ => None,
                };
                let (status, body, wait) = match (refused, mood) {
                    (Some(status), _) => (status, json!({ "error": "refused" }), None),
                    (None, Mood::Busy(wait)) => (429, json!({ "error": "busy" }), Some(wait)),
                    (None, Mood::Away) => (502, json!({ "error": "upstream_error" }), None),
                    (None, Mood::Unknown) => (
                        200,
                        json!({ "error": 6, "message": "Track not found" }),
                        None,
                    ),
                    (None, _) => match known {
                        Some(body) => (200, body, None),
                        None => (
                            200,
                            json!({ "error": 6, "message": "Track not found" }),
                            None,
                        ),
                    },
                };
                let mut response =
                    tiny_http::Response::from_string(body.to_string()).with_status_code(status);
                if let Some(wait) = wait {
                    response.add_header(
                        tiny_http::Header::from_bytes("Retry-After", wait.to_string()).unwrap(),
                    );
                }
                let _ = request.respond(response);
            }
        });
        LastFm {
            url: format!("http://127.0.0.1:{port}/lastfm"),
            port,
            mood,
            got,
        }
    }

    fn be(&self, mood: Mood) {
        *self.mood.lock().unwrap() = mood;
    }

    fn asked(&self) -> Vec<Got> {
        self.got.lock().unwrap().clone()
    }

    /// How many times it was asked with `method`.
    fn count(&self, method: &str) -> usize {
        self.asked()
            .iter()
            .filter(|g| g.query.get("method").map(String::as_str) == Some(method))
            .count()
    }

    /// Fails when `text` carries the stand-in's address in any way.
    fn not_in(&self, text: &str) {
        assert!(
            !text.contains(&self.port.to_string()) && !text.contains("/lastfm"),
            "the proxy's address is in: {text}"
        );
    }
}

struct Client {
    agent: ureq::Agent,
    base: String,
}

impl Client {
    fn get(&self, path: &str) -> (u16, Value) {
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.base))
            .call()
            .unwrap();
        (
            response.status().as_u16(),
            response.body_mut().read_json().unwrap_or(Value::Null),
        )
    }

    fn send(&self, path: &str, body: Value) -> (u16, Value) {
        let mut response = self
            .agent
            .post(&format!("{}{path}", self.base))
            .send_json(body)
            .unwrap();
        (
            response.status().as_u16(),
            response.body_mut().read_json().unwrap_or(Value::Null),
        )
    }

    fn post(&self, path: &str, body: Value) -> Value {
        let (status, answer) = self.send(path, body);
        assert_eq!(status, 200, "{path}: {answer}");
        answer
    }

    fn player(&self) -> Value {
        self.get("/api/player").1
    }

    /// The queue's rows, every one.
    fn queue(&self) -> Vec<Value> {
        self.get("/api/player/queue?offset=0&limit=500").1["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// Asks for the player's state until it satisfies `wanted`.
    fn player_until(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = self.player();
            if wanted(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "waited 15 s for: {what}; the player says {state}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Waits until the station of the queue has added its songs.
    fn station_settled(&self, what: &str) -> Value {
        self.player_until(what, |s| {
            s["station"].is_object() && s["station"]["finding"] == false
        })
    }
}

/// "artist / title" of each row.
fn named(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            format!(
                "{} / {}",
                r["artist"].as_str().unwrap_or_default(),
                r["title"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// No two rows side by side by the same artist.
fn spread(rows: &[Value]) {
    for pair in rows.windows(2) {
        assert_ne!(
            pair[0]["artist"].as_str().map(str::to_lowercase),
            pair[1]["artist"].as_str().map(str::to_lowercase),
            "the same artist twice in a row: {:?}",
            named(rows)
        );
    }
}

/// A core of its own on a library of the test songs, with the demo add-on added, and radio when `radio`.
struct Rig {
    dir: PathBuf,
    db: PathBuf,
    client: Client,
    lastfm: LastFm,
    addon: Arc<Mutex<Vec<String>>>,
    ids: HashMap<String, i64>,
}

impl Rig {
    fn new(name: &str, radio: bool) -> Option<Self> {
        let engine = engine()?;
        let (music, addon_folder) = folders();
        let dir = std::env::temp_dir().join(format!("stash-radio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.db");
        let ids = {
            let mut library = Library::open(&db).unwrap();
            scan_folder(&mut library, music, |_| {}).unwrap();
            library
                .tracks_at(0, 100)
                .unwrap()
                .into_iter()
                .map(|t| (t.title, t.id))
                .collect()
        };
        let lastfm = LastFm::start();
        let demo = DemoAddon::bind(addon_folder, 0, Duration::ZERO).unwrap();
        let (link, addon) = (demo.link(), demo.asked());
        std::thread::spawn(move || demo.run());
        let client = Rig::open(&db, radio.then_some(lastfm.url.as_str()), &engine);
        client.post("/api/addons", json!({ "link": link }));
        addon.lock().unwrap().clear();
        Some(Rig {
            dir,
            db,
            client,
            lastfm,
            addon,
            ids,
        })
    }

    /// A core on `db`, asking the stand-in at `proxy`.
    fn open(db: &Path, proxy: Option<&str>, engine: &Path) -> Client {
        let core = Core::bind(Options {
            db: db.to_path_buf(),
            ui: None,
            port: 0,
            bench_out: None,
            audio: Some(Output::Silent),
            mpv: Some(engine.to_path_buf()),
            play_counts_between_seconds: None,
        })
        .unwrap();
        let core = match proxy {
            Some(proxy) => core.with_similar(proxy),
            None => core,
        };
        let base = format!("http://127.0.0.1:{}", core.port());
        std::thread::spawn(move || core.run());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Client { agent, base }
    }

    fn id(&self, title: &str) -> i64 {
        self.ids[title]
    }

    /// The add-on's searches, by their words.
    fn searches(&self) -> Vec<String> {
        self.addon
            .lock()
            .unwrap()
            .iter()
            .filter_map(|asked| asked.strip_prefix("/search?q=").map(str::to_string))
            .collect()
    }

    /// Fails when any answer the window reads carries the proxy's address.
    fn nothing_tells_the_address(&self) {
        for path in [
            "/api/player",
            "/api/player/queue?offset=0&limit=500",
            "/api/settings/similar",
            "/api/settings/autoplay",
        ] {
            self.lastfm.not_in(&self.client.get(path).1.to_string());
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.client.send("/api/player/stop", json!({}));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

macro_rules! rig {
    ($name:expr, $radio:expr) => {
        match Rig::new($name, $radio) {
            Some(rig) => rig,
            None => {
                eprintln!("SKIPPED {}: no libmpv found", $name);
                return;
            }
        }
    };
}

#[test]
fn a_song_radio_plays_similar_music_from_the_library_then_add_ons_and_grows() {
    let rig = rig!("song", true);
    let client = &rig.client;
    assert_eq!(
        client.get("/api/settings/similar").1,
        json!({ "available": true, "on": true })
    );

    // The seed plays at once; the station's songs follow it.
    let started = client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    assert_eq!(started["station"]["kind"], "radio", "{started}");
    assert_eq!(started["station"]["label"], "Teardrop radio", "{started}");
    rig.lastfm.not_in(&started.to_string());
    let now = client.station_settled("the radio's first songs");
    assert_eq!(
        (&now["track"]["title"], &now["from"]),
        (&json!("Teardrop"), &json!("Teardrop radio")),
        "{now}"
    );
    assert_eq!(now["station"]["via"], "lastfm", "{now}");
    let rows = client.queue();
    let names = named(&rows);
    assert!(rows.len() >= 8, "{names:?}");
    spread(&rows);
    // The library's own songs by the artists Last.fm named, as library rows; the add-on's for those it has none of.
    let by = |artist: &str| {
        rows.iter()
            .filter(|r| r["artist"] == artist)
            .collect::<Vec<_>>()
    };
    assert!(
        !by("Portishead").is_empty() && by("Portishead").iter().all(|r| r["id"].is_i64()),
        "{names:?}"
    );
    assert!(
        !by("Zero 7").is_empty() && by("Zero 7").iter().all(|r| r["addon"].is_object()),
        "{names:?}"
    );
    // The seed itself never comes again, nor another version Last.fm did not name.
    assert_eq!(
        names.iter().filter(|n| n.ends_with("/ Teardrop")).count(),
        1,
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.contains("(Live)")), "{names:?}");
    // Last.fm's titles come first for their artist: Glory Box is Portishead's first.
    assert_eq!(by("Portishead")[0]["title"], "Glory Box", "{names:?}");
    // The seed's artist has at most 3 of the first 10.
    let seeds = rows[1..rows.len().min(11)]
        .iter()
        .filter(|r| r["artist"] == "Massive Attack")
        .count();
    assert!(seeds <= 3, "{names:?}");

    // One question, as the phones ask it, with Stash's own headers and no key.
    let asked = rig.lastfm.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    let q = &asked[0].query;
    assert_eq!(
        q.get("method").map(String::as_str),
        Some("track.getsimilar")
    );
    assert_eq!(
        (
            q.get("artist").map(String::as_str),
            q.get("track").map(String::as_str)
        ),
        (Some("Massive Attack"), Some("Teardrop"))
    );
    assert_eq!(
        (
            q.get("autocorrect").map(String::as_str),
            q.get("limit").map(String::as_str)
        ),
        (Some("1"), Some("30"))
    );
    assert!(
        !q.contains_key("api_key") && !q.contains_key("format"),
        "{q:?}"
    );
    assert!(
        asked[0].agent.starts_with("Stash-desktop/"),
        "{:?}",
        asked[0].agent
    );
    assert!(
        asked[0].version.starts_with("desktop/"),
        "{:?}",
        asked[0].version
    );
    // One search for each artist the library had too few of.
    let searches = rig.searches();
    let mut once = searches.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), searches.len(), "{searches:?}");
    assert!(searches.contains(&"Zero 7".to_string()), "{searches:?}");

    // It grows as it plays: three from the end, a batch more. Last.fm is asked twice at most in its life, and an
    // add-on once at most for each artist.
    let first = rows.len() as u64;
    for _ in 0..3 {
        let before = client.player()["queue_len"].as_u64().unwrap();
        client.post("/api/player/jump", json!({ "index": before - 3 }));
        // A station with nothing more like it stops growing: the queue then ends as any queue does.
        let deadline = Instant::now() + Duration::from_secs(6);
        while client.player()["queue_len"].as_u64() == Some(before) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    assert!(
        client.player()["queue_len"].as_u64() > Some(first),
        "{:?}",
        named(&client.queue())
    );
    let asked = rig.lastfm.asked();
    assert!(asked.len() <= 2, "{asked:?}");
    let searches = rig.searches();
    let mut once = searches.clone();
    once.sort();
    once.dedup();
    assert_eq!(once.len(), searches.len(), "{searches:?}");
    spread(&client.queue());
    rig.nothing_tells_the_address();
}

#[test]
fn the_same_seed_is_asked_once_and_stop_takes_out_the_stations_songs() {
    let rig = rig!("again", true);
    let client = &rig.client;
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs");
    assert_eq!(rig.lastfm.asked().len(), 1);
    // Each question once at most, however often the seed is started (a later station leaves out what earlier ones
    // added, so it may need the second question).
    let once = |rig: &Rig| {
        assert_eq!(
            rig.lastfm.count("track.getsimilar"),
            1,
            "{:?}",
            rig.lastfm.asked()
        );
        assert!(
            rig.lastfm.count("artist.getsimilar") <= 1,
            "{:?}",
            rig.lastfm.asked()
        );
    };

    // A song added by hand stays; Stop takes the station's songs out of what comes next.
    client.post(
        "/api/player/enqueue",
        json!({ "items": [{ "id": rig.id("Mysterons") }] }),
    );
    let stopped = client.post("/api/radio/stop", json!({}));
    assert!(stopped["removed"].as_u64() > Some(0), "{stopped}");
    let rows = client.queue();
    assert_eq!(
        named(&rows[1..]),
        ["Portishead / Mysterons"],
        "{:?}",
        named(&rows)
    );
    assert!(client.player()["station"].is_null());
    assert_eq!(client.post("/api/radio/stop", json!({}))["removed"], 0);

    // Started again: what Last.fm said is kept, so it is not asked.
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs again");
    once(&rig);

    // "Radio from the song playing": it plays on, and what follows is the station's.
    let playing = client.player();
    let qid = playing["track"]["qid"].clone();
    let started = client.post("/api/radio/start", json!({ "playing": true }));
    assert_eq!(started["station"]["kind"], "radio", "{started}");
    let now = client.station_settled("the radio from the song playing");
    assert_eq!(
        (&now["track"]["qid"], &now["index"]),
        (&qid, &json!(0)),
        "{now}"
    );
    assert!(
        now["queue_id"].as_u64() > playing["queue_id"].as_u64(),
        "{now}"
    );
    assert_eq!(now["from"], "Teardrop radio");
    once(&rig);

    // Another run on the same library asks nothing either.
    client.post("/api/player/stop", json!({}));
    let second = Rig::open(&rig.db, Some(&rig.lastfm.url), &engine().unwrap());
    second.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    second.station_settled("the radio in another run");
    once(&rig);
    second.post("/api/player/stop", json!({}));
    rig.nothing_tells_the_address();
}

#[test]
fn songs_and_artists_chosen_not_to_play_never_come() {
    let rig = rig!("blocked", true);
    let client = &rig.client;
    client.post(
        "/api/blocks",
        json!({ "kind": "artist", "artist": "Zero 7", "blocked": true }),
    );
    client.post(
        "/api/blocks",
        json!({ "kind": "song", "title": "Glory Box", "artist": "Portishead", "blocked": true }),
    );
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs");
    let names = named(&client.queue());
    assert!(
        !names
            .iter()
            .any(|n| n.starts_with("Zero 7") || n.ends_with("Glory Box")),
        "{names:?}"
    );
    assert!(
        !rig.searches().contains(&"Zero 7".to_string()),
        "{:?}",
        rig.searches()
    );
    let (status, refused) = client.send("/api/radio/start", json!({ "artist": "Zero 7" }));
    assert_eq!(
        (status, &refused["error"]["message"]),
        (409, &json!("You chose not to play Zero 7")),
        "{refused}"
    );
    // A song chosen not to be played still plays when it is asked for by itself.
    client.post(
        "/api/blocks",
        json!({ "kind": "song", "title": "Angel", "artist": "Massive Attack", "blocked": true }),
    );
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Angel") } }),
    );
    let now = client.player_until("Angel playing", |s| s["track"]["title"] == "Angel");
    assert_eq!(now["from"], "Angel radio");
}

#[test]
fn offline_asks_no_one_and_plays_only_what_is_here() {
    let rig = rig!("offline", true);
    let client = &rig.client;
    // Asked once while online, and kept.
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs");
    client.post("/api/radio/stop", json!({}));
    let (asked, searched) = (rig.lastfm.asked().len(), rig.addon.lock().unwrap().len());

    client.post("/api/offline", json!({ "offline": true }));
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    let now = client.station_settled("the radio offline");
    assert_eq!(now["station"]["via"], "lastfm", "{now}");
    let rows = client.queue();
    assert!(
        rows.len() > 1 && rows.iter().all(|r| r["here"] == true),
        "{:?}",
        named(&rows)
    );
    assert_eq!(
        (rig.lastfm.asked().len(), rig.addon.lock().unwrap().len()),
        (asked, searched)
    );

    // With nothing kept for the seed: the library picks.
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Roads") } }),
    );
    let now = client.station_settled("the library's picks offline");
    assert_eq!(
        (&now["station"]["via"], &now["station"]["why"]),
        (&json!("library"), &json!("Offline")),
        "{now}"
    );
    assert_eq!(rig.lastfm.asked().len(), asked);
    client.post("/api/offline", json!({ "offline": false }));
}

#[test]
fn autoplay_goes_on_with_similar_music_when_the_queue_ends() {
    let rig = rig!("autoplay", true);
    let client = &rig.client;
    // One song queued: once it starts, a station from it follows.
    client.post("/api/player/play", json!({ "ids": [rig.id("Teardrop")] }));
    let now = client.station_settled("autoplay's songs");
    assert_eq!(
        (&now["station"]["kind"], &now["station"]["via"]),
        (&json!("autoplay"), &json!("lastfm")),
        "{now}"
    );
    assert_eq!(
        (&now["track"]["title"], &now["index"]),
        (&json!("Teardrop"), &json!(0)),
        "{now}"
    );
    let rows = client.queue();
    assert!(rows.len() > 1, "{:?}", named(&rows));
    spread(&rows);
    assert_eq!(rig.lastfm.asked().len(), 1);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(rig.lastfm.asked().len(), 1, "autoplay acts once a song");

    // A last song Last.fm cannot be asked about: the library picks, and no one is asked.
    client.post(
        "/api/player/play",
        json!({ "ids": [rig.id("AUD-20231004-WA0012")] }),
    );
    let now = client.player_until("autoplay from the library", |s| {
        s["queue_len"].as_u64() > Some(1)
    });
    assert_eq!(
        (&now["station"]["kind"], &now["station"]["via"]),
        (&json!("autoplay"), &json!("library")),
        "{now}"
    );
    assert_eq!(rig.lastfm.asked().len(), 1);
    rig.nothing_tells_the_address();
}

#[test]
fn last_fm_busy_away_or_asked_enough_the_music_still_goes_on() {
    let rig = rig!("busy", true);
    let client = &rig.client;
    rig.lastfm.be(Mood::Busy(60));
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    let now = client.station_settled("the library's picks");
    assert_eq!(
        (&now["station"]["via"], &now["station"]["why"]),
        (&json!("library"), &json!("Last.fm could not be reached")),
        "{now}"
    );
    assert!(client.queue().len() > 1);
    // Left alone as it asked: the next station asks nothing.
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Roads") } }),
    );
    client.station_settled("the library's picks again");
    assert_eq!(rig.lastfm.asked().len(), 1);
    rig.nothing_tells_the_address();
    drop(rig);

    let rig = rig!("away", true);
    let client = &rig.client;
    rig.lastfm.be(Mood::Away);
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    let now = client.station_settled("the library's picks");
    assert_eq!(now["station"]["via"], "library", "{now}");
    rig.lastfm.not_in(&now.to_string());
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Roads") } }),
    );
    client.station_settled("the library's picks again");
    assert_eq!(rig.lastfm.asked().len(), 1);
    drop(rig);

    // Last.fm knows nothing like it: that is kept too, and not asked again.
    let rig = rig!("unknown", true);
    let client = &rig.client;
    rig.lastfm.be(Mood::Unknown);
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Roads") } }),
    );
    let now = client.station_settled("the library's picks");
    assert_eq!(
        (&now["station"]["via"], &now["station"]["why"]),
        (&json!("library"), &json!("Last.fm found nothing like it")),
        "{now}"
    );
    // Its song question named no one, so the artist was asked about too.
    let asked = rig.lastfm.asked().len();
    assert!((1..=2).contains(&asked), "{asked}");
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Roads") } }),
    );
    client.station_settled("the library's picks again");
    assert_eq!(rig.lastfm.asked().len(), asked);
    drop(rig);

    // A hundred questions today already: none more until tomorrow, and the music goes on.
    let rig = rig!("cap", true);
    {
        let mut library = Library::open(&rig.db).unwrap();
        let today = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            / 86_400_000;
        library
            .set_setting("similar.day", &format!("{today}:100"))
            .unwrap();
    }
    let client = &rig.client;
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    let now = client.station_settled("the library's picks");
    assert_eq!(
        (&now["station"]["via"], &now["station"]["why"]),
        (
            &json!("library"),
            &json!("Last.fm was asked enough for today")
        ),
        "{now}"
    );
    assert!(rig.lastfm.asked().is_empty());
}

#[test]
fn a_new_queue_ends_the_station() {
    let rig = rig!("new-queue", true);
    let client = &rig.client;
    client.post("/api/settings/autoplay", json!({ "on": false }));
    // Last.fm answers slowly: the listener plays something else meanwhile.
    rig.lastfm.be(Mood::Slow(Duration::from_millis(1_500)));
    let started = client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    assert_eq!(started["station"]["finding"], true, "{started}");
    let before = client.player()["queue_id"].as_u64().unwrap();
    client.post("/api/player/play", json!({ "ids": [rig.id("Sour Times")] }));
    let now = client.player_until("the new queue", |s| s["track"]["title"] == "Sour Times");
    assert!(now["queue_id"].as_u64() > Some(before), "{now}");
    std::thread::sleep(Duration::from_millis(2_500));
    let now = client.player();
    assert!(now["station"].is_null(), "{now}");
    assert_eq!(named(&client.queue()), ["Portishead / Sour Times"]);
    assert_eq!(rig.lastfm.asked().len(), 1);
}

#[test]
fn switching_similar_music_off_forgets_what_was_kept() {
    let rig = rig!("off", true);
    let client = &rig.client;
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs");
    let off = client.post("/api/settings/similar", json!({ "on": false }));
    assert_eq!(off["on"], false, "{off}");
    assert!(off["forgot"].as_u64() >= Some(1), "{off}");
    // The station ends; its songs stay.
    let now = client.player();
    assert!(
        now["station"].is_null() && now["queue_len"].as_u64() > Some(1),
        "{now}"
    );
    let (status, refused) = client.send(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    assert_eq!(
        (status, &refused["error"]["message"]),
        (
            409,
            &json!("Similar music from Last.fm is switched off (Settings › Sound)")
        ),
        "{refused}"
    );
    // On again: what was kept was forgotten, so it is asked again.
    client.post("/api/settings/similar", json!({ "on": true }));
    client.post(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    client.station_settled("the radio's songs again");
    assert_eq!(rig.lastfm.count("track.getsimilar"), 2);
}

#[test]
fn a_build_without_the_proxy_has_no_radio() {
    let rig = rig!("none", false);
    let client = &rig.client;
    assert_eq!(
        client.get("/api/settings/similar").1,
        json!({ "available": false, "on": false })
    );
    let (status, refused) = client.send(
        "/api/radio/start",
        json!({ "song": { "id": rig.id("Teardrop") } }),
    );
    assert_eq!(
        (status, &refused["error"]["message"]),
        (404, &json!("Radio is not in this build of Stash")),
        "{refused}"
    );
    let (status, _) = client.send("/api/settings/similar", json!({ "on": true }));
    assert_eq!(status, 404);
    // Autoplay picks from the library, as it always did.
    client.post("/api/player/play", json!({ "ids": [rig.id("Teardrop")] }));
    let now = client.player_until("autoplay from the library", |s| {
        s["queue_len"].as_u64() > Some(1)
    });
    assert_eq!(
        (&now["station"]["kind"], &now["station"]["via"]),
        (&json!("autoplay"), &json!("library")),
        "{now}"
    );
    assert!(rig.lastfm.asked().is_empty());
}

#[test]
fn an_artist_radio_starts_with_their_song() {
    let rig = rig!("artist", true);
    let client = &rig.client;
    let started = client.post("/api/radio/start", json!({ "artist": "Massive Attack" }));
    assert_eq!(
        started["station"]["label"], "Massive Attack radio",
        "{started}"
    );
    let now = client.station_settled("the artist radio's songs");
    assert_eq!(now["track"]["artist"], "Massive Attack", "{now}");
    let asked = rig.lastfm.asked();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(
        asked[0].query.get("method").map(String::as_str),
        Some("artist.getsimilar")
    );
    assert_eq!(asked[0].query.get("limit").map(String::as_str), Some("100"));
    spread(&client.queue());
    // An artist with nothing here that plays, nor on the add-on.
    let (status, refused) = client.send("/api/radio/start", json!({ "artist": "Nobody At All" }));
    assert_eq!(
        (status, &refused["error"]["message"]),
        (409, &json!("Nothing by Nobody At All can play here")),
        "{refused}"
    );
}
