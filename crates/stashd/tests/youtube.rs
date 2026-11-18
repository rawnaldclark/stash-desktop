//! YouTube Music, built into the core: listed last among the listener's sources, searched like an add-on, and a
//! library song no add-on has is found on it and played through the relay. YouTube is a small server of the test's
//! own, which notes what it was asked.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::audio::Output;
use stash_core::reach::Reach;
use stash_core::youtube::YouTube;
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

fn wav(seconds: u32) -> Vec<u8> {
    let data = 44_100 * 4 * seconds;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&44_100u32.to_le_bytes());
    b.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    b.resize(44 + data as usize, 0);
    b
}

/// An answer of the real YouTube, as the core's own tests keep them.
fn recorded(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../stash-core/tests/fixtures/youtube")
        .join(name);
    std::fs::read(path).unwrap()
}

/// What the test's YouTube was asked: the path, and the visitor id sent with it.
type Asked = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// YouTube with one song: "Test Tone" by Stash Tester, two seconds long.
fn youtube() -> (String, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let asked: Asked = Arc::default();
    let (noted, here) = (Arc::clone(&asked), base.clone());
    std::thread::spawn(move || {
        let tone = wav(2);
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (mut length, mut visitor, mut cookie) = (0usize, None, String::new());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    match name.trim().to_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap_or(0),
                        "x-goog-visitor-id" => visitor = Some(value.trim().to_string()),
                        "cookie" => cookie = value.trim().to_string(),
                        _ => {}
                    }
                }
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            noted.lock().unwrap().push((path.clone(), visitor));
            let asked_for = |name: &str| body.get(name).and_then(Value::as_str).unwrap_or_default();
            // Signed in as the test's account: "good"; any other login is one YouTube does not take.
            let good = cookie.contains("SAPISID=good");
            let mut status = 200;
            let (kind, answer): (&str, Vec<u8>) = if path
                .starts_with("/youtubei/v1/account/accounts_list")
            {
                if !good {
                    status = 401;
                }
                let item = json!({ "accountName": { "simpleText": "Stash Tester" }, "channelHandle": { "simpleText": "@tester" }, "isSelected": true });
                (
                    "application/json",
                    json!({ "contents": [{ "accountItem": item }] })
                        .to_string()
                        .into_bytes(),
                )
            } else if path.starts_with("/youtubei/v1/browse")
                && asked_for("browseId") == "FEmusic_liked_playlists"
            {
                if !good {
                    status = 401;
                }
                let card = |id: &str, title: &str| {
                    json!({ "musicTwoRowItemRenderer": {
                    "title": { "runs": [{ "text": title }] },
                    "subtitle": { "runs": [{ "text": "Stash Tester" }] },
                    "navigationEndpoint": { "browseEndpoint": { "browseId": id, "browseEndpointContextSupportedConfigs": { "browseEndpointContextMusicConfig": { "pageType": "MUSIC_PAGE_TYPE_PLAYLIST" } } } },
                } })
                };
                let grid = json!({ "contents": { "singleColumnBrowseResultsRenderer": { "tabs": [{ "tabRenderer": { "content": { "sectionListRenderer": { "contents": [
                    { "gridRenderer": { "items": [card("VLLM", "Liked music"), card("VLPLmine", "Road trip")] } }
                ] } } } }] } } });
                ("application/json", grid.to_string().into_bytes())
            } else if path.starts_with("/youtubei/v1/browse")
                && asked_for("browseId") == "FEmusic_home"
            {
                ("application/json", recorded("home_live.json"))
            } else if path.starts_with("/youtubei/v1/browse") {
                // A page, or the songs after a playlist's first: the real YouTube's answers, cut down.
                let page = asked_for("browseId");
                let name = if !asked_for("continuation").is_empty() {
                    "playlist_more_live.json"
                } else if page.starts_with("MPREb_") {
                    "album_live.json"
                } else if page.starts_with("UC") {
                    "artist_live.json"
                } else {
                    "playlist_live.json"
                };
                ("application/json", recorded(name))
            } else if path.starts_with("/youtubei/v1/search") && body.get("params").is_none() {
                ("application/json", recorded("found_live.json"))
            } else if path.starts_with("/youtubei/v1/search") {
                let row = json!({ "musicResponsiveListItemRenderer": {
                    "playlistItemData": { "videoId": "testtone001" },
                    "flexColumns": [
                        { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Test Tone" }] } } },
                        { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                            { "text": "Stash Tester", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCtester" } } },
                            { "text": " • " },
                            { "text": "0:02" }
                        ] } } }
                    ]
                } });
                let found = json!({
                    "responseContext": { "visitorData": "TESTVISITOR" },
                    "contents": { "tabbedSearchResultsRenderer": { "tabs": [{ "tabRenderer": { "content": {
                        "sectionListRenderer": { "contents": [{ "musicShelfRenderer": { "contents": [row] } }] }
                    } } }] } }
                });
                ("application/json", found.to_string().into_bytes())
            } else if path.starts_with("/youtubei/v1/player") {
                let playable = json!({
                    "playabilityStatus": { "status": "OK" },
                    "streamingData": { "adaptiveFormats": [{
                        "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 141000,
                        "contentLength": tone.len().to_string(), "approxDurationMs": "2000",
                        "url": format!("{here}/file/tone")
                    }] }
                });
                ("application/json", playable.to_string().into_bytes())
            } else if path.starts_with("/file/tone") {
                ("audio/webm", tone.clone())
            } else {
                ("text/plain", b"no".to_vec())
            };
            let mut out = stream;
            let _ = write!(
                out,
                "HTTP/1.1 {status} OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                answer.len()
            );
            let _ = out.write_all(&answer);
        }
    });
    (base, asked)
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
}

