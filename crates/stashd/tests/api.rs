//! The local API from the outside: a folder is scanned into a library, the core serves it, and a client reads the
//! library and drives the player over HTTP, as the window does; then an add-on is added from its link, searched and
//! played from. The player parts need libmpv; without it the test checks that the API says so.

use std::path::{Path, PathBuf};
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

/// Two seconds of stereo silence, 16-bit 44.1 kHz, with no tags: the scan names it after the file.
fn silence(path: &Path) {
    let data: u32 = 44_100 * 2 * 4;
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

    fn post(&self, path: &str, body: Value) -> Value {
        let mut response = self
            .agent
            .post(&format!("{}{path}", self.base))
            .send_json(body)
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "{path}");
        response.body_mut().read_json().unwrap()
    }

    /// A POST whatever its status.
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

    /// Asks for the player's state until it satisfies `wanted`.
    fn player_until(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let (_, state) = self.get("/api/player");
            if wanted(&state) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "waited 8 s for: {what}; the player says {state}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn a_client_reads_the_library_and_drives_the_player_over_http() {
    let dir = std::env::temp_dir().join(format!("stash-api-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
        silence(&music.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        assert_eq!(scan_folder(&mut library, &music, |_| {}).unwrap().added, 5);
    }

    let engine = engine();
    let core = Core::bind(Options {
        db,
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
    })
    .unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let client = Client { agent, base };

    // The library.
    assert_eq!(
        client.get("/api/counts"),
        (200, json!({ "tracks": 5, "albums": 0, "artists": 1 }))
    );
    let (_, page) = client.get("/api/tracks?offset=1&limit=2");
    let titles: Vec<&str> = page["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["bravo", "charlie"]);
    // A page from somewhere else may not read it.
    let mut foreign = client
        .agent
        .get(&format!("{}/api/counts", client.base))
        .header("Origin", "https://evil.example")
        .call()
        .unwrap();
    assert_eq!(foreign.status().as_u16(), 403);
    let _ = foreign.body_mut().read_to_string();

    let (_, idle) = client.get("/api/player");
    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        assert_eq!(
            (idle["available"].as_bool(), idle["status"].as_str()),
            (Some(false), Some("idle"))
        );
        assert!(
            idle["reason"]
                .as_str()
                .is_some_and(|r| r.contains("not found"))
        );
        return;
    }
    assert_eq!(
        (idle["available"].as_bool(), idle["status"].as_str()),
        (Some(true), Some("idle"))
    );

    // Starting the row at offset 1 plays "bravo" and queues the rows after it.
    client.post("/api/player/play", json!({ "offset": 1 }));
    let playing = client.player_until("bravo playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "bravo" && s["duration_ms"] == 2000
    });
    assert_eq!(
        (playing["index"].as_u64(), playing["queue_len"].as_u64()),
        (Some(0), Some(4))
    );
    assert!(
        playing["track"].get("locator").is_none(),
        "the window is not told where files are"
    );
    let signal = client.player_until("the signal path", |s| {
        s["signal"]["output"]["sample_rate"].is_number()
    })["signal"]
        .clone();
    assert_eq!(
        (
            signal["decoded"]["sample_rate"].as_u64(),
            signal["resampled"].as_bool(),
            signal["driver"].as_str()
        ),
        (Some(44_100), Some(false), Some("null"))
    );

    client.post("/api/player/pause", json!({ "paused": true }));
    client.player_until("paused", |s| s["status"] == "paused");
    client.post("/api/player/seek", json!({ "seconds": 1.5 }));
    // The position is what has left the output's buffer, so right after a seek it reads a little short of 1.5 s.
    client.player_until("the seek", |s| {
        s["position_ms"]
            .as_u64()
            .is_some_and(|p| (1250..=1600).contains(&p))
    });
    client.post("/api/player/next", json!({}));
    client.player_until("charlie", |s| {
        s["track"]["title"] == "charlie" && s["index"] == 1
    });
    client.post("/api/player/previous", json!({}));
    client.player_until("bravo again", |s| s["track"]["title"] == "bravo");

    // Chosen tracks play in the order asked; an id the library does not have is left out.
    let (_, all) = client.get("/api/tracks?offset=0&limit=5");
    let id = |title: &str| {
        all["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["title"] == title)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    client.post(
        "/api/player/play",
        json!({ "ids": [id("echo"), 999_999, id("alpha")] }),
    );
    let chosen = client.player_until("echo playing", |s| {
        s["track"]["title"] == "echo" && s["status"] == "playing"
    });
    assert_eq!(chosen["queue_len"].as_u64(), Some(2));

    // ---- An add-on: the same folder, served by the demo add-on, added by its link.
    let (_, none) = client.get("/api/addons");
    assert_eq!(none, json!({ "addons": [] }));
    let demo = DemoAddon::bind(&music, 0, Duration::from_millis(120)).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());

    // A link that is not an add-on is refused with the reason, and nothing is kept.
    let (status, refused) = client.send(
        "/api/addons",
        json!({ "link": "https://example.invalid/not-a-manifest" }),
    );
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (400, Some("bad_request"))
    );
    let (status, refused) = client.send(
        "/api/addons",
        json!({ "link": "ftp://127.0.0.1/manifest.json" }),
    );
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (400, Some("invalid_response"))
    );
    assert_eq!(client.get("/api/addons").1, json!({ "addons": [] }));

    let added = client.post("/api/addons", json!({ "link": link }));
    let listed = &added["addons"][0];
    assert_eq!(
        (
            listed["id"].as_str(),
            listed["name"].as_str(),
            listed["host"].as_str(),
            listed["enabled"].as_bool(),
            listed["problem"].is_null()
        ),
        (
            Some("demo.folder"),
            Some("Demo folder"),
            Some("127.0.0.1"),
            Some(true),
            true
        )
    );
    assert!(
        listed.get("link").is_none(),
        "the window is not given the link: it can carry a key"
    );
    // Adding it again does not make a second one.
    assert_eq!(
        client.post("/api/addons", json!({ "link": link }))["addons"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Searching the add-ons: one group for each, in the listener's order.
    let (_, found) = client.get("/api/addons/search?q=char");
    let group = &found["groups"][0];
    assert_eq!(
        (
            group["addon"]["id"].as_str(),
            group["addon"]["name"].as_str(),
            group["error"].is_null()
        ),
        (Some("demo.folder"), Some("Demo folder"), true)
    );
    let tracks = group["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(
        (
            tracks[0]["title"].as_str(),
            tracks[0]["max_tier"].as_str(),
            tracks[0]["duration_ms"].as_u64()
        ),
        (Some("charlie"), Some("lossless"), Some(2000))
    );
    assert_eq!(
        client.get("/api/addons/search?q=").1,
        json!({ "groups": [] })
    );
    assert_eq!(
        client.get("/api/addons/search?q=char&addon=someone-else").1,
        json!({ "groups": [] })
    );

    // Switched off, it is not asked; switched on again, it is.
    let off = client.post(
        "/api/addons/enabled",
        json!({ "id": "demo.folder", "enabled": false }),
    );
    assert_eq!(off["addons"][0]["enabled"].as_bool(), Some(false));
    assert_eq!(
        client.get("/api/addons/search?q=char").1,
        json!({ "groups": [] })
    );
    client.post(
        "/api/addons/enabled",
        json!({ "id": "demo.folder", "enabled": true }),
    );

    // Playing what the add-on found: the window hands back the tracks, the core asks the add-on for each stream
    // when it is about to play. While the first is being asked for, the player says so.
    let (_, all) = client.get("/api/addons/search?q=a&limit=50");
    let mut wanted: Vec<Value> = all["groups"][0]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["title"] == "alpha" || t["title"] == "bravo")
        .cloned()
        .collect();
    wanted.sort_by_key(|t| t["title"].as_str().map(str::to_string));
    wanted.insert(
        1,
        json!({ "id": "999999", "title": "Nowhere", "artist": "Nobody" }),
    );
    let asked = client.post(
        "/api/player/play",
        json!({ "addon": "demo.folder", "tracks": wanted }),
    );
    assert_eq!(
        (
            asked["status"].as_str(),
            asked["track"]["title"].as_str(),
            asked["queue_len"].as_u64()
        ),
        (Some("loading"), Some("alpha"), Some(3))
    );
    let playing = client.player_until("the add-on's track playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "alpha" && s["duration_ms"] == 2000
    });
    assert_eq!(
        (
            playing["tier"].as_str(),
            playing["track"]["addon"]["addon"].as_str(),
            playing["track"]["id"].is_null()
        ),
        (Some("lossless"), Some("demo.folder"), true)
    );
    assert!(playing["track"].get("locator").is_none());
    // The engine reads the stream over HTTP and reports the same sound as from the file.
    let signal = client.player_until("the stream's signal path", |s| {
        s["signal"]["decoded"]["sample_rate"].is_number()
    })["signal"]
        .clone();
    assert_eq!(
        (
            signal["codec"].as_str(),
            signal["decoded"]["sample_rate"].as_u64(),
            signal["resampled"].as_bool()
        ),
        (Some("pcm_s16le"), Some(44_100), Some(false))
    );
    // A seek in a stream asks the add-on's server for the part from there.
    client.post("/api/player/seek", json!({ "seconds": 1.2 }));
    client.player_until("the seek in the stream", |s| {
        s["position_ms"]
            .as_u64()
            .is_some_and(|p| (950..=1400).contains(&p))
    });
    // On by itself to the next track the add-on has, past the one it does not have.
    let next = client.player_until("bravo, past the missing track", |s| {
        s["status"] == "playing" && s["track"]["title"] == "bravo"
    });
    assert_eq!(next["index"].as_u64(), Some(2));
    assert_eq!(
        next["error"].as_str(),
        Some("Nowhere could not be played: the add-on does not have it")
    );

    // The add-on is kept with the library: a core opened on the same library has it, without asking it anything.
    let again = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
    })
    .unwrap();
    let second = Client {
        agent: client.agent.clone(),
        base: format!("http://127.0.0.1:{}", again.port()),
    };
    std::thread::spawn(move || again.run());
    assert_eq!(
        second.get("/api/addons").1["addons"][0]["id"].as_str(),
        Some("demo.folder")
    );

    // Removed, it is gone, and a track of it that is still queued says so.
    let removed = client.post("/api/addons/remove", json!({ "id": "demo.folder" }));
    assert_eq!(removed, json!({ "addons": [] }));
    client.post("/api/player/play", json!({ "addon": "demo.folder", "tracks": [{ "id": "1", "title": "Orphan", "artist": "Nobody" }] }));
    let orphan = client.player_until("the orphan's failure", |s| {
        s["status"] == "idle" && s["error"].is_string()
    });
    assert_eq!(
        orphan["error"].as_str(),
        Some("Orphan could not be played: its add-on was removed")
    );

    let stopped = client.post("/api/player/stop", json!({}));
    assert_eq!(stopped["queue_len"].as_u64(), Some(0));
    client.player_until("idle", |s| s["status"] == "idle");
    let _ = std::fs::remove_dir_all(&dir);
}
