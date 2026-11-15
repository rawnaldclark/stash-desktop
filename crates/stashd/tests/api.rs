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
        s["status"] == "idle" && s["error"].is_string()
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
