//! The phone transfer over HTTP, as the window drives it: a folder stands in for the phone (`STASH_PHONE_FOLDER`), with
//! a send of songs and the phone's library beside them. A test program of its own: the core keeps one board for every
//! phone and the folder is set for the whole program, so no other core may run beside it (the API tests' cores would
//! look at it too).

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::Library;
use stashd::serve::{Core, Options};

/// Two seconds of stereo silence, 16-bit 44.1 kHz, with no tags.
fn silence(path: &Path) {
    let data: u32 = 44_100 * 4 * 2;
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
        let mut response = self.agent.get(&format!("{}{path}", self.base)).call().unwrap();
        (
            response.status().as_u16(),
            response.body_mut().read_json().unwrap_or(Value::Null),
        )
    }

    fn post(&self, path: &str, body: Value) -> (u16, Value) {
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

    /// `GET /api/phone` until it says `state`.
    fn phone_until(&self, state: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (_, now) = self.get("/api/phone");
            if now["state"] == state {
                return now;
            }
            assert!(Instant::now() < deadline, "waited 20 s for {state}; the phone says {now}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

const SENT_AT: i64 = 1_764_000_000_000;

#[test]
fn a_send_with_the_phones_library_is_summed_up_and_brought_in_as_chosen() {
    let dir = std::env::temp_dir().join(format!("stash-phone-api-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let folder = dir.join("Stash transfer");
    let downloads = dir.join("Music");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    // A folder for the phone, never a phone: set before anything reads it (this is the program's only test, and no
    // core has started yet).
    unsafe {
        std::env::set_var("STASH_PHONE_FOLDER", &folder);
        std::env::remove_var("STASH_NO_PHONE");
    }
    let db = dir.join("library.db");
    Library::open(&db)
        .unwrap()
        .set_setting("downloads.folder", &downloads.to_string_lossy())
        .unwrap();

    // The phone copies its songs out, then its library; the list comes last.
    silence(&folder.join("1.wav"));
    silence(&folder.join("2.wav"));
    let size = std::fs::metadata(folder.join("1.wav")).unwrap().len();
    let ivy = json!({ "title": "Ivy", "artist": "Frank Ocean", "isrc": "USUM71607010",
                      "artwork": [{ "url": "https://art/ivy" }] });
    let nights = json!({ "title": "Nights", "artist": "Frank Ocean" });
    let halo = json!({ "title": "Halo", "artist": "Beyoncé", "artwork": [{ "url": "https://art/halo" }] });
    let library = json!({
        "kind": "stash-web-library", "v": 1, "generator": "Stash for Android", "sent_at_ms": SENT_AT,
        "likes": [{ "item": ivy, "likedAt": 1000 }, { "item": halo, "likedAt": 1001 }],
        "playlists": [
            { "id": "app-9", "name": "Daily Mix 1", "from": "spotify", "items": [halo] },
            { "id": "app-2", "name": "Night drive", "items": [ivy, nights] },
        ],
        "history": [{ "item": ivy, "playedAt": 2000 }, { "item": nights, "playedAt": 1500 }],
    });
    std::fs::write(folder.join("stash-library.json"), library.to_string()).unwrap();

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
    let client = Client { agent, base };

    // No list yet: nothing is waiting, so neither is a library.
    assert_eq!(client.get("/api/phone/library").0, 404);
    let list = json!({ "stash_transfer": 1, "sent_at_ms": SENT_AT, "songs": [
        { "file": "1.wav", "title": "Ivy", "artist": "Frank Ocean", "album": "Blonde", "isrc": "USUM71607010",
          "art_url": "https://i.scdn.co/blonde" },
        { "file": "2.wav", "title": "Nights", "artist": "Frank Ocean", "album": "Blonde" },
    ] });
    std::fs::write(folder.join("stash-transfer.json"), list.to_string()).unwrap();
    client.post("/api/phone/look", json!({}));

    let now = client.phone_until("ready");
    assert_eq!(
        (now["total"].as_u64(), now["with_library"].as_bool(), now["library_choice_kept"].as_bool()),
        (Some(2), Some(true), Some(false)),
        "a library waits with the send, and nothing was chosen for this phone yet: the first run"
    );
    let (status, summary) = client.get("/api/phone/library");
    assert_eq!(status, 200);
    assert_eq!(
        summary,
        json!({
            "sent_at_ms": SENT_AT,
            "phone": "Test folder",
            "downloads": { "songs": 2, "bytes": 2 * size, "here": 0 },
            "likes": { "songs": 2, "downloaded": 1 },
            "plays": 2,
            "playlists": [
                { "id": "app-2", "name": "Night drive", "from": null, "songs": 2, "downloaded": 2,
                  "covers": ["https://art/ivy"] },
                { "id": "app-9", "name": "Daily Mix 1", "from": "spotify", "songs": 1, "downloaded": 0,
                  "covers": ["https://art/halo"] },
            ],
            "albums": [{ "title": "Blonde", "artist": "Frank Ocean", "art_url": "https://i.scdn.co/blonde", "songs": 2 }],
            "choice": null,
        })
    );

    // A playlist that is not in the library, or a library field that is not one: refused, and nothing taken.
    let (status, _) = client.post(
        "/api/phone/take",
        json!({ "sent_at_ms": SENT_AT, "all": true, "library": { "likes": true, "playlists": ["app-404"] } }),
    );
    assert_eq!(status, 409);
    let (status, _) = client.post("/api/phone/take", json!({ "sent_at_ms": SENT_AT, "all": true, "library": "all" }));
    assert_eq!(status, 400);
    assert_eq!(client.get("/api/phone/library").0, 200, "still waiting");

    let chose = json!({ "likes": true, "plays": true, "playlists": ["app-2"], "only_here": false });
    let (status, taking) = client.post(
        "/api/phone/take",
        json!({ "sent_at_ms": SENT_AT, "all": true, "library": chose }),
    );
    assert_eq!((status, taking), (200, json!({ "taking": 2 })));
    let now = client.phone_until("done");
    assert_eq!(
        (now["taken"].as_u64(), now["cleared"].as_bool(), now["library_choice_kept"].as_bool()),
        (Some(2), Some(true), Some(true))
    );

    // The likes (Ivy a file, Halo to stream), the playlist chosen and the plays; the library and the list went from the
    // phone, and then the folder.
    let (_, liked) = client.get("/api/liked");
    let mut liked: Vec<(String, bool)> = liked["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["title"].as_str().unwrap().to_string(), r["here"].as_bool().unwrap()))
        .collect();
    liked.sort();
    assert_eq!(liked, [("Halo".to_string(), false), ("Ivy".to_string(), true)]);
    let (_, playlists) = client.get("/api/playlists");
    let playlists: Vec<(&str, u64)> = playlists["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["tracks"].as_u64().unwrap()))
        .collect();
    assert_eq!(playlists, [("Night drive", 2)]);
    assert_eq!(client.get("/api/history").1["rows"].as_array().unwrap().len(), 2);
    assert!(!folder.exists(), "the folder went once nothing was left in it");
    assert_eq!(client.get("/api/phone/library").0, 404, "nothing waiting now");
    let log = std::fs::read_to_string(dir.join("phone.log")).unwrap();
    assert!(log.contains("the phone's library came with the send") && log.contains("the phone's library came in"), "{log}");
}
