//! The equalizer over HTTP: Bass and Treble, a correction from a file, the switch, what is refused and in which
//! words, and what is kept for the next run. The core plays to the silent output; without libmpv the tests say so and
//! pass. The equalizer files here are made up in the test.

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

/// A WAV of `seconds` of silence, 16-bit stereo at 44.1 kHz.
fn silence(path: &Path, seconds: u32) {
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
    fn get(&self, path: &str) -> Value {
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.base))
            .call()
            .unwrap();
        response.body_mut().read_json().unwrap_or(Value::Null)
    }

    /// A POST, whatever its status.
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

    fn player_until(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let state = self.get("/api/player");
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

/// A core on the database `db`, playing to the silent output.
fn open(db: &Path, engine: &Path) -> Client {
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
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    Client { agent, base }
}

/// A scratch folder with a library of one song, "quiet"; its database and the song's id.
fn library_of_one(name: &str) -> (PathBuf, PathBuf, i64) {
    let dir = std::env::temp_dir().join(format!("stash-api-eq-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let music = dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    silence(&music.join("quiet.wav"), 30);
    let db = dir.join("library.db");
    let mut library = Library::open(&db).unwrap();
    scan_folder(&mut library, &music, |_| {}).unwrap();
    let id = library.track_ids_at(0, 10).unwrap()[0];
    (dir, db, id)
}

fn kept(db: &Path) -> Option<String> {
    Library::open(db).unwrap().setting("eq").unwrap()
}

/// An equalizer file as AutoEq writes them for Equalizer APO, made up: three filters.
const PHONES: &str = "Preamp: -4.2 dB\nFilter 1: ON LSC Fc 105 Hz Gain 3.5 dB Q 0.70\nFilter 2: ON PK Fc 2400 Hz Gain -2.1 dB Q 1.80\nFilter 3: ON HSC Fc 10000 Hz Gain 1.5 dB Q 0.70\n";

#[test]
fn bass_and_treble_are_set_kept_and_heard() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the equalizer: no libmpv found");
        return;
    };
    let (dir, db, id) = library_of_one("tones");
    let first = open(&db, &engine);
    let idle = first.get("/api/player");
    assert_eq!(
        (
            idle["eq"]["on"].as_bool(),
            idle["eq"]["output"]["name"].as_str()
        ),
        (Some(false), Some("auto")),
        "off on every output at first: {}",
        idle["eq"]
    );
    let set = first.post("/api/player/eq", json!({ "bass": "more" }));
    assert_eq!(
        (
            set["eq"]["on"].as_bool(),
            set["eq"]["bass"].as_str(),
            set["eq"]["treble"].as_str(),
            set["eq"]["softer_db"].as_f64()
        ),
        (Some(true), Some("more"), Some("as-is"), Some(5.0)),
        "a tone chosen turns the equalizer on: {}",
        set["eq"]
    );
    first.post("/api/player/play", json!({ "ids": [id] }));
    let playing = first.player_until("the song equalized", |s| {
        s["status"] == "playing" && s["signal"]["equalized"] == true
    });
    assert_eq!(
        playing["signal"]["scaled"], false,
        "equalized is not scaled"
    );
    // Off: the switch alone, the tones kept for when it is on again.
    let off = first.post("/api/player/eq", json!({ "on": false }));
    assert_eq!(
        (off["eq"]["on"].as_bool(), off["eq"]["bass"].as_str()),
        (Some(false), Some("more"))
    );
    first.player_until("the song as it is", |s| s["signal"]["equalized"] == false);
    // A tone chosen with "on" in the same request: the request says.
    let quiet = first.post("/api/player/eq", json!({ "treble": "less", "on": false }));
    assert_eq!(
        (quiet["eq"]["on"].as_bool(), quiet["eq"]["treble"].as_str()),
        (Some(false), Some("less"))
    );
    first.post("/api/player/eq", json!({ "on": true }));

    // Twenty in a row: the last is the one heard and kept.
    let words = ["less", "as-is", "more"];
    for n in 0..20 {
        first.post(
            "/api/player/eq",
            json!({ "bass": words[n % 3], "treble": words[(n + 1) % 3] }),
        );
    }
    let last = first.get("/api/player");
    assert_eq!(
        (last["eq"]["bass"].as_str(), last["eq"]["treble"].as_str()),
        (Some(words[19 % 3]), Some(words[20 % 3]))
    );
    first.post("/api/player/stop", json!({}));

    // The next run: the same.
    let second = open(&db, &engine);
    let again = second.get("/api/player");
    assert_eq!(
        (
            again["eq"]["on"].as_bool(),
            again["eq"]["bass"].as_str(),
            again["eq"]["treble"].as_str()
        ),
        (Some(true), Some(words[19 % 3]), Some(words[20 % 3])),
        "{}",
        again["eq"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_is_refused_is_said_and_changes_nothing() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the equalizer: no libmpv found");
        return;
    };
    let (dir, db, _) = library_of_one("refused");
    let client = open(&db, &engine);
    client.post("/api/player/eq", json!({ "bass": "less" }));
    let before = kept(&db);
    assert!(before.is_some());
    for (body, words) in [
        (json!({ "bass": "loud" }), "bass is less, as-is or more"),
        (json!({ "treble": 5 }), "treble is less, as-is or more"),
        (json!({ "on": "yes" }), "on is true or false"),
        (
            json!({ "correction": {} }),
            "A correction comes from a file; here it can only be removed (null)",
        ),
        (json!({}), "say on, bass, treble or correction"),
        (
            json!({ "loud": true }),
            "say on, bass, treble or correction",
        ),
    ] {
        let (status, answer) = client.send("/api/player/eq", body.clone());
        assert_eq!(
            (status, answer["error"]["message"].as_str()),
            (400, Some(words)),
            "{body}: {answer}"
        );
    }

    // Files: a missing one, a graphic EQ, one that picks channels, one too big.
    let file = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    };
    let graphic = file("Graphic.txt", "GraphicEQ: 20 -3; 40 -2; 80 1\n");
    let channels = file(
        "Channels.txt",
        "Preamp: -3 dB\nFilter 1: ON PK Fc 100 Hz Gain 3 dB Q 1\nChannel: L\n",
    );
    let big = file("Big.txt", &"#".repeat(65 * 1024));
    let gone = dir.join("Gone.txt").to_string_lossy().into_owned();
    for (path, words) in [
        (String::new(), "Choose an equalizer file".to_string()),
        (gone, "That file was not found".to_string()),
        (
            graphic,
            "This file is a graphic EQ (GraphicEQ). Stash reads the parametric kind: on autoeq.app, download it for Equalizer APO as ParametricEQ.".to_string(),
        ),
        (
            channels,
            "Line 3 picks channels (Channel: L). Stash puts one correction on both sides, so the file was not brought in.".to_string(),
        ),
        (big, "That file is too big to be an equalizer file".to_string()),
    ] {
        let (status, answer) = client.send("/api/player/eq/file", json!({ "path": path }));
        assert_eq!(
            (status, answer["error"]["message"].as_str()),
            (400, Some(words.as_str())),
            "{path}: {answer}"
        );
    }
    assert_eq!(kept(&db), before, "nothing refused was kept");
    assert_eq!(
        client.get("/api/player")["eq"]["bass"].as_str(),
        Some("less")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_correction_comes_from_a_file_and_is_kept() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the equalizer: no libmpv found");
        return;
    };
    let (dir, db, _) = library_of_one("file");
    let client = open(&db, &engine);
    let path = dir.join("Test Phones ParametricEQ.txt");
    std::fs::write(&path, PHONES).unwrap();
    let brought = client.post(
        "/api/player/eq/file",
        json!({ "path": path.to_string_lossy() }),
    );
    let correction = &brought["eq"]["correction"];
    assert_eq!(
        (
            brought["eq"]["on"].as_bool(),
            correction["name"].as_str(),
            correction["file"].as_str(),
            correction["filters"].as_u64()
        ),
        (
            Some(true),
            Some("Test Phones"),
            Some("Test Phones ParametricEQ.txt"),
            Some(3)
        ),
        "{correction}"
    );
    assert_eq!(
        correction["summary"].as_str(),
        Some("Read 3 filters and a preamp of −4.2 dB from Test Phones ParametricEQ.txt.")
    );
    assert_eq!(brought["eq"]["describe"].as_str(), Some("Test Phones"));

    // Room EQ Wizard's headers are passed over, as Equalizer APO passes them over.
    let rew = dir.join("Room.txt");
    std::fs::write(
        &rew,
        format!("Filter Settings file\n\nRoom EQ V5.20\nDated: Nov 16, 2026\n\nNotes:\n\nEqualiser: Generic\n{PHONES}"),
    )
    .unwrap();
    let room = client.post(
        "/api/player/eq/file",
        json!({ "path": rew.to_string_lossy() }),
    );
    assert_eq!(room["eq"]["correction"]["name"].as_str(), Some("Room"));
    assert!(
        room["eq"]["correction"]["passed_over"].as_u64().unwrap() >= 4,
        "{}",
        room["eq"]
    );

    // Kept for the next run.
    let next = open(&db, &engine);
    assert_eq!(
        next.get("/api/player")["eq"]["correction"]["name"].as_str(),
        Some("Room")
    );
    // Removed.
    let removed = next.post("/api/player/eq", json!({ "correction": null }));
    assert!(removed["eq"]["correction"].is_null(), "{}", removed["eq"]);
    assert!(
        !kept(&db).unwrap_or_default().contains("Room"),
        "the correction is no longer kept"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_is_kept_and_cannot_be_used_leaves_the_equalizer_off() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED the equalizer: no libmpv found");
        return;
    };
    let (dir, db, id) = library_of_one("garbage");
    // Garbage: the core starts, the equalizer is off, and songs play.
    Library::open(&db)
        .unwrap()
        .set_setting("eq", "{not json")
        .unwrap();
    let client = open(&db, &engine);
    assert_eq!(client.get("/api/player")["eq"]["on"].as_bool(), Some(false));
    client.post("/api/player/play", json!({ "ids": [id] }));
    client.player_until("the song playing", |s| {
        s["status"] == "playing" && s["signal"]["equalized"] == false
    });
    client.post("/api/player/stop", json!({}));

    // An equalizer only for an output that is gone: the output in use has none.
    let book = json!({
        "v": 1,
        "last": null,
        "outputs": { "wasapi/{gone}": { "description": "Headphones", "on": true, "bass": "more", "treble": "as-is", "correction": null } }
    });
    Library::open(&db)
        .unwrap()
        .set_setting("eq", &book.to_string())
        .unwrap();
    let next = open(&db, &engine);
    let eq = next.get("/api/player")["eq"].clone();
    assert_eq!(
        (eq["output"]["name"].as_str(), eq["on"].as_bool()),
        (Some("auto"), Some(false)),
        "{eq}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
