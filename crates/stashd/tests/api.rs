//! The local API from the outside: a folder is scanned into a library, the core serves it, and a client reads the
//! library and drives the player over HTTP, as the window does. The player part needs libmpv; without it the test
//! checks that the API says so.

use std::path::{Path, PathBuf};
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

    let stopped = client.post("/api/player/stop", json!({}));
    assert_eq!(stopped["queue_len"].as_u64(), Some(0));
    client.player_until("idle", |s| s["status"] == "idle");
    let _ = std::fs::remove_dir_all(&dir);
}