fn core(db: &Path, api: &str, engine: Option<PathBuf>) -> Client {
    let core = Core::bind(Options {
        db: db.to_path_buf(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: engine.as_ref().map(|_| Output::Silent),
        mpv: engine,
        play_counts_between_seconds: None,
    })
    .unwrap()
    .with_youtube(YouTube::at(api, api), Reach::Here);
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    Client { agent, base }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn youtube_music_is_the_last_of_the_listener_s_sources() {
    let dir = std::env::temp_dir().join(format!("stash-youtube-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (base, asked) = youtube();
    let api = format!("{base}/youtubei/v1");
    let engine = engine();
    let client = core(&dir.join("library.db"), &api, engine.clone());

    // Listed with the add-ons, as part of Stash: no link, no host.
    let (_, listed) = client.get("/api/addons");
    let sources = listed["addons"].as_array().unwrap();
    assert_eq!(sources.len(), 1, "{listed}");
    let it = &sources[0];
    assert_eq!(
        (
            it["id"].as_str(),
            it["name"].as_str(),
            it["builtIn"].as_bool(),
            it["enabled"].as_bool()
        ),
        (
            Some("youtube"),
            Some("YouTube Music"),
            Some(true),
            Some(true)
        )
    );
    assert!(it["host"].is_null() && it["problem"].is_null(), "{it}");
    assert_eq!(it["resources"], json!(["search", "stream", "download"]));

    // It is not removed, checked for a new version or added over.
    for path in [
        "/api/addons/remove",
        "/api/addons/check",
        "/api/addons/update",
    ] {
        let (status, refused) = client.post(path, json!({ "id": "youtube" }));
        assert_eq!(status, 400, "{path}: {refused}");
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|e| e.contains("part of Stash")),
            "{refused}"
        );
    }
    assert_eq!(
        client.get("/api/addons").1["addons"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Searched like an add-on.
    let (status, found) = client.get("/api/addons/search?q=test%20tone&addon=youtube");
    assert_eq!(status, 200, "{found}");
    let track = &found["groups"][0]["tracks"][0];
    assert_eq!(
        (
            track["title"].as_str(),
            track["artist"].as_str(),
            track["duration_ms"].as_u64()
        ),
        (Some("Test Tone"), Some("Stash Tester"), Some(2000)),
        "{found}"
    );

    // The visitor id YouTube handed out is kept: a core started later on the same library is the same visitor.
    until("the visitor id kept", || {
        let later = core(&dir.join("library.db"), &api, None);
        asked.lock().unwrap().clear();
        later.get("/api/addons/search?q=again&addon=youtube");
        asked
            .lock()
            .unwrap()
            .first()
            .is_some_and(|(_, visitor)| visitor.as_deref() == Some("TESTVISITOR"))
    });

    // A library song known by its description, with no add-on to ask: found on YouTube and played from it.
    let file = json!({ "kind": "stash-web-library", "v": 1, "likes": [
        { "item": { "title": "Test Tone", "artist": "Stash Tester", "durationMs": 2000 }, "likedAt": 5 }
    ] })
    .to_string();
    let mut imported = client
        .agent
        .post(&format!("{}/api/import", client.base))
        .header("Content-Type", "application/octet-stream")
        .send(file.as_bytes())
        .unwrap();
    assert_eq!(
        imported.status().as_u16(),
        200,
        "{:?}",
        imported.body_mut().read_to_string()
    );
    if engine.is_some() {
        client.post("/api/player/volume", json!({ "percent": 0 }));
        asked.lock().unwrap().clear();
        client.post("/api/player/play", json!({ "liked": true }));
        let mut playing = Value::Null;
        until("the liked song playing from YouTube", || {
            playing = client.get("/api/player").1;
            playing["status"] == "playing" && playing["served_by"] == "youtube"
        });
        assert_eq!(
            (
                playing["track"]["title"].as_str(),
                playing["tier"].as_str(),
                playing["error"].is_null()
            ),
            (Some("Test Tone"), Some("low"), true),
            "{playing}"
        );
        // Searched for, asked where it plays from, its end asked for, and then read through the relay.
        let paths: Vec<String> = asked
            .lock()
            .unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert!(paths[0].starts_with("/youtubei/v1/search"), "{paths:?}");
        assert!(paths[1].starts_with("/youtubei/v1/player"), "{paths:?}");
        assert!(
            paths.iter().filter(|p| p.starts_with("/file/tone")).count() >= 2,
            "{paths:?}"
        );
        client.post("/api/player/stop", json!({}));

        // Its name on YouTube is kept (and a copy of what played, perhaps): played again, it is not searched for.
        asked.lock().unwrap().clear();
        client.post("/api/player/play", json!({ "liked": true }));
        until("the liked song playing again", || {
            let state = client.get("/api/player").1;
            state["status"] == "playing" && state["track"]["title"] == "Test Tone"
        });
        assert!(
            !asked
                .lock()
                .unwrap()
                .iter()
                .any(|(path, _)| path.starts_with("/youtubei/v1/search")),
            "{:?}",
            asked.lock().unwrap()
        );
        client.post("/api/player/stop", json!({}));
    } else {
        eprintln!("SKIPPED the player: no libmpv found");
    }

    // Its own pages: what a search finds beside the songs, an album, an artist, a playlist.
    let (status, found) = client.get("/api/youtube/search?q=daft%20punk");
    assert_eq!(status, 200, "{found}");
    assert_eq!(
        (
            found["top"]["kind"].as_str(),
            found["top"]["artist"]["name"].as_str()
        ),
        (Some("artist"), Some("Daft Punk"))
    );
    assert_eq!(found["albums"][0]["title"], "Random Access Memories");
    assert_eq!(
        (
            found["albums"].as_array().map(Vec::len),
            found["artists"].as_array().map(Vec::len),
            found["playlists"].as_array().map(Vec::len)
        ),
        (Some(3), Some(3), Some(5))
    );
    let (status, album) = client.get("/api/youtube/album?id=MPREb_DYSzDJvUSDu");
    assert_eq!(status, 200, "{album}");
    assert_eq!(
        (
            album["artist"].as_str(),
            album["single"].as_bool(),
            album["songs"].as_array().map(Vec::len)
        ),
        (Some("Daft Punk"), Some(true), Some(4))
    );
    assert_eq!(album["songs"][1]["album_id"], "MPREb_DYSzDJvUSDu");
    let (status, artist) = client.get("/api/youtube/artist?id=UCRr1xG_2WIDs18a6cIiCxeA");
    assert_eq!(status, 200, "{artist}");
    assert_eq!(
        (
            artist["name"].as_str(),
            artist["songs"].as_array().map(Vec::len),
            artist["albums"].as_array().map(Vec::len),
            artist["singles"].as_array().map(Vec::len),
            artist["related"].as_array().map(Vec::len)
        ),
        (Some("Daft Punk"), Some(5), Some(3), Some(3), Some(3))
    );
    // A playlist is read on until an answer brings nothing new (this one's "songs after these" are the same again).
    asked.lock().unwrap().clear();
    let (status, playlist) =
        client.get("/api/youtube/playlist?id=RDCLAK5uy_l607eBFIYRZ_itkDW19eigUzFefu5bJ6Y");
    assert_eq!(status, 200, "{playlist}");
    assert_eq!(
        (
            playlist["author"].as_str(),
            playlist["songs"].as_array().map(Vec::len),
            playlist["more"].as_bool()
        ),
        (Some("YouTube Music"), Some(5), Some(false))
    );
    assert_eq!(asked.lock().unwrap().len(), 2);
    // What is no page of its kind is never asked of YouTube.
    asked.lock().unwrap().clear();
    for path in [
        "/api/youtube/album?id=UCabc",
        "/api/youtube/artist?id=../x",
        "/api/youtube/playlist?id=MPREb_x",
        "/api/youtube/album",
    ] {
        assert_eq!(client.get(path).0, 404, "{path}");
    }
    assert!(asked.lock().unwrap().is_empty());

    // Switched off, it is asked nothing; and it stays off for the next start.
    let (status, off) = client.post(
        "/api/addons/enabled",
        json!({ "id": "youtube", "enabled": false }),
    );
    assert_eq!(
        (status, off["addons"][0]["enabled"].as_bool()),
        (200, Some(false)),
        "{off}"
    );
    asked.lock().unwrap().clear();
    let (_, nothing) = client.get("/api/addons/search?q=test%20tone&addon=youtube");
    assert!(
        nothing["groups"].as_array().is_none_or(|g| g.is_empty()),
        "{nothing}"
    );
    assert_eq!(client.get("/api/youtube/search?q=daft%20punk").0, 404);
    assert!(asked.lock().unwrap().is_empty());
    let later = core(&dir.join("library.db"), &api, None);
    assert_eq!(
        later.get("/api/addons").1["addons"][0]["enabled"].as_bool(),
        Some(false)
    );
    let (_, on) = client.post(
        "/api/addons/enabled",
        json!({ "id": "youtube", "enabled": true }),
    );
    assert_eq!(on["addons"][0]["enabled"].as_bool(), Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn signing_in_to_youtube_brings_in_the_listener_s_library() {
    let dir = std::env::temp_dir().join(format!("stash-youtube-account-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("library.db");
    let (base, _asked) = youtube();
    let api = format!("{base}/youtubei/v1");
    let client = core(&db, &api, None);

    // Nobody yet; this core has no window to sign in with, so cookies are the way.
    let (status, account) = client.get("/api/youtube/account");
    assert_eq!(status, 200);
    assert_eq!(account["signed_in"], false, "{account}");
    assert_eq!(account["window"], false);
    assert_eq!(client.post("/api/youtube/account/window", json!({})).0, 404);
    let (status, _) = client.get("/api/youtube/library?kind=playlists");
    assert_eq!(status, 401);

    // Cookies without SAPISID are no sign-in; a login YouTube does not take is not kept.
    let (status, said) = client.post(
        "/api/youtube/account",
        json!({ "cookies": "HSID=x; SID=y" }),
    );
    assert_eq!(status, 400, "{said}");
    let (status, said) = client.post("/api/youtube/account", json!({ "cookies": "SAPISID=bad" }));
    assert_eq!(status, 401, "{said}");
    assert_eq!(client.get("/api/youtube/account").1["signed_in"], false);

    // A good one: YouTube says whose it is, and the window is told that, never the cookies.
    let (status, account) = client.post(
        "/api/youtube/account",
        json!({ "cookies": "SAPISID=good; LOGIN_INFO=l; SID=s" }),
    );
    assert_eq!(status, 200, "{account}");
    assert_eq!(account["signed_in"], true);
    assert_eq!(account["spent"], false);
    assert_eq!(account["account"]["name"], "Stash Tester");
    assert_eq!(account["account"]["handle"], "@tester");
    assert!(!account.to_string().contains("SAPISID"), "{account}");

    // Their library, and the home page.
    let (status, found) = client.get("/api/youtube/library?kind=playlists");
    assert_eq!(status, 200, "{found}");
    let titles: Vec<_> = found["cards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["playlist"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Liked music", "Road trip"]);
    assert_eq!(found["cards"][0]["kind"], "playlist");
    assert_eq!(client.get("/api/youtube/library?kind=everything").0, 400);
    let (status, home) = client.get("/api/youtube/home");
    assert_eq!(status, 200, "{home}");
    assert_eq!(home["shelves"][0]["title"], "Pump it up");
    assert_eq!(home["shelves"][0]["cards"][0]["kind"], "playlist");

    // Kept for the next start.
    let again = core(&db, &api, None);
    let (_, account) = again.get("/api/youtube/account");
    assert_eq!(account["signed_in"], true, "{account}");
    assert_eq!(account["account"]["name"], "Stash Tester");

    // Signed out: gone here, and at the next start.
    let (status, account) = again.post("/api/youtube/account/out", json!({}));
    assert_eq!(status, 200);
    assert_eq!(account["signed_in"], false);
    assert_eq!(again.get("/api/youtube/library?kind=albums").0, 401);
    let third = core(&db, &api, None);
    assert_eq!(third.get("/api/youtube/account").1["signed_in"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn yt_dlp_is_found_beside_the_library_and_switched_by_the_listener() {
    let dir = std::env::temp_dir().join(format!("stash-youtube-ytdlp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    let (base, _asked) = youtube();
    let api = format!("{base}/youtubei/v1");

    // Stash's own copy is the one in the tools folder beside the library (this one does not run: no version).
    let name = if cfg!(windows) {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    std::fs::write(dir.join("tools").join(name), b"not a program").unwrap();
    let client = core(&dir.join("library.db"), &api, None);
    let (status, said) = client.get("/api/youtube/ytdlp");
    assert_eq!(status, 200, "{said}");
    assert_eq!(said["on"], true);
    assert_eq!(said["found"]["own"], true, "{said}");
    assert_eq!(said["found"]["version"], Value::Null);
    assert_eq!(said["fetching"], Value::Null);

    // Switched off, and kept so.
    let (status, said) = client.post("/api/youtube/ytdlp", json!({ "on": false }));
    assert_eq!((status, &said["on"]), (200, &json!(false)), "{said}");
    let again = core(&dir.join("library.db"), &api, None);
    assert_eq!(again.get("/api/youtube/ytdlp").1["on"], false);
    let _ = std::fs::remove_dir_all(&dir);
}
