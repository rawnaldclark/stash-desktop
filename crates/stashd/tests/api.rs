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
    silence_for(path, 2);
}

/// A WAV of `seconds` of silence, 16-bit stereo at 44.1 kHz.
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

    /// A file sent as a request's body.
    fn upload(&self, path: &str, bytes: &[u8]) -> (u16, Value) {
        let mut response = self
            .agent
            .post(&format!("{}{path}", self.base))
            .header("Content-Type", "application/octet-stream")
            .send(bytes)
            .unwrap();
        (
            response.status().as_u16(),
            response.body_mut().read_json().unwrap_or(Value::Null),
        )
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
        play_counts_between_seconds: None,
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
        (
            200,
            json!({ "tracks": 5, "albums": 0, "artists": 1, "liked": 0, "playlists": 0 })
        )
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
        play_counts_between_seconds: None,
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
        s["resting"] == true && s["error"].is_string()
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

#[test]
fn a_library_file_is_imported_and_its_songs_play_through_an_add_on() {
    let dir = std::env::temp_dir().join(format!("stash-api-import-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The music is the add-on's, not the library's: the library starts empty.
    let catalogue = dir.join("Catalogue");
    std::fs::create_dir_all(&catalogue).unwrap();
    for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
        silence(&catalogue.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    let engine = engine();
    let core = Core::bind(Options {
        db: db.clone(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        play_counts_between_seconds: None,
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

    // What is not a Stash library file is refused in words, and nothing comes in.
    let (status, refused) = client.upload("/api/import", b"just some text");
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (400, Some("That file is not a Stash library file"))
    );
    let (status, refused) = client.upload("/api/import", br#"{"kind":"stash-web-library","v":9}"#);
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (
            400,
            Some("That file comes from a newer Stash than this one")
        )
    );

    // A file as the Android app or the web player writes it. The songs are known to the demo add-on as files without
    // tags, so by "Unknown artist".
    let song =
        |title: &str| json!({ "title": title, "artist": "Unknown artist", "durationMs": 2000 });
    let mut stale = song("charlie");
    stale["refs"] = json!({ "demo.folder": "999999", "youtube": "AE005nZeF-A" });
    let mut wrong_length = song("echo");
    wrong_length["durationMs"] = json!(95_000);
    let file = json!({
        "kind": "stash-web-library", "v": 1, "generator": "Stash for Android 0.9.125",
        "likes": [
            { "item": song("bravo"), "likedAt": 1_759_838_400_300i64 },
            { "item": { "title": "Nowhere", "artist": "Nobody" }, "likedAt": 1_759_838_400_200i64 },
            { "item": song("alpha"), "likedAt": 1_759_838_400_100i64 }
        ],
        "playlists": [{ "id": "app-7", "name": "Mix", "items": [stale, wrong_length, song("delta")] }],
        "history": [{ "item": song("alpha"), "playedAt": 1_759_838_402_000i64 }]
    })
    .to_string();
    let (status, imported) = client.upload("/api/import", file.as_bytes());
    assert_eq!(status, 200, "{imported}");
    assert_eq!(
        (imported["source"].as_str(), imported["generator"].as_str()),
        (Some("web-library"), Some("Stash for Android 0.9.125"))
    );
    assert_eq!(
        imported["summary"],
        json!({ "songs_added": 6, "songs_already": 0, "likes_added": 3, "likes_already": 0, "playlists_added": 1, "playlists_merged": 0, "playlist_songs_added": 0, "plays_added": 1, "plays_already": 0 })
    );
    assert_eq!(
        imported["counts"],
        json!({ "tracks": 6, "albums": 0, "artists": 2, "liked": 3, "playlists": 1 })
    );

    // What came in is listed: liked songs newest like first, the playlist in its order.
    let titles = |rows: &Value| {
        rows["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let (_, liked) = client.get("/api/liked");
    assert_eq!(titles(&liked), ["bravo", "Nowhere", "alpha"]);
    assert_eq!(
        (
            liked["rows"][0]["origin"].as_str(),
            liked["rows"][0]["liked"].as_bool()
        ),
        (Some("described"), Some(true))
    );
    let (_, playlists) = client.get("/api/playlists");
    let mix = &playlists["rows"][0];
    assert_eq!(
        (
            mix["id"].as_i64(),
            mix["name"].as_str(),
            mix["tracks"].as_u64(),
            mix["followed"].as_bool(),
            mix["external_id"].as_str()
        ),
        (Some(1), Some("Mix"), Some(3), Some(false), Some("app-7"))
    );
    assert_eq!(playlists["rows"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        titles(&client.get("/api/playlists/1/tracks").1),
        ["charlie", "echo", "delta"]
    );
    assert_eq!(
        titles(&client.get("/api/playlists/1/tracks?offset=2&limit=5").1),
        ["delta"]
    );
    // And found by search, like anything else in the library.
    assert_eq!(titles(&client.get("/api/search?q=nowh").1), ["Nowhere"]);

    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        return;
    }

    // With no add-on there is nowhere to play a described song from, and the player says so.
    client.post("/api/player/play", json!({ "liked": true }));
    let nowhere = client.player_until("the refusal without add-ons", |s| {
        s["resting"] == true && s["error"].is_string()
    });
    assert_eq!(
        nowhere["error"].as_str(),
        Some(
            "alpha could not be played: it is not a file here, and there is no add-on to play it from"
        )
    );

    let demo = DemoAddon::bind(&catalogue, 0, Duration::from_millis(60)).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());
    client.post("/api/addons", json!({ "link": link }));

    // The liked songs from the top: bravo is asked of the add-on (does it have this song?), then streamed.
    let asked = client.post("/api/player/play", json!({ "liked": true }));
    assert_eq!(
        (
            asked["status"].as_str(),
            asked["track"]["title"].as_str(),
            asked["queue_len"].as_u64()
        ),
        (Some("loading"), Some("bravo"), Some(3))
    );
    let bravo = client.player_until("bravo playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "bravo"
    });
    assert_eq!(
        (
            bravo["served_by"].as_str(),
            bravo["tier"].as_str(),
            bravo["track"]["addon"].is_null(),
            bravo["track"]["id"].is_number()
        ),
        (Some("demo.folder"), Some("lossless"), true, true)
    );
    // On by itself: the add-on does not have "Nowhere", so alpha is next, and the song left out is named.
    let alpha = client.player_until("alpha, past the song no add-on has", |s| {
        s["status"] == "playing" && s["track"]["title"] == "alpha"
    });
    assert_eq!(
        (alpha["index"].as_u64(), alpha["error"].as_str()),
        (
            Some(2),
            Some("Nowhere could not be played: none of your add-ons has it")
        )
    );

    // The playlist: charlie came in with a name the add-on no longer knows; the add-on is asked for the song again.
    // echo is in the add-on's folder under the same words but is a recording of another length: not the same song.
    client.post("/api/player/play", json!({ "playlist": 1 }));
    let charlie = client.player_until("charlie playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "charlie"
    });
    assert_eq!(charlie["served_by"].as_str(), Some("demo.folder"));
    let delta = client.player_until("delta, past echo", |s| {
        s["status"] == "playing" && s["track"]["title"] == "delta"
    });
    assert_eq!(
        (delta["index"].as_u64(), delta["error"].as_str()),
        (
            Some(2),
            Some("echo could not be played: none of your add-ons has it")
        )
    );
    client.post("/api/player/stop", json!({}));

    // What the add-on calls each song it played is kept with the song; the stale name was replaced, the other
    // source's name left alone.
    let library = Library::open(&db).unwrap();
    let refs = |title: &str| {
        let id = library.search(title, 1).unwrap()[0].id;
        library.describe(id).unwrap().unwrap().refs
    };
    let charlie_refs = refs("charlie");
    assert_eq!(charlie_refs.len(), 2);
    assert!(
        charlie_refs
            .iter()
            .any(|(source, theirs)| source == "demo.folder" && theirs != "999999")
    );
    assert!(charlie_refs.contains(&("youtube".to_string(), "AE005nZeF-A".to_string())));
    assert_eq!(refs("bravo").len(), 1);
    assert!(refs("Nowhere").is_empty());
    assert!(refs("echo").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_is_done_here_goes_out_as_a_library_file() {
    let dir = std::env::temp_dir().join(format!("stash-api-lists-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let catalogue = dir.join("Catalogue");
    std::fs::create_dir_all(&catalogue).unwrap();
    for name in ["alpha", "bravo", "charlie"] {
        silence(&catalogue.join(format!("{name}.wav")));
    }
    let engine = engine();
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        // The rule's floor of 30 s would leave nothing to count in 2 s of silence: here half the track decides.
        play_counts_between_seconds: Some((0.2, 240.0)),
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
    let demo = DemoAddon::bind(&catalogue, 0, Duration::from_millis(20)).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());
    client.post("/api/addons", json!({ "link": link }));

    // A track the add-on found is kept: it becomes a song of the library, once.
    let (_, found) = client.get("/api/addons/search?q=a&limit=10");
    let track = |title: &str| {
        found["groups"][0]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["title"] == title)
            .unwrap()
            .clone()
    };
    let keep = |title: &str| {
        client.post(
            "/api/tracks/keep",
            json!({ "addon": "demo.folder", "track": track(title) }),
        )["id"]
            .as_i64()
            .unwrap()
    };
    let (alpha, bravo, charlie) = (keep("alpha"), keep("bravo"), keep("charlie"));
    assert_eq!(keep("alpha"), alpha);
    assert_eq!(client.get("/api/counts").1["tracks"], 3);

    // Liked, and the like taken back.
    let liked = client.post("/api/tracks/like", json!({ "id": alpha, "liked": true }));
    assert_eq!(
        (liked["liked"].as_bool(), liked["counts"]["liked"].as_u64()),
        (Some(true), Some(1))
    );
    client.post("/api/tracks/like", json!({ "id": bravo, "liked": true }));
    let unliked = client.post("/api/tracks/like", json!({ "id": bravo, "liked": false }));
    assert_eq!(unliked["counts"]["liked"].as_u64(), Some(1));
    assert_eq!(
        client
            .send("/api/tracks/like", json!({ "id": 999, "liked": true }))
            .0,
        404
    );

    // A playlist: made, filled, reordered, renamed. A playlist without a name is refused in words.
    let (status, refused) = client.send("/api/playlists", json!({ "name": "   " }));
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (400, Some("A playlist needs a name"))
    );
    let made = client.post("/api/playlists", json!({ "name": "Made here" }));
    let mix = made["id"].as_i64().unwrap();
    let row = &made["playlists"][0];
    assert_eq!(
        (
            row["id"].as_i64(),
            row["name"].as_str(),
            row["tracks"].as_u64(),
            row["followed"].as_bool()
        ),
        (Some(mix), Some("Made here"), Some(0), Some(false))
    );
    assert!(
        row["external_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("desk-"))
            && row["created_at_ms"].is_number()
    );
    let added = client.post(
        &format!("/api/playlists/{mix}/add"),
        json!({ "ids": [alpha, bravo, charlie, alpha] }),
    );
    assert_eq!(
        (
            added["added"].as_u64(),
            added["playlists"][0]["tracks"].as_u64()
        ),
        (Some(3), Some(3))
    );
    let titles = |rows: &Value| {
        rows["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        client.post(
            &format!("/api/playlists/{mix}/move"),
            json!({ "from": 2, "to": 0 })
        )["done"],
        true
    );
    assert_eq!(
        titles(&client.get(&format!("/api/playlists/{mix}/tracks")).1),
        ["charlie", "alpha", "bravo"]
    );
    assert_eq!(
        client.post(
            &format!("/api/playlists/{mix}/remove"),
            json!({ "index": 2 })
        )["playlists"][0]["tracks"],
        2
    );
    assert_eq!(
        client.post(
            &format!("/api/playlists/{mix}/remove"),
            json!({ "index": 7 })
        )["done"],
        false
    );
    assert_eq!(
        client.post(
            &format!("/api/playlists/{mix}/rename"),
            json!({ "name": "Late drive" })
        )["playlists"][0]["name"],
        "Late drive"
    );

    // Played past its half, a song's play is kept.
    let export = || {
        let mut response = client
            .agent
            .get(&format!("{}/api/export", client.base))
            .call()
            .unwrap();
        let named = response
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        (named, response.body_mut().read_json::<Value>().unwrap())
    };
    if engine.is_some() {
        client.post("/api/player/play", json!({ "playlist": mix }));
        client.player_until("charlie playing", |s| {
            s["status"] == "playing" && s["track"]["title"] == "charlie"
        });
        let deadline = Instant::now() + Duration::from_secs(8);
        while export().1["history"].as_array().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the play was not kept");
            std::thread::sleep(Duration::from_millis(50));
        }
        client.post("/api/player/stop", json!({}));
    } else {
        eprintln!("SKIPPED the player: no libmpv found");
    }

    // The library goes out as the file the other Stashes read.
    let (named, file) = export();
    assert!(
        named.starts_with("attachment; filename=\"stash-library-") && named.ends_with(".json\""),
        "{named}"
    );
    assert_eq!(
        (file["kind"].as_str(), file["v"].as_i64()),
        (Some("stash-web-library"), Some(1))
    );
    assert!(
        file["generator"]
            .as_str()
            .unwrap()
            .starts_with("Stash desktop ")
    );
    assert_eq!(file["likes"].as_array().unwrap().len(), 1);
    let like = &file["likes"][0]["item"];
    assert_eq!(
        (
            like["title"].as_str(),
            like["artist"].as_str(),
            like["durationMs"].as_u64(),
            like["refs"]["demo.folder"].is_string()
        ),
        (Some("alpha"), Some("Unknown artist"), Some(2000), true)
    );
    let playlist = &file["playlists"][0];
    assert_eq!(playlist["name"], "Late drive");
    assert_eq!(
        playlist["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["charlie", "alpha"]
    );
    if engine.is_some() {
        assert_eq!(file["history"][0]["item"]["title"], "charlie");
    }
    // The core's own import reads what it wrote: everything is here already.
    let (status, again) = client.upload("/api/import", file.to_string().as_bytes());
    assert_eq!(status, 200);
    assert_eq!(
        (
            again["summary"]["songs_added"].as_u64(),
            again["summary"]["likes_added"].as_u64(),
            again["summary"]["playlists_added"].as_u64(),
            again["summary"]["plays_added"].as_u64()
        ),
        (Some(0), Some(0), Some(0), Some(0))
    );

    let deleted = client.post(&format!("/api/playlists/{mix}/delete"), json!({}));
    assert_eq!(
        (
            deleted["done"].as_bool(),
            deleted["playlists"].as_array().map(Vec::len),
            deleted["counts"]["tracks"].as_u64()
        ),
        (Some(true), Some(0), Some(3))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_add_on_made_for_eclipse_is_added_searched_and_played_from() {
    let dir = std::env::temp_dir().join(format!("stash-api-eclipse-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let catalogue = dir.join("Catalogue");
    std::fs::create_dir_all(&catalogue).unwrap();
    for name in ["alpha", "bravo"] {
        silence(&catalogue.join(format!("{name}.wav")));
    }
    let engine = engine();
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        play_counts_between_seconds: None,
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
    let demo = DemoAddon::bind_as(&catalogue, 0, Duration::from_millis(20), true).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());

    // Its manifest has no `stash`, and its id is written with capitals: it is taken as the contract maps it.
    let added = client.post("/api/addons", json!({ "link": link }));
    let listed = &added["addons"][0];
    assert_eq!(
        (
            listed["id"].as_str(),
            listed["name"].as_str(),
            listed["problem"].is_null()
        ),
        (
            Some("demo.folder.eclipse"),
            Some("Demo folder (Eclipse)"),
            true
        )
    );
    assert_eq!(listed["resources"], json!(["search", "stream", "resolve"]));
    // An address with nothing at it is said to have no add-on.
    let (status, refused) = client.send(
        "/api/addons",
        json!({ "link": link.replace("manifest.json", "nothing/manifest.json") }),
    );
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (502, Some("there is no add-on at that address"))
    );

    // Its search rows have numbers for ids and lengths in seconds.
    let (_, found) = client.get("/api/addons/search?q=bravo");
    let track = found["groups"][0]["tracks"][0].clone();
    assert_eq!(
        (
            track["title"].as_str(),
            track["duration_ms"].as_u64(),
            track["id"].is_string(),
            track["max_tier"].is_null()
        ),
        (Some("bravo"), Some(2000), true, true)
    );

    // A library song known by its description: the add-on is asked whether it has it, in Eclipse's way.
    let file = json!({ "kind": "stash-web-library", "v": 1, "likes": [{ "item": { "title": "alpha", "artist": "Unknown artist", "durationMs": 2000 }, "likedAt": 5 }] }).to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);

    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        return;
    }
    client.post(
        "/api/player/play",
        json!({ "addon": "demo.folder.eclipse", "tracks": [track] }),
    );
    let playing = client.player_until("the Eclipse add-on's track playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "bravo"
    });
    // It said "LOSSLESS" and a wav: read as lossless, and the engine agrees.
    assert_eq!(
        (playing["tier"].as_str(), playing["served_by"].as_str()),
        (Some("lossless"), Some("demo.folder.eclipse"))
    );
    let signal = client.player_until("its signal path", |s| {
        s["signal"]["decoded"]["sample_rate"].is_number()
    })["signal"]
        .clone();
    assert_eq!(signal["codec"].as_str(), Some("pcm_s16le"));

    client.post("/api/player/play", json!({ "liked": true }));
    let liked = client.player_until("the liked song from the Eclipse add-on", |s| {
        s["status"] == "playing" && s["track"]["title"] == "alpha"
    });
    assert_eq!(
        (liked["served_by"].as_str(), liked["error"].is_null()),
        (Some("demo.folder.eclipse"), true)
    );
    client.post("/api/player/stop", json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_song_is_found_by_searching_an_add_on_that_cannot_be_asked_for_it() {
    let dir = std::env::temp_dir().join(format!("stash-api-matching-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let catalogue = dir.join("Catalogue");
    std::fs::create_dir_all(&catalogue).unwrap();
    for name in ["alpha", "alpha (Live)", "bravo - 2011 Remaster"] {
        silence(&catalogue.join(format!("{name}.wav")));
    }
    let engine = engine();
    let db = dir.join("library.db");
    let core = Core::bind(Options {
        db: db.clone(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        play_counts_between_seconds: None,
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
    // An add-on as many are: made for Eclipse, with search and streams and nothing else.
    let demo = DemoAddon::bind_with(&catalogue, 0, Duration::from_millis(20), true, false).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());
    let added = client.post("/api/addons", json!({ "link": link }));
    assert_eq!(added["addons"][0]["resources"], json!(["search", "stream"]));

    let song =
        |title: &str| json!({ "title": title, "artist": "Unknown artist", "durationMs": 2000 });
    let file = json!({ "kind": "stash-web-library", "v": 1, "playlists": [{ "id": "p1", "name": "Mix", "items": [song("Alpha"), song("Charlie"), song("Bravo")] }] }).to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);
    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        return;
    }

    // Alpha: the add-on has it twice; the live take is not the song. Charlie: it has nothing like it. Bravo: it has
    // it under a release label.
    client.post("/api/player/play", json!({ "playlist": 1 }));
    let alpha = client.player_until("Alpha playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "Alpha"
    });
    assert_eq!(alpha["served_by"].as_str(), Some("demo.folder.eclipse"));
    let bravo = client.player_until("Bravo, past Charlie", |s| {
        s["status"] == "playing" && s["track"]["title"] == "Bravo"
    });
    assert_eq!(
        (bravo["index"].as_u64(), bravo["error"].as_str()),
        (
            Some(2),
            Some("Charlie could not be played: none of your add-ons has it")
        )
    );
    client.post("/api/player/stop", json!({}));

    // What the add-on calls each song it played is kept, so the next time there is nothing to search for; the take
    // that was picked for Alpha is the studio one.
    let library = Library::open(&db).unwrap();
    let kept = |title: &str| {
        library
            .describe(library.search(title, 1).unwrap()[0].id)
            .unwrap()
            .unwrap()
            .refs
    };
    let (_, found) = client.get("/api/addons/search?q=alpha");
    let studio = found["groups"][0]["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "alpha")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(kept("Alpha"), [("demo.folder.eclipse".to_string(), studio)]);
    assert_eq!(kept("Bravo").len(), 1);
    assert!(kept("Charlie").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_is_scanned_from_the_window_and_the_players_settings_are_kept() {
    let dir = std::env::temp_dir().join(format!("stash-api-basics-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["alpha", "bravo", "charlie", "delta"] {
        silence(&music.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    let engine = engine();
    let options = || Options {
        db: db.clone(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        play_counts_between_seconds: None,
    };
    let core = Core::bind(options()).unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let client = Client { agent, base };

    // Nothing to scan yet; a folder that is not there is refused in words.
    let (_, idle) = client.get("/api/scan");
    assert_eq!(
        (
            idle["scan"]["running"].as_bool(),
            idle["folders"].as_array().map(Vec::len)
        ),
        (Some(false), Some(0))
    );
    assert_eq!(
        client.send("/api/scan", json!({})).1["error"]["message"],
        "There is no music folder to scan yet"
    );
    assert_eq!(
        client
            .send("/api/scan", json!({ "folder": dir.join("nowhere") }))
            .1["error"]["message"],
        "That folder was not found"
    );

    // The folder is read in the background; when it is done the library has its songs and remembers the folder.
    client.post("/api/scan", json!({ "folder": music }));
    let scanned = |client: &Client, finished: u64| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, state) = client.get("/api/scan");
            if state["scan"]["finished"].as_u64() == Some(finished)
                && state["scan"]["running"] == false
            {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "the scan did not finish: {state}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let done = scanned(&client, 1);
    assert_eq!(
        (
            done["scan"]["report"]["files"].as_u64(),
            done["scan"]["report"]["added"].as_u64(),
            done["scan"]["error"].is_null()
        ),
        (Some(4), Some(4), true)
    );
    assert_eq!(done["folders"].as_array().map(Vec::len), Some(1));
    assert_eq!(client.get("/api/counts").1["tracks"], 4);
    // Scan again, with no folder named: the remembered one; a new file is found, the rest are as they were.
    silence(&music.join("echo.wav"));
    client.post("/api/scan", json!({}));
    let again = scanned(&client, 2);
    assert_eq!(
        (
            again["scan"]["report"]["added"].as_u64(),
            again["scan"]["report"]["unchanged"].as_u64()
        ),
        (Some(1), Some(4))
    );
    assert_eq!(client.get("/api/counts").1["tracks"], 5);

    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        return;
    }
    // The volume and the sound card setting.
    let quieter = client.post("/api/player/volume", json!({ "percent": 42 }));
    assert_eq!(
        (quieter["volume"].as_u64(), quieter["exclusive"].as_bool()),
        (Some(42), Some(false))
    );
    assert_eq!(
        client.post("/api/player/exclusive", json!({ "exclusive": true }))["exclusive"],
        true
    );

    // The queue, and a jump into it.
    client.post("/api/player/play", json!({ "offset": 0 }));
    client.player_until("alpha playing", |s| {
        s["status"] == "playing" && s["track"]["title"] == "alpha"
    });
    let (_, queue) = client.get("/api/player/queue");
    let rows = queue["rows"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| (r["place"].as_u64().unwrap(), r["title"].as_str().unwrap()))
            .collect::<Vec<_>>(),
        [
            (0, "alpha"),
            (1, "bravo"),
            (2, "charlie"),
            (3, "delta"),
            (4, "echo")
        ]
    );
    assert!(rows[0].get("locator").is_none());
    assert_eq!(
        client.get("/api/player/queue?offset=3&limit=1").1["rows"][0]["title"],
        "delta"
    );
    client.post("/api/player/jump", json!({ "index": 3 }));
    let delta = client.player_until("delta after the jump", |s| {
        s["status"] == "playing" && s["track"]["title"] == "delta"
    });
    assert_eq!(
        (delta["index"].as_u64(), delta["queue_len"].as_u64()),
        (Some(3), Some(5))
    );
    client.post("/api/player/stop", json!({}));

    // A core opened on the same library later has the volume and the setting as they were left.
    let later = Core::bind(options()).unwrap();
    let second = Client {
        agent: client.agent.clone(),
        base: format!("http://127.0.0.1:{}", later.port()),
    };
    std::thread::spawn(move || later.run());
    let (_, kept) = second.get("/api/player");
    assert_eq!(
        (kept["volume"].as_u64(), kept["exclusive"].as_bool()),
        (Some(42), Some(true))
    );
    assert_eq!(
        second.get("/api/scan").1["folders"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_lists_and_the_queue_the_window_is_built_on() {
    let dir = std::env::temp_dir().join(format!("stash-api-port-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["alpha", "bravo", "charlie", "delta"] {
        silence(&music.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
        // An album with a cover, from another Stash: two songs known by their description.
        let file = json!({ "kind": "stash-web-library", "v": 1, "likes": [
            { "item": { "title": "One", "artist": "Band", "album": "Record", "durationMs": 2000, "artwork": [{ "url": "https://covers.example/record.jpg" }] }, "likedAt": 9 },
            { "item": { "title": "Two", "artist": "Band", "album": "Record", "durationMs": 2000 }, "likedAt": 8 }
        ] }).to_string();
        library
            .import(&stash_core::import::read_bytes(file.as_bytes()).unwrap())
            .unwrap();
        let id = library.search("alpha", 1).unwrap()[0].id;
        library.record_play(id, 5_000).unwrap();
        library.record_play(id, 7_000).unwrap();
    }
    let engine = engine();
    let core = Core::bind(Options {
        db,
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: engine.clone(),
        play_counts_between_seconds: None,
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
    let titles = |rows: &Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };

    // A search finds albums and artists as well as songs: by the album's title, and an artist's albums by the name.
    let (_, found) = client.get("/api/search?q=reco");
    assert_eq!(titles(&found["rows"]).len(), 2, "{found}");
    assert_eq!(titles(&found["albums"]), ["Record"]);
    assert_eq!(
        found["albums"][0]["art"].as_str(),
        Some("https://covers.example/record.jpg")
    );
    assert_eq!(found["artists"], json!([]));
    let (_, found) = client.get("/api/search?q=band");
    assert_eq!(titles(&found["albums"]), ["Record"]);
    assert_eq!(found["artists"][0]["name"].as_str(), Some("Band"));
    assert_eq!(found["artists"][0]["tracks"].as_u64(), Some(2));
    // A song's title is not an album or an artist.
    let (_, found) = client.get("/api/search?q=alpha");
    assert_eq!(
        (
            found["rows"].as_array().unwrap().len(),
            &found["albums"],
            &found["artists"]
        ),
        (1, &json!([]), &json!([]))
    );

    // A row says what a list needs to draw and link it.
    let (_, liked) = client.get("/api/liked");
    let one = &liked["rows"][0];
    assert_eq!(
        (
            one["title"].as_str(),
            one["art"].as_str(),
            one["liked_at_ms"].as_i64(),
            one["origin"].as_str()
        ),
        (
            Some("One"),
            Some("https://covers.example/record.jpg"),
            Some(9),
            Some("described")
        )
    );
    assert!(
        one["album_id"].is_number()
            && one["artist_id"].is_number()
            && one["added_at_ms"].is_number()
    );
    // History: every play, the newest first.
    let (_, history) = client.get("/api/history");
    assert_eq!(
        history["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (
                r["title"].as_str().unwrap(),
                r["played_at_ms"].as_i64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("alpha", 7_000), ("alpha", 5_000)]
    );

    // Albums and artists, and one of each.
    let (_, albums) = client.get("/api/albums");
    let record = &albums["rows"][0];
    assert_eq!(
        (
            record["title"].as_str(),
            record["artist"].as_str(),
            record["tracks"].as_u64(),
            record["art"].as_str()
        ),
        (
            Some("Record"),
            Some("Band"),
            Some(2),
            Some("https://covers.example/record.jpg")
        )
    );
    let (_, album) = client.get(&format!("/api/albums/{}", record["id"]));
    assert_eq!(
        (album["album"]["title"].as_str(), titles(&album["rows"])),
        (Some("Record"), vec!["One".to_string(), "Two".into()])
    );
    assert_eq!(client.get("/api/albums/99999").0, 404);
    let (_, artists) = client.get("/api/artists");
    let band = artists["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Band")
        .unwrap()
        .clone();
    assert_eq!(
        (band["tracks"].as_u64(), band["albums"].as_u64()),
        (Some(2), Some(1))
    );
    let (_, artist) = client.get(&format!("/api/artists/{}", band["id"]));
    assert_eq!(
        (
            artist["artist"]["name"].as_str(),
            artist["rows"].as_array().map(Vec::len),
            artist["albums"][0]["title"].as_str()
        ),
        (Some("Band"), Some(2), Some("Record"))
    );

    if engine.is_none() {
        eprintln!("SKIPPED the player: no libmpv found");
        return;
    }
    // A list of chosen tracks, from a place in it, saying where from. An id the library does not have is left out,
    // and the place still means the same track.
    let (_, rows) = client.get("/api/tracks?offset=0&limit=10");
    let id = |title: &str| {
        rows["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["title"] == title)
            .unwrap()["id"]
            .as_i64()
            .unwrap()
    };
    let items = json!([{ "id": id("alpha") }, { "id": 999_999 }, { "id": id("bravo") }, { "id": id("charlie") }]);
    let started = client.post(
        "/api/player/play",
        json!({ "items": items, "start": 2, "from": "A test" }),
    );
    assert_eq!(
        (
            started["queue_len"].as_u64(),
            started["from"].as_str(),
            started["shuffle"].as_bool(),
            started["repeat"].as_str()
        ),
        (Some(3), Some("A test"), Some(false), Some("off"))
    );
    let bravo = client.player_until("bravo, the chosen one", |s| {
        s["status"] == "playing" && s["track"]["title"] == "bravo"
    });
    assert_eq!(
        (
            bravo["index"].as_u64(),
            bravo["track"]["duration_ms"].as_u64()
        ),
        (Some(1), Some(2000))
    );
    let version = bravo["queue_version"].as_u64().unwrap();

    // The queue is edited: one added next, one taken out, one moved.
    client.post(
        "/api/player/enqueue",
        json!({ "items": [{ "id": id("delta") }], "next": true }),
    );
    let queue = |client: &Client| titles(&client.get("/api/player/queue").1["rows"]);
    assert_eq!(queue(&client), ["alpha", "bravo", "delta", "charlie"]);
    client.post("/api/player/remove", json!({ "index": 0 }));
    let moved = client.post("/api/player/move", json!({ "from": 2, "to": 1 }));
    assert_eq!(queue(&client), ["bravo", "charlie", "delta"]);
    assert!(moved["queue_version"].as_u64().unwrap() > version);
    assert_eq!(
        (moved["index"].as_u64(), moved["track"]["title"].as_str()),
        (Some(0), Some("bravo"))
    );
    let next = client.player_until("charlie, as the edited queue says", |s| {
        s["status"] == "playing" && s["index"] == 1
    });
    assert_eq!(next["track"]["title"], "charlie");

    assert_eq!(
        client.post("/api/player/shuffle", json!({ "on": true }))["shuffle"],
        true
    );
    assert_eq!(
        client.post("/api/player/repeat", json!({ "mode": "all" }))["repeat"],
        "all"
    );
    client.post("/api/player/shuffle", json!({ "on": false }));
    assert_eq!(queue(&client), ["bravo", "charlie", "delta"]);

    // An album from one of its rows on.
    let album_play = client.post(
        "/api/player/play",
        json!({ "album": record["id"], "offset": 1, "from": "Record" }),
    );
    assert_eq!(
        (
            album_play["queue_len"].as_u64(),
            album_play["track"]["title"].as_str(),
            album_play["from"].as_str()
        ),
        (Some(1), Some("Two"), Some("Record"))
    );
    client.post("/api/player/stop", json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_screen_of_the_window_is_the_page_and_a_missing_file_is_missing() {
    let dir = std::env::temp_dir().join(format!("stash-api-ui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ui = dir.join("ui");
    std::fs::create_dir_all(ui.join("_app")).unwrap();
    std::fs::write(
        ui.join("index.html"),
        "<!doctype html><title>the page</title>",
    )
    .unwrap();
    std::fs::write(ui.join("_app").join("start.js"), "export {}").unwrap();
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: Some(ui),
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
    })
    .unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let fetch = |path: &str| {
        let mut response = agent.get(&format!("{base}{path}")).call().unwrap();
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let status = response.status().as_u16();
        (
            status,
            kind,
            response.body_mut().read_to_string().unwrap_or_default(),
        )
    };

    let (status, kind, body) = fetch("/");
    assert_eq!((status, kind.starts_with("text/html")), (200, true));
    assert!(body.contains("the page"));
    // A screen's address is the page again, however deep.
    for screen in ["/library", "/album/lib/7", "/settings"] {
        let (status, kind, body) = fetch(screen);
        assert_eq!(
            (status, kind.starts_with("text/html")),
            (200, true),
            "{screen}"
        );
        assert!(body.contains("the page"), "{screen}");
    }
    // A real file is itself; a missing one is missing, not the page.
    let (status, kind, _) = fetch("/_app/start.js");
    assert_eq!((status, kind.starts_with("text/javascript")), (200, true));
    assert_eq!(fetch("/_app/gone.js").0, 404);
    // The API is never the page, and nothing climbs out of the folder.
    assert_eq!(fetch("/api/nothing").0, 404);
    assert!(!fetch("/api/nothing").2.contains("the page"));
    assert_eq!(fetch("/..%2Flibrary.db").0 / 100, 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_files_picture_is_served_and_a_file_without_one_stops_offering_it() {
    let dir = std::env::temp_dir().join(format!("stash-api-art-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let with = dir.join("With");
    let without = dir.join("Without");
    let beside = dir.join("Beside");
    for folder in [&without, &beside] {
        std::fs::create_dir_all(folder).unwrap();
    }
    stash_core::synth::write_files(&with, 3, 4, 0.1, true).unwrap();
    silence(&without.join("bare.wav"));
    silence(&without.join("unplugged.wav"));
    silence(&beside.join("kept.wav"));
    std::fs::write(beside.join("Folder.png"), b"\x89PNG\r\n\x1a\nthe rest").unwrap();
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &dir, |_| {}).unwrap();
    }
    let core = Core::bind(Options {
        db,
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
    })
    .unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let picture = |address: &str| {
        let mut response = agent.get(&format!("{base}{address}")).call().unwrap();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let (kind, kept) = (
            header("content-type"),
            format!("{} vary={}", header("cache-control"), header("vary")),
        );
        let status = response.status().as_u16();
        (
            status,
            kind,
            kept,
            response.body_mut().read_to_vec().unwrap_or_default(),
        )
    };
    let client = Client {
        agent: agent.clone(),
        base: base.clone(),
    };
    let row = |title: &str| {
        let (_, found) = client.get(&format!("/api/search?q={title}&limit=1"));
        found["rows"][0].clone()
    };

    // Every file is offered a picture at first: whether it has one is not known until it is asked for.
    let bare = row("bare");
    let address = bare["art"].as_str().unwrap().to_string();
    assert_eq!(address, format!("/api/art/track/{}", bare["id"]));
    let (status, ..) = picture(&address);
    assert_eq!(status, 404);
    // Asked once and found without: the row no longer offers one.
    assert!(row("bare")["art"].is_null());

    // A file that cannot be reached right now (its drive is unplugged) is not judged: it is offered again.
    std::fs::remove_file(without.join("unplugged.wav")).unwrap();
    let away = row("unplugged");
    assert_eq!(picture(away["art"].as_str().unwrap()).0, 404);
    assert!(row("unplugged")["art"].is_string());

    // The picture inside a file, as the image its bytes are, and the window may keep it.
    let (_, all) = client.get("/api/tracks?offset=0&limit=10");
    let inside = all["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["album"].is_string())
        .unwrap()
        .clone();
    let (status, kind, kept, bytes) = picture(inside["art"].as_str().unwrap());
    assert_eq!((status, kind.as_str()), (200, "image/bmp"));
    // Kept, but not across origins: the colour-reading request must get its own answer.
    assert!(
        kept.contains("max-age") && kept.contains("vary=Origin"),
        "{kept}"
    );
    assert_eq!(&bytes[..2], b"BM");
    // Its album and its artist show the same picture.
    let (_, album) = client.get(&format!("/api/albums/{}", inside["album_id"]));
    assert!(
        album["album"]["art"]
            .as_str()
            .unwrap()
            .starts_with("/api/art/track/")
    );
    let (_, artist) = client.get(&format!("/api/artists/{}", inside["artist_id"]));
    assert!(
        artist["artist"]["art"]
            .as_str()
            .unwrap()
            .starts_with("/api/art/track/")
    );

    // The folder's picture, for a file with none inside.
    let kept_row = row("kept");
    let (status, kind, _, _) = picture(kept_row["art"].as_str().unwrap());
    assert_eq!((status, kind.as_str()), (200, "image/png"));
    assert!(row("kept")["art"].is_string());

    // Not a track, not a number.
    assert_eq!(picture("/api/art/track/999999").0, 404);
    assert_eq!(picture("/api/art/track/..%2Fx").0, 404);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_queue_is_there_again_when_the_program_is_opened_again() {
    let dir = std::env::temp_dir().join(format!("stash-api-kept-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["alpha", "bravo", "charlie", "delta"] {
        silence(&music.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
    }
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the kept queue: no libmpv found");
        return;
    };
    let open = || {
        let core = Core::bind(Options {
            db: db.clone(),
            ui: None,
            port: 0,
            bench_out: None,
            audio: Some(Output::Silent),
            mpv: Some(engine.clone()),
            play_counts_between_seconds: None,
        })
        .unwrap();
        let base = format!("http://127.0.0.1:{}", core.port());
        std::thread::spawn(move || core.run());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Client { agent, base }
    };
    let kept = |key: &str| {
        Library::open(&db)
            .unwrap()
            .setting(key)
            .unwrap()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null)
    };
    let until_kept = |what: &str, done: &dyn Fn(&Value, &Value) -> bool| {
        let started = Instant::now();
        loop {
            let (queue, place) = (kept("queue"), kept("queue.place"));
            if done(&queue, &place) {
                return (queue, place);
            }
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "waited 8 s for {what}; kept: {queue} / {place}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };

    // The first run: the library plays from its third song, repeat is on, and it is paused a second in.
    let first = open();
    assert_eq!(first.get("/api/player").1["status"], "idle");
    first.post("/api/player/repeat", json!({ "mode": "all" }));
    first.post(
        "/api/player/play",
        json!({ "offset": 0, "from": "Your library" }),
    );
    first.post("/api/player/jump", json!({ "index": 2 }));
    first.player_until("the third song", |s| {
        s["status"] == "playing" && s["index"] == 2
    });
    first.post("/api/player/pause", json!({ "paused": true }));
    first.post("/api/player/seek", json!({ "seconds": 1.0 }));
    first.player_until("paused a second in", |s| {
        s["status"] == "paused"
            && s["position_ms"]
                .as_u64()
                .is_some_and(|ms| (900..1500).contains(&ms))
    });
    let (queue, place) = until_kept(
        "the queue and the place to be written down",
        &|queue, place| {
            queue["items"]
                .as_array()
                .is_some_and(|items| items.len() == 4)
                && place["place"] == 2
                && place["seconds"] == 1.0
        },
    );
    // A song of the library is kept by its row, nothing more.
    assert!(
        queue["items"].as_array().unwrap().iter().all(Value::is_i64),
        "{queue}"
    );
    assert_eq!(
        (place["repeat"].as_str(), place["from"].as_str()),
        (Some("all"), Some("Your library"))
    );

    // Before the next run the first song's file goes away.
    std::fs::remove_file(music.join("alpha.wav")).unwrap();
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
    }

    // The second run opens on the same song, paused where it was, in a queue without the song that is gone.
    let second = open();
    let back = second.get("/api/player").1;
    assert_eq!(
        (
            back["status"].as_str(),
            back["resting"].as_bool(),
            back["index"].as_u64(),
            back["queue_len"].as_u64(),
            back["track"]["title"].as_str(),
            back["position_ms"].as_u64(),
            back["repeat"].as_str(),
            back["from"].as_str(),
        ),
        (
            Some("paused"),
            Some(true),
            Some(1),
            Some(3),
            Some("charlie"),
            Some(1000),
            Some("all"),
            Some("Your library")
        )
    );
    let (_, rows) = second.get("/api/player/queue?offset=0&limit=10");
    let titles: Vec<&str> = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["bravo", "charlie", "delta"]);
    // Nothing plays by itself; play goes on from the second it was left at.
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(second.get("/api/player").1["resting"], true);
    second.post("/api/player/pause", json!({ "paused": false }));
    let on = second.player_until("the song to go on", |s| {
        s["status"] == "playing"
            && s["resting"] == false
            && s["index"] == 1
            && s["position_ms"].as_u64().is_some_and(|ms| ms >= 900)
    });
    assert_eq!(on["track"]["title"].as_str(), Some("charlie"));

    // Stopped, there is nothing to bring back.
    second.post("/api/player/stop", json!({}));
    until_kept("the empty queue to be written down", &|queue, _| {
        queue["items"].as_array().is_some_and(Vec::is_empty)
    });
    let third = open();
    let empty = third.get("/api/player").1;
    assert_eq!(
        (
            empty["status"].as_str(),
            empty["queue_len"].as_u64(),
            empty["repeat"].as_str()
        ),
        (Some("idle"), Some(0), Some("all"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_sound_output_is_chosen_and_kept() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the sound outputs: no libmpv found");
        return;
    };
    let dir = std::env::temp_dir().join(format!("stash-api-device-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("library.db");
    let open = || {
        let core = Core::bind(Options {
            db: db.clone(),
            ui: None,
            port: 0,
            bench_out: None,
            audio: Some(Output::Silent),
            mpv: Some(engine.clone()),
            play_counts_between_seconds: None,
        })
        .unwrap();
        let base = format!("http://127.0.0.1:{}", core.port());
        std::thread::spawn(move || core.run());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Client { agent, base }
    };

    let first = open();
    let (_, listed) = first.get("/api/player/devices");
    assert_eq!(
        listed["devices"][0]["name"].as_str(),
        Some("auto"),
        "{listed}"
    );
    assert_eq!(
        (listed["device"].as_str(), &listed["missing"]),
        (Some("auto"), &Value::Null)
    );
    // An output that is not there is refused.
    let refused = first
        .agent
        .post(format!("{}/api/player/device", first.base))
        .send_json(json!({ "name": "nowhere/1" }))
        .unwrap();
    assert_eq!(refused.status().as_u16(), 400);
    // One that is: chosen, and kept for the next run.
    let last = listed["devices"].as_array().unwrap().last().unwrap()["name"]
        .as_str()
        .unwrap()
        .to_string();
    let chosen = first.post("/api/player/device", json!({ "name": last }));
    assert_eq!(chosen["device"].as_str(), Some(last.as_str()));
    // Evening out loudness: refused unless off, track or album; kept for the next run like the output.
    let wrong = first
        .agent
        .post(format!("{}/api/player/replaygain", first.base))
        .send_json(json!({ "mode": "loud" }))
        .unwrap();
    assert_eq!(wrong.status().as_u16(), 400);
    assert_eq!(
        first.post("/api/player/replaygain", json!({ "mode": "album" }))["replaygain"].as_str(),
        Some("album")
    );
    let second = open();
    assert_eq!(
        second.get("/api/player/devices").1["device"].as_str(),
        Some(last.as_str())
    );
    assert_eq!(
        second.get("/api/player").1["replaygain"].as_str(),
        Some("album")
    );

    // Chosen, then gone (unplugged) by the next run: the system's choice plays, and the window is told which one is missing.
    Library::open(&db)
        .unwrap()
        .set_setting(
            "device",
            &json!({ "name": "wasapi/{gone}", "description": "Headphones" }).to_string(),
        )
        .unwrap();
    let third = open();
    let (_, now) = third.get("/api/player/devices");
    assert_eq!(
        (
            now["device"].as_str(),
            now["missing"]["description"].as_str()
        ),
        (Some("auto"), Some("Headphones"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lyrics_come_from_the_file_then_the_add_on_and_are_kept() {
    let dir = std::env::temp_dir().join(format!("stash-api-lyrics-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (music, served) = (dir.join("Music"), dir.join("Served"));
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&served).unwrap();
    silence(&music.join("alpha.wav"));
    std::fs::write(
        music.join("alpha.lrc"),
        "[00:00.50]Alpha, from the file\n[00:01.50]and its second line\n",
    )
    .unwrap();
    silence(&music.join("bravo.wav"));
    silence(&served.join("charlie.wav"));
    std::fs::write(
        served.join("charlie.lrc"),
        "[00:00.20]Charlie, from the add-on\n",
    )
    .unwrap();
    silence(&served.join("delta.wav"));
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
        // Nothing reaches LRCLIB from a test.
        library.set_setting("lyrics.lrclib", "0").unwrap();
    }
    let addon = DemoAddon::bind(&served, 0, Duration::ZERO).unwrap();
    let link = addon.link();
    std::thread::spawn(move || addon.run());
    let core = Core::bind(Options {
        db: db.clone(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
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
    let id_of = |title: &str| {
        client.get(&format!("/api/search?q={title}&limit=1")).1["rows"][0]["id"]
            .as_i64()
            .unwrap()
    };

    // A file with a .lrc beside it: timed lines, each ending where the next starts.
    let (_, alpha) = client.get(&format!("/api/lyrics?id={}", id_of("alpha")));
    assert_eq!(alpha["via"].as_str(), Some("the .lrc file"));
    assert_eq!(
        alpha["lyrics"]["synced"][0],
        json!({ "startMs": 500, "endMs": 1500, "text": "Alpha, from the file" })
    );
    assert_eq!(
        alpha["lyrics"]["plain"].as_str(),
        Some("Alpha, from the file\nand its second line")
    );
    // A file without, and no add-on: none.
    let bravo_id = id_of("bravo");
    assert_eq!(
        client.get(&format!("/api/lyrics?id={bravo_id}")).1,
        json!({ "lyrics": null, "via": null })
    );

    // An add-on's track: its add-on is asked by its own id.
    client.post("/api/addons", json!({ "link": link }));
    let (_, found) = client.get("/api/addons/search?q=charlie&limit=1");
    let track = found["groups"][0]["tracks"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, charlie) = client.get(&format!(
        "/api/lyrics?addon=demo.folder&track={track}&title=charlie&artist=Unknown%20artist"
    ));
    assert_eq!(charlie["via"].as_str(), Some("Demo folder"));
    assert_eq!(
        charlie["lyrics"]["synced"][0]["text"].as_str(),
        Some("Charlie, from the add-on")
    );
    // Any song by what it is: the add-on is asked by title and artist; one it has none for is none.
    let (_, by_words) = client.get("/api/lyrics?title=charlie&artist=Unknown%20artist");
    assert_eq!(
        by_words["lyrics"]["synced"][0]["text"].as_str(),
        Some("Charlie, from the add-on")
    );
    assert_eq!(
        client
            .get("/api/lyrics?title=delta&artist=Unknown%20artist")
            .1["lyrics"],
        Value::Null
    );

    // What the network gave is kept: asked again with the add-on gone, the same lyrics.
    client.post("/api/addons/remove", json!({ "id": "demo.folder" }));
    let (_, kept) = client.get("/api/lyrics?title=charlie&artist=Unknown%20artist");
    assert_eq!(
        (
            kept["via"].as_str(),
            kept["lyrics"]["synced"][0]["text"].as_str()
        ),
        (Some("Demo folder"), Some("Charlie, from the add-on"))
    );

    // The LRCLIB switch.
    assert_eq!(
        client.get("/api/settings/lyrics").1,
        json!({ "lrclib": false })
    );
    assert_eq!(
        client.post("/api/settings/lyrics", json!({ "lrclib": true })),
        json!({ "lrclib": true })
    );
    assert_eq!(
        Library::open(&db)
            .unwrap()
            .setting("lyrics.lrclib")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn songs_from_add_ons_are_saved_as_files_and_given_back() {
    let dir = std::env::temp_dir().join(format!("stash-api-downloads-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (served, saved) = (dir.join("Served"), dir.join("Saved"));
    std::fs::create_dir_all(&served).unwrap();
    silence(&served.join("echo.wav"));
    silence(&served.join("foxtrot.wav"));
    let addon = DemoAddon::bind(&served, 0, Duration::ZERO).unwrap();
    let eclipse = DemoAddon::bind_as(&served, 0, Duration::ZERO, true).unwrap();
    let (link, eclipse_link) = (addon.link(), eclipse.link());
    std::thread::spawn(move || addon.run());
    std::thread::spawn(move || eclipse.run());
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
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
    let folder = saved.to_string_lossy().into_owned();
    assert_eq!(
        client.post("/api/downloads/folder", json!({ "folder": folder }))["folder"].as_str(),
        Some(folder.as_str())
    );
    client.post("/api/addons", json!({ "link": link }));
    client.post("/api/addons", json!({ "link": eclipse_link }));
    // A song an add-on found, kept in the library.
    let keep = |addon: &str, title: &str| -> i64 {
        let (_, found) = client.get(&format!(
            "/api/addons/search?q={title}&addon={addon}&limit=1"
        ));
        let track = found["groups"][0]["tracks"][0].clone();
        client.post(
            "/api/tracks/keep",
            json!({ "addon": addon, "track": track }),
        )["id"]
            .as_i64()
            .unwrap()
    };
    let until_settled = || -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (_, list) = client.get("/api/downloads");
            let c = &list["counts"];
            if c["queued"] == 0 && c["saving"] == 0 {
                return list;
            }
            assert!(Instant::now() < deadline, "still saving: {list}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    // Through the contract's download route.
    let echo = keep("demo.folder", "echo");
    assert_eq!(
        client.post("/api/downloads", json!({ "ids": [echo] }))["queued"],
        1
    );
    let list = until_settled();
    let row = &list["rows"][0];
    assert_eq!(
        (
            row["id"].as_i64(),
            row["state"].as_str(),
            row["origin"].as_str(),
            row["codec"].as_str(),
            row["addon"].as_str()
        ),
        (
            Some(echo),
            Some("saved"),
            Some("file"),
            Some("pcm"),
            Some("demo.folder")
        ),
        "{list}"
    );
    let file = saved.join("Unknown artist").join("echo.wav");
    assert!(file.is_file(), "{list}");
    // What the add-on was asked for is counted (its allowance is the listener's).
    let (_, used) = client.get("/api/addons/use?since=0");
    let demo = &used["addons"]["demo.folder"];
    assert_eq!(
        (demo["downloads"].as_u64(), demo["streams"].as_u64()),
        (Some(1), Some(0)),
        "{used}"
    );
    assert!(demo["found_at_ms"].is_i64(), "{used}");
    // Its tags say what the library knows it by.
    {
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;
        let tagged = lofty::read_from_path(&file).unwrap();
        let tag = tagged.primary_tag().unwrap();
        assert_eq!(
            (tag.title().as_deref(), tag.artist().as_deref()),
            (Some("echo"), Some("Unknown artist"))
        );
    }
    // Asked again: it is a file now, nothing to do.
    assert_eq!(
        client.post("/api/downloads", json!({ "ids": [echo] }))["queued"],
        0
    );

    // From an add-on made for Eclipse: its stream is the whole file.
    let foxtrot = keep("demo.folder.eclipse", "foxtrot");
    client.post("/api/downloads", json!({ "ids": [foxtrot] }));
    let list = until_settled();
    assert_eq!(list["counts"]["saved"], 2, "{list}");
    assert!(saved.join("Unknown artist").join("foxtrot.wav").is_file());

    // Given back: the file goes, its emptied folder with it, and the song plays through the add-ons again.
    client.post("/api/downloads/remove", json!({ "ids": [echo, foxtrot] }));
    assert!(!file.exists() && !saved.join("Unknown artist").exists());
    assert!(saved.is_dir(), "the downloads folder itself stays");
    let (_, rows) = client.get("/api/search?q=echo&limit=5");
    assert_eq!(rows["rows"][0]["origin"].as_str(), Some("described"));
    assert_eq!(
        client.get("/api/downloads").1["counts"],
        json!({ "queued": 0, "saving": 0, "saved": 0, "failed": 0 })
    );

    // With no add-on left that can save it, it fails, and says why.
    client.post("/api/addons/remove", json!({ "id": "demo.folder" }));
    client.post("/api/addons/remove", json!({ "id": "demo.folder.eclipse" }));
    client.post("/api/downloads", json!({ "ids": [echo] }));
    let list = until_settled();
    assert_eq!(
        (
            list["counts"]["failed"].as_u64(),
            list["rows"][0]["error"].as_str()
        ),
        (Some(1), Some("there is no add-on to save it from")),
        "{list}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autoplay_goes_on_with_songs_like_the_last_when_the_queue_ends() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED autoplay: no libmpv found");
        return;
    };
    let dir = std::env::temp_dir().join(format!("stash-api-autoplay-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("Music");
    std::fs::create_dir_all(&music).unwrap();
    for name in ["one", "two", "three", "four"] {
        silence_for(&music.join(format!("{name}.wav")), 4);
    }
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        scan_folder(&mut library, &music, |_| {}).unwrap();
    }
    let core = Core::bind(Options {
        db: db.clone(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: Some(engine),
        play_counts_between_seconds: None,
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
    let until = |what: &str, wanted: &dyn Fn(&Value) -> bool| -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (_, state) = client.get("/api/player");
            if wanted(&state) {
                return state;
            }
            assert!(Instant::now() < deadline, "waited for {what}: {state}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    assert_eq!(
        client.get("/api/settings/autoplay").1,
        json!({ "on": true })
    );
    let one = client.get("/api/search?q=one&limit=1").1["rows"][0]["id"]
        .as_i64()
        .unwrap();

    // One song queued: as it starts, the other three follow it (no file can be picked twice).
    client.post("/api/player/play", json!({ "items": [{ "id": one }] }));
    let grown = until("the queue to grow", &|s| s["queue_len"].as_u64() == Some(4));
    assert_eq!(grown["index"].as_u64(), Some(0));
    let (_, queue) = client.get("/api/player/queue?offset=0&limit=10");
    let mut ids: Vec<i64> = queue["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_i64())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4, "{queue}");

    // Switched off: the queue ends where it ends.
    assert_eq!(
        client.post("/api/settings/autoplay", json!({ "on": false })),
        json!({ "on": false })
    );
    client.post("/api/player/play", json!({ "items": [{ "id": one }] }));
    until("the song to start", &|s| {
        s["status"] == "playing" && s["queue_len"].as_u64() == Some(1)
    });
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(client.get("/api/player").1["queue_len"].as_u64(), Some(1));

    // The queue's last song can't be played (no add-on has it): it is passed over before it starts, and autoplay
    // goes on all the same.
    client.post("/api/settings/autoplay", json!({ "on": true }));
    let file = json!({
        "kind": "stash-web-library", "v": 1, "generator": "a test",
        "likes": [{ "item": { "title": "Nowhere", "artist": "Nobody" }, "likedAt": 1_759_838_400_000i64 }]
    })
    .to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);
    let id = |q: &str| {
        client.get(&format!("/api/search?q={q}&limit=1")).1["rows"][0]["id"]
            .as_i64()
            .unwrap()
    };
    client.post(
        "/api/player/play",
        json!({ "items": [{ "id": id("two") }, { "id": id("nowhere") }] }),
    );
    let grown = until(
        "the queue to grow past the song that can't be played",
        &|s| s["queue_len"].as_u64().is_some_and(|n| n > 2),
    );
    // The player says which song it passed over (the window offers to find it).
    assert_eq!(grown["error_id"].as_i64(), Some(id("nowhere")), "{grown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_page_asks_the_program_around_the_core_for_the_mini_player() {
    use stashd::serve::View;
    use std::sync::{Arc, Mutex};

    let dir = std::env::temp_dir().join(format!("stash-api-window-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let options = |name: &str| Options {
        db: dir.join(name),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let ask = |base: &str, body: Value| -> (u16, Value) {
        let mut response = agent
            .post(&format!("{base}/api/window"))
            .send_json(body)
            .unwrap();
        (
            response.status().as_u16(),
            response.body_mut().read_json().unwrap_or(Value::Null),
        )
    };

    // A core on its own (served to a browser) has no window to change.
    let alone = Core::bind(options("alone.db")).unwrap();
    let base = format!("http://127.0.0.1:{}", alone.port());
    std::thread::spawn(move || alone.run());
    let client = Client {
        agent: agent.clone(),
        base: base.clone(),
    };
    assert_eq!(client.get("/api/window"), (200, json!({ "mini": false })));
    assert_eq!(ask(&base, json!({ "view": "mini" })).0, 404);

    // Inside a program that owns its windows, the ask reaches it, and its answer is the reply.
    let asked: Arc<Mutex<Vec<View>>> = Arc::default();
    let works = Arc::new(Mutex::new(true));
    let shell = {
        let (asked, works) = (Arc::clone(&asked), Arc::clone(&works));
        Arc::new(move |view: View| {
            asked.lock().unwrap().push(view);
            *works.lock().unwrap()
        })
    };
    let windowed = Core::bind(options("windowed.db"))
        .unwrap()
        .with_window(shell);
    let base = format!("http://127.0.0.1:{}", windowed.port());
    std::thread::spawn(move || windowed.run());
    let client = Client {
        agent: agent.clone(),
        base: base.clone(),
    };
    assert_eq!(client.get("/api/window"), (200, json!({ "mini": true })));
    assert_eq!(
        client.post("/api/window", json!({ "view": "mini" })),
        json!({ "view": "mini" })
    );
    assert_eq!(
        client.post("/api/window", json!({ "view": "full" })),
        json!({ "view": "full" })
    );
    assert_eq!(ask(&base, json!({ "view": "small" })).0, 400);
    *works.lock().unwrap() = false;
    assert_eq!(ask(&base, json!({ "view": "mini" })).0, 503);
    assert_eq!(*asked.lock().unwrap(), [View::Mini, View::Full, View::Mini]);
}

#[test]
fn songs_no_add_on_finds_wait_to_be_fixed_by_hand() {
    let dir = std::env::temp_dir().join(format!("stash-api-unfound-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (served, saved) = (dir.join("Served"), dir.join("Saved"));
    std::fs::create_dir_all(&served).unwrap();
    silence(&served.join("golf.wav"));
    silence(&served.join("hotel.wav"));
    let addon = DemoAddon::bind(&served, 0, Duration::ZERO).unwrap();
    let link = addon.link();
    std::thread::spawn(move || addon.run());
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
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
    client.post(
        "/api/downloads/folder",
        json!({ "folder": saved.to_string_lossy() }),
    );
    // Golf is on the add-on; Hotel is too, but the library's length for it is not the add-on's (so asking for it
    // finds nothing); India is nowhere.
    let song = |title: &str, ms: u64| json!({ "title": title, "artist": "Unknown artist", "durationMs": ms });
    let file = json!({
        "kind": "stash-web-library", "v": 1, "generator": "a test",
        "likes": [
            { "item": song("golf", 2_000), "likedAt": 1_759_838_400_300i64 },
            { "item": song("hotel", 95_000), "likedAt": 1_759_838_400_200i64 },
            { "item": song("india", 2_000), "likedAt": 1_759_838_400_100i64 }
        ]
    })
    .to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);
    client.post("/api/addons", json!({ "link": link }));
    let id = |title: &str| {
        client.get(&format!("/api/search?q={title}&limit=1")).1["rows"][0]["id"]
            .as_i64()
            .unwrap()
    };
    let (golf, hotel, india) = (id("golf"), id("hotel"), id("india"));
    let save = |ids: &[i64]| {
        client.post("/api/downloads", json!({ "ids": ids }));
        client.post("/api/downloads/retry", json!({ "ids": ids }));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (_, list) = client.get("/api/downloads");
            if list["counts"]["queued"] == 0 && list["counts"]["saving"] == 0 {
                return list;
            }
            assert!(Instant::now() < deadline, "still saving: {list}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let unfound = || client.get("/api/unfound").1;

    // An add-on's "I don't have it" counts only once it is finding other songs it is asked for.
    save(&[india]);
    assert_eq!(unfound()["open"], 0);
    let list = save(&[golf]);
    assert_eq!(list["counts"]["saved"], 1, "{list}");
    // Hotel is asked now, and listed. India's "no" was said before, when it was not believed: it is not asked
    // again for a while, and does not count.
    save(&[india, hotel]);
    assert_eq!(unfound()["open"], 1, "{}", unfound());
    // Switched off and on again, the add-on is asked again, and now its "no" for India is believed.
    client.post(
        "/api/addons/enabled",
        json!({ "id": "demo.folder", "enabled": false }),
    );
    client.post(
        "/api/addons/enabled",
        json!({ "id": "demo.folder", "enabled": true }),
    );
    save(&[india]);
    let listed = unfound();
    let mut titles: Vec<&str> = listed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(
        (listed["open"].as_u64(), titles),
        (Some(2), vec!["hotel", "india"]),
        "{listed}"
    );
    let mut marked: Vec<i64> = serde_json::from_value(listed["marked"].clone()).unwrap();
    marked.sort_unstable();
    assert_eq!(marked, {
        let mut both = vec![hotel, india];
        both.sort_unstable();
        both
    });

    // The add-on's closest results for Hotel, searched as the library knows it, or in other words.
    let (status, found) = client.get(&format!("/api/unfound/{hotel}/candidates"));
    assert_eq!(status, 200, "{found}");
    assert_eq!(found["query"].as_str(), Some("Unknown artist hotel"));
    let tracks = &found["groups"][0]["tracks"];
    let pick = tracks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "hotel")
        .unwrap_or_else(|| panic!("{found}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        client
            .get(&format!("/api/unfound/{hotel}/candidates?q=golf"))
            .1["query"]
            .as_str(),
        Some("golf")
    );
    assert_eq!(client.get("/api/unfound/12345678/candidates").0, 404);

    // Picked: it leaves the list, and the save that failed goes again and comes from the pick.
    let fixed = client.post(
        "/api/unfound/fix",
        json!({ "id": hotel, "addon": "demo.folder", "track": pick }),
    );
    assert_eq!(
        (fixed["open"].as_u64(), fixed["saving"].as_bool()),
        (Some(1), Some(true)),
        "{fixed}"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while client.get("/api/downloads").1["counts"]["saved"] != 2 {
        assert!(
            Instant::now() < deadline,
            "{}",
            client.get("/api/downloads").1
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(saved.join("Unknown artist").join("hotel.wav").is_file());

    // "None of these": off the list, still known not to play.
    let aside = client.post("/api/unfound/aside", json!({ "ids": [india] }));
    assert_eq!(
        (aside["open"].as_u64(), aside["marked"].clone()),
        (Some(0), json!([india]))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_song_played_from_an_add_on_is_kept_and_plays_again_without_it() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED played copies: no libmpv found");
        return;
    };
    let dir = std::env::temp_dir().join(format!("stash-api-played-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let served = dir.join("Served");
    std::fs::create_dir_all(&served).unwrap();
    silence_for(&served.join("kilo.wav"), 3);
    let addon = DemoAddon::bind(&served, 0, Duration::ZERO).unwrap();
    let link = addon.link();
    std::thread::spawn(move || addon.run());
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: Some(engine),
        play_counts_between_seconds: None,
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
    let until = |what: &str, wanted: &dyn Fn() -> Option<Value>| -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(found) = wanted() {
                return found;
            }
            assert!(Instant::now() < deadline, "waited for {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let file = json!({
        "kind": "stash-web-library", "v": 1, "generator": "a test",
        "likes": [{ "item": { "title": "kilo", "artist": "Unknown artist", "durationMs": 3000 }, "likedAt": 1_759_838_400_000i64 }]
    })
    .to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);
    client.post("/api/addons", json!({ "link": link }));
    let kilo = client.get("/api/search?q=kilo&limit=1").1["rows"][0]["id"]
        .as_i64()
        .unwrap();
    assert_eq!(
        client.get("/api/played").1,
        json!({ "songs": 0, "bytes": 0, "cap_mb": 2048 })
    );

    // Played from the add-on: the engine reads it through the core, which keeps what passes.
    client.post("/api/player/play", json!({ "items": [{ "id": kilo }] }));
    let room = until("the copy to be kept", &|| {
        let (_, room) = client.get("/api/played");
        (room["songs"] == 1).then_some(room)
    });
    let size = std::fs::metadata(served.join("kilo.wav")).unwrap().len();
    assert_eq!(room["bytes"].as_u64(), Some(size), "{room}");
    let copy = dir.join("Played").join(format!("{kilo}.wav"));
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(served.join("kilo.wav")).unwrap()
    );

    // One stream was asked of the add-on.
    let (_, used) = client.get("/api/addons/use?since=0");
    assert_eq!(
        used["addons"]["demo.folder"]["streams"].as_u64(),
        Some(1),
        "{used}"
    );

    // With the add-on gone, it plays from the copy.
    client.post("/api/addons/remove", json!({ "id": "demo.folder" }));
    client.post("/api/player/stop", json!({}));
    client.post("/api/player/play", json!({ "items": [{ "id": kilo }] }));
    let state = until("it to play from the copy", &|| {
        let (_, s) = client.get("/api/player");
        (s["status"] == "playing").then_some(s)
    });
    assert!(state["served_by"].is_null(), "{state}");
    assert!(state["error"].is_null(), "{state}");

    // Cleared: the copy is gone.
    client.post("/api/player/stop", json!({}));
    assert_eq!(client.post("/api/played/clear", json!({}))["songs"], 0);
    assert!(!copy.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn smart_playlists_fill_themselves_and_are_not_edited_by_hand() {
    let dir = std::env::temp_dir().join(format!("stash-api-smart-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
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
    let song = |title: &str, artist: &str| json!({ "title": title, "artist": artist, "durationMs": 200_000 });
    let file = json!({
        "kind": "stash-web-library", "v": 1, "generator": "a test",
        "likes": [
            { "item": song("Quiet Rooms", "The Lanterns"), "likedAt": 1_759_838_400_300i64 },
            { "item": song("Afterglow", "Beyoncé"), "likedAt": 1_759_838_400_200i64 },
            { "item": song("Paper Boats", "The Lanterns"), "likedAt": 1_759_838_400_100i64 }
        ]
    })
    .to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()).0, 200);
    let titles = |answer: &Value| -> Vec<String> {
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap().to_string())
            .collect()
    };

    // Made with rules: listed with them, and as many songs as they pick.
    let made = client.post(
        "/api/playlists",
        json!({ "name": "Lanterns", "rules": { "rules": [{ "field": "artist", "op": "contains", "value": "lanterns" }], "order": "title" } }),
    );
    let smart = made["id"].as_i64().unwrap();
    let listed = made["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == smart)
        .unwrap()
        .clone();
    assert_eq!(listed["tracks"], 2, "{listed}");
    assert_eq!(listed["rules"]["order"], "title", "{listed}");
    assert_eq!(
        titles(&client.get(&format!("/api/playlists/{smart}/tracks")).1),
        ["Paper Boats", "Quiet Rooms"]
    );

    // Its songs are its rules': none are added, removed or moved by hand.
    let afterglow = client.get("/api/search?q=afterglow&limit=1").1["rows"][0]["id"].clone();
    for (verb, body) in [
        ("add", json!({ "ids": [afterglow] })),
        ("remove", json!({ "index": 0 })),
        ("move", json!({ "from": 0, "to": 1 })),
    ] {
        let (status, said) = client.send(&format!("/api/playlists/{smart}/{verb}"), body);
        assert_eq!(status, 409, "{verb}: {said}");
    }

    // The editor's preview: how many, and the first few; rules that make no sense are refused in words.
    let preview = client.post(
        "/api/playlists/preview",
        json!({ "rules": { "rules": [{ "field": "artist", "op": "contains", "value": "beyonce" }] } }),
    );
    assert_eq!(
        (preview["count"].as_u64(), titles(&preview)),
        (Some(1), vec!["Afterglow".to_string()])
    );
    let (status, said) = client.send(
        "/api/playlists/preview",
        json!({ "rules": { "rules": [{ "field": "artist", "op": "at_least", "value": 3 }] } }),
    );
    assert_eq!(status, 400, "{said}");
    assert!(said.to_string().contains("cannot be compared"), "{said}");

    // New rules; a playlist made by hand takes none.
    let changed = client.post(
        &format!("/api/playlists/{smart}/rules"),
        json!({ "rules": { "rules": [{ "field": "liked", "op": "is" }], "order": "title" } }),
    );
    assert_eq!(changed["done"], true, "{changed}");
    assert_eq!(
        titles(&client.get(&format!("/api/playlists/{smart}/tracks")).1),
        ["Afterglow", "Paper Boats", "Quiet Rooms"]
    );
    let plain = client.post("/api/playlists", json!({ "name": "By hand" }))["id"].clone();
    let refused = client.post(
        &format!("/api/playlists/{plain}/rules"),
        json!({ "rules": { "rules": [] } }),
    );
    assert_eq!(refused["done"], false, "{refused}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_web_page_cannot_use_the_core_and_the_window_can() {
    let dir = std::env::temp_dir().join(format!("stash-api-pages-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let core = Core::bind(Options {
        db: dir.join("library.db"),
        ui: None,
        port: 0,
        bench_out: None,
        audio: None,
        mpv: None,
        play_counts_between_seconds: None,
    })
    .unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let status = |headers: &[(&str, &str)]| {
        let mut asked = agent.get(format!("{base}/api/counts"));
        for (name, value) in headers {
            asked = asked.header(*name, *value);
        }
        asked.call().unwrap().status().as_u16()
    };
    let other = ("Sec-Fetch-Site", "cross-site");
    // A web page's image, or its no-cors request: from another site, naming no page of the window.
    assert_eq!(status(&[other]), 403);
    assert_eq!(
        status(&[other, ("Referer", "https://pages.example/a/page")]),
        403
    );
    assert_eq!(status(&[other, ("Origin", "https://pages.example")]), 403);
    // The window: by its origin, or, for its pictures, by the page they were asked from.
    assert_eq!(status(&[other, ("Origin", "http://tauri.localhost")]), 200);
    assert_eq!(
        status(&[other, ("Referer", "http://tauri.localhost/library")]),
        200
    );
    assert_eq!(status(&[other, ("Referer", "tauri://localhost/")]), 200);
    // The page the core serves itself, the listener typing the address, and programs that are not browsers.
    assert_eq!(status(&[("Sec-Fetch-Site", "same-origin")]), 200);
    assert_eq!(status(&[("Sec-Fetch-Site", "none")]), 200);
    assert_eq!(status(&[]), 200);
    let _ = std::fs::remove_dir_all(&dir);
}

/// How the stand-in for ListenBrainz behaves.
#[derive(Clone, Copy, PartialEq)]
enum Mood {
    Taking,
    Away,
    Refusing,
}

/// A stand-in for ListenBrainz: it checks a token, and what is sent, the way the service does; it keeps what it takes,
/// and it can be away, or refuse every token.
struct Brainz {
    url: String,
    mood: std::sync::Arc<std::sync::Mutex<Mood>>,
    got: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}

impl Brainz {
    fn start() -> Self {
        use std::sync::{Arc, Mutex};
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!(
            "http://127.0.0.1:{}",
            server.server_addr().to_ip().unwrap().port()
        );
        let mood = Arc::new(Mutex::new(Mood::Taking));
        let got = Arc::new(Mutex::new(Vec::new()));
        let (moods, kept) = (Arc::clone(&mood), Arc::clone(&got));
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let token = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let refused = json!({ "code": 401, "error": "Invalid authorization token." });
                let (status, answer) = match request.url() {
                    "/1/validate-token" if token == "Token good" => (
                        200,
                        json!({ "code": 200, "message": "Token valid.", "valid": true, "user_name": "listener" }),
                    ),
                    "/1/validate-token" => (
                        200,
                        json!({ "code": 200, "message": "Token invalid.", "valid": false }),
                    ),
                    "/1/submit-listens" => match *moods.lock().unwrap() {
                        _ if token != "Token good" => (401, refused),
                        Mood::Refusing => (401, refused),
                        Mood::Away => (503, Value::Null),
                        Mood::Taking => match checked(&body) {
                            Ok(taken) => {
                                kept.lock().unwrap().push(taken);
                                (200, json!({ "status": "ok" }))
                            }
                            Err(why) => (400, json!({ "code": 400, "error": why })),
                        },
                    },
                    _ => (404, json!({ "code": 404, "error": "Not found" })),
                };
                let text = if answer.is_null() {
                    String::new()
                } else {
                    answer.to_string()
                };
                let _ = request
                    .respond(tiny_http::Response::from_string(text).with_status_code(status));
            }
        });
        Brainz { url, mood, got }
    }

    fn be(&self, mood: Mood) {
        *self.mood.lock().unwrap() = mood;
    }

    /// The listens taken of one kind ("single", "import", "playing_now"), in the order they came.
    fn listens(&self, kind: &str) -> Vec<Value> {
        self.got
            .lock()
            .unwrap()
            .iter()
            .filter(|sent| sent["listen_type"] == kind)
            .flat_map(|sent| sent["payload"].as_array().cloned().unwrap_or_default())
            .collect()
    }

    /// The titles of the plays taken ("single" and "import").
    fn plays(&self) -> Vec<String> {
        let mut plays = self.listens("single");
        plays.extend(self.listens("import"));
        plays
            .iter()
            .map(|l| {
                l["track_metadata"]["track_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// What ListenBrainz checks of what is sent to it.
fn checked(body: &str) -> Result<Value, String> {
    let sent: Value =
        serde_json::from_str(body).map_err(|_| "Cannot parse JSON document".to_string())?;
    let kind = sent["listen_type"]
        .as_str()
        .ok_or("JSON document must contain listen_type")?;
    let payload = sent["payload"]
        .as_array()
        .ok_or("JSON document must contain payload")?;
    match kind {
        "single" | "playing_now" if payload.len() != 1 => {
            return Err(format!(
                "JSON document should contain exactly one listen for {kind}"
            ));
        }
        "import" if payload.is_empty() => {
            return Err("JSON document should not contain an empty payload".into());
        }
        "single" | "playing_now" | "import" => {}
        _ => return Err("listen_type must be one of 'single', 'import' or 'playing_now'".into()),
    }
    for listen in payload {
        for key in ["artist_name", "track_name"] {
            if !listen["track_metadata"][key]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            {
                return Err(format!(
                    "JSON document does not contain required track_metadata.{key}"
                ));
            }
        }
        match (kind, listen.get("listened_at")) {
            ("playing_now", Some(_)) => {
                return Err(
                    "JSON document must not contain listened_at while submitting playing_now"
                        .into(),
                );
            }
            ("single" | "import", None) => {
                return Err("JSON document must contain listened_at".into());
            }
            ("single" | "import", Some(at)) if !at.as_i64().is_some_and(|t| t > 1_033_430_400) => {
                return Err("Value for key listened_at is too low".into());
            }
            _ => {}
        }
    }
    Ok(sent)
}

/// Waits up to 10 s for `done`.
fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "waited 10 s for: {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn plays_go_to_listenbrainz_and_wait_while_it_is_away() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED scrobbling: no libmpv found");
        return;
    };
    let dir = std::env::temp_dir().join(format!("stash-api-scrobble-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (music, catalogue) = (dir.join("Music"), dir.join("Catalogue"));
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&catalogue).unwrap();
    silence(&music.join("untagged.wav"));
    for name in ["alpha", "bravo", "charlie"] {
        silence(&catalogue.join(format!("{name}.wav")));
    }
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        assert_eq!(scan_folder(&mut library, &music, |_| {}).unwrap().added, 1);
    }
    let brainz = Brainz::start();
    let core = Core::bind(Options {
        db,
        ui: None,
        port: 0,
        bench_out: None,
        audio: Some(Output::Silent),
        mpv: Some(engine),
        // The rule's floor of 30 s would leave nothing to count in 2 s of silence: here half the track decides.
        play_counts_between_seconds: Some((0.2, 240.0)),
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
    let demo = DemoAddon::bind(&catalogue, 0, Duration::from_millis(20)).unwrap();
    let link = demo.link();
    std::thread::spawn(move || demo.run());
    client.post("/api/addons", json!({ "link": link }));
    let target = || client.get("/api/scrobbling").1["targets"][0].clone();
    let play = |id: &str, title: &str, artist: &str| {
        client.post(
            "/api/player/play",
            json!({ "addon": "demo.folder", "tracks": [{ "id": id, "title": title, "artist": artist, "album": "Night Drive" }] }),
        );
    };

    // No service yet. A token the service does not take, no token, and an address that is not one are refused in words.
    assert_eq!(client.get("/api/scrobbling").1, json!({ "targets": [] }));
    let refused = |body: Value| {
        let (status, answer) = client.send("/api/scrobbling", body);
        (
            status,
            answer["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    };
    assert_eq!(
        refused(json!({ "token": "wrong", "url": brainz.url })),
        (400, "That server did not take that token".to_string())
    );
    assert_eq!(
        refused(json!({ "token": " ", "url": brainz.url })),
        (400, "A token is needed".to_string())
    );
    assert_eq!(
        refused(json!({ "token": "good", "url": "ftp://example.org" })).0,
        400
    );
    assert_eq!(
        refused(json!({ "token": "good", "url": format!("{}/elsewhere", brainz.url) })),
        (
            400,
            "Nothing at that address answers as ListenBrainz does".to_string()
        )
    );

    // The right token, at the address of one of the service's own paths: the service is kept at its root, under the
    // name it knows the listener by, and the token is never given back.
    let added = client.post(
        "/api/scrobbling",
        json!({ "token": "good", "url": format!("{}/1/submit-listens", brainz.url) }),
    );
    let id = added["id"].as_i64().unwrap();
    assert!(
        !added.to_string().contains("good"),
        "the token is not given back: {added}"
    );
    let kept = &added["targets"][0];
    assert_eq!(
        (
            kept["url"].as_str(),
            kept["user"].as_str(),
            kept["enabled"].as_bool(),
            kept["waiting"].as_i64()
        ),
        (
            Some(brainz.url.as_str()),
            Some("listener"),
            Some(true),
            Some(0)
        )
    );

    // A file without tags plays and counts, and is not sent: "Unknown artist" names no one.
    client.post("/api/player/play", json!({ "offset": 0 }));
    until("the untagged file's play to count", || {
        client.get("/api/history?offset=0&limit=5").1["rows"]
            .as_array()
            .is_some_and(|r| r.len() == 1)
    });

    // A song from an add-on: shown as playing now, then sent once its play counts.
    let before = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 1) as i64;
    play("1", "Paper Boats", "The Lanterns");
    until("the play to be taken", || brainz.plays() == ["Paper Boats"]);
    let now = brainz.listens("playing_now");
    assert_eq!(now[0]["track_metadata"]["track_name"], "Paper Boats");
    assert!(
        now.iter()
            .all(|l| l["track_metadata"]["track_name"] != "untagged")
    );
    let single = &brainz.listens("single")[0];
    let at = single["listened_at"].as_i64().unwrap();
    assert!(
        at >= before && at <= before + 10,
        "listened_at {at} is when it began"
    );
    let song = &single["track_metadata"];
    assert_eq!(
        (
            song["artist_name"].as_str(),
            song["release_name"].as_str(),
            song["additional_info"]["media_player"].as_str()
        ),
        (Some("The Lanterns"), Some("Night Drive"), Some("Stash"))
    );
    until("the service to show it sent", || target()["sent"] == 1);
    assert_eq!(
        (target()["waiting"].as_i64(), target()["trouble"].is_null()),
        (Some(0), true)
    );

    // The service is away: the play waits, with why in words, and goes when the service is asked again.
    brainz.be(Mood::Away);
    play("2", "Afterglow", "Mira Sol");
    until("the play to wait", || {
        target()["waiting"] == 1 && target()["trouble"]["message"] == "it answered 503"
    });
    brainz.be(Mood::Taking);
    client.post(&format!("/api/scrobbling/{id}/again"), json!({}));
    until("the waiting play to be taken", || target()["sent"] == 2);
    assert_eq!(brainz.plays(), ["Paper Boats", "Afterglow"]);
    assert_eq!(
        (target()["waiting"].as_i64(), target()["trouble"].is_null()),
        (Some(0), true)
    );

    // The token is refused: plays wait and nothing is sent, until the token is given again.
    brainz.be(Mood::Refusing);
    play("3", "Cinder Hands", "Oda Lune");
    until("the token to be refused", || {
        target()["refused"] == true && target()["waiting"] == 1
    });
    assert_eq!(target()["trouble"]["message"], "it refused the token");
    brainz.be(Mood::Taking);
    client.post(
        "/api/scrobbling",
        json!({ "token": "good", "url": brainz.url }),
    );
    until("the play to go once the token is given again", || {
        target()["sent"] == 3
    });
    assert_eq!(brainz.plays(), ["Paper Boats", "Afterglow", "Cinder Hands"]);
    assert_eq!(target()["refused"], false);

    // Off: plays are not kept for it.
    let off = client.post(
        &format!("/api/scrobbling/{id}/enabled"),
        json!({ "enabled": false }),
    );
    assert_eq!(off["targets"][0]["enabled"], false);
    play("1", "Paper Boats", "The Lanterns");
    // Past the point its play counts (1 s of 2 s), then to its end: the play counted, and was not kept for the service.
    client.player_until("the song past the point its play counts", |s| {
        s["track"]["title"] == "Paper Boats"
            && s["status"] == "playing"
            && s["position_ms"].as_u64().is_some_and(|p| p >= 1100)
    });
    let ended = client.player_until("the song to end", |s| s["resting"] == true);
    assert!(ended["error"].is_null(), "it played through: {ended}");
    assert_eq!(
        (target()["waiting"].as_i64(), target()["sent"].as_i64()),
        (Some(0), Some(3))
    );
    assert_eq!(brainz.plays().len(), 3);

    // Removed; a service that is not there is said to be so.
    assert_eq!(
        client.post(&format!("/api/scrobbling/{id}/remove"), json!({})),
        json!({ "targets": [] })
    );
    assert_eq!(
        client
            .send(&format!("/api/scrobbling/{id}/again"), json!({}))
            .0,
        404
    );
    let _ = std::fs::remove_dir_all(&dir);
}
