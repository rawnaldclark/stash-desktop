//! The library on other devices: switched on from Settings, a device added, and a Subsonic app's requests answered
//! on the network port (signing in with a token or the password, browsing, searching, playing with seeking, liking,
//! playlists), in JSON and XML; a device removed, or sharing switched off, is answered no more. The songs are made up
//! in the test.

use std::path::{Path, PathBuf};
use std::time::Duration;

use md5::{Digest, Md5};
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::library::{NewTrack, Origin};
use stashd::serve::{Core, Options};

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into()
}

fn song(dir: &Path, title: &str, n: u32, bytes: usize) -> NewTrack {
    let path = dir.join(format!("{n:02} {title}.flac"));
    std::fs::write(
        &path,
        (0..bytes).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
    )
    .unwrap();
    NewTrack {
        title: title.into(),
        artist: "The Band".into(),
        album_artist: None,
        album: Some("First Record".into()),
        year: Some(1999),
        disc_no: 1,
        track_no: n,
        duration_ms: 180_000,
        isrc: None,
        origin: Origin::File,
        addon_id: None,
        locator: path.to_string_lossy().into_owned(),
        codec: Some("flac".into()),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        added_at: 1_700_000_000 + n as i64,
        file_size: Some(bytes as u64),
        file_mtime: Some(1),
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Phone {
    agent: ureq::Agent,
    base: String,
    user: String,
    password: String,
}

impl Phone {
    /// A request as Subsonic apps send it: a fresh salt and the token of the password and salt.
    fn ask(&self, method: &str, extra: &str) -> Value {
        let salt = format!(
            "{:x}",
            std::process::id() as u64 * 7919 + method.len() as u64
        );
        let token: String = Md5::digest(format!("{}{salt}", self.password).as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let url = format!(
            "{}/rest/{method}.view?u={}&t={token}&s={salt}&v=1.16.1&c=test&f=json{extra}",
            self.base, self.user
        );
        let mut answer = self.agent.get(&url).call().unwrap();
        let value: Value = answer.body_mut().read_json().unwrap();
        value["subsonic-response"].clone()
    }

    fn ok(&self, method: &str, extra: &str) -> Value {
        let answer = self.ask(method, extra);
        assert_eq!(answer["status"], "ok", "{method}: {answer}");
        answer
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stash-lan-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("music")).unwrap();
    dir
}

#[test]
fn a_subsonic_app_on_another_device_plays_the_library() {
    let dir = scratch("app");
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        let music = dir.join("music");
        library
            .add_tracks([
                song(&music, "Opening", 1, 40_000),
                song(&music, "Second Song", 2, 30_000),
            ])
            .unwrap();
    }
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
    let local = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent = agent();
    let post = |path: &str, body: Value| -> Value {
        let mut answer = agent
            .post(&format!("{local}{path}"))
            .send_json(body)
            .unwrap();
        assert_eq!(answer.status().as_u16(), 200, "{path}");
        answer.body_mut().read_json().unwrap()
    };

    // Off until switched on.
    let status: Value = agent
        .get(&format!("{local}/api/lan"))
        .call()
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    assert_eq!(status["on"], false);
    assert_eq!(status["port"], 4747);
    let port = free_port();
    let status = post("/api/lan", json!({ "on": true, "port": port }));
    assert_eq!(status["listening"], port, "{status}");
    let low = agent
        .post(&format!("{local}/api/lan"))
        .send_json(json!({ "port": 80 }))
        .unwrap();
    assert_eq!(low.status().as_u16(), 400);

    let status = post("/api/lan/devices", json!({ "name": "My Phone" }));
    let device = &status["devices"][0];
    assert_eq!(device["user"], "myphone");
    let base = format!("http://127.0.0.1:{port}");
    let phone = Phone {
        agent: agent.clone(),
        base: base.clone(),
        user: "myphone".into(),
        password: device["password"].as_str().unwrap().into(),
    };

    // Signing in.
    let pong = phone.ok("ping", "");
    assert_eq!(pong["openSubsonic"], true);
    let wrong = Phone {
        password: "nope".into(),
        user: "myphone".into(),
        agent: agent.clone(),
        base: base.clone(),
    };
    let refused = wrong.ask("ping", "");
    assert_eq!(
        (
            refused["status"].as_str(),
            refused["error"]["code"].as_u64()
        ),
        (Some("failed"), Some(40))
    );
    let plain: Value = agent
        .get(&format!(
            "{base}/rest/ping?u=myphone&p={}&f=json",
            phone.password
        ))
        .call()
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    assert_eq!(plain["subsonic-response"]["status"], "ok");
    let keyed: Value = agent
        .get(&format!(
            "{base}/rest/ping?apiKey={}&f=json",
            phone.password
        ))
        .call()
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    assert_eq!(keyed["subsonic-response"]["status"], "ok");

    // Browsing.
    let artists = phone.ok("getArtists", "");
    let index = &artists["artists"]["index"][0];
    assert_eq!(
        (index["name"].as_str(), index["artist"][0]["name"].as_str()),
        (Some("B"), Some("The Band"))
    );
    let albums = phone.ok("getAlbumList2", "&type=alphabeticalByName&size=10");
    let album = &albums["albumList2"]["album"][0];
    assert_eq!(
        (
            album["name"].as_str(),
            album["songCount"].as_u64(),
            album["year"].as_u64()
        ),
        (Some("First Record"), Some(2), Some(1999))
    );
    let album_id = album["id"].as_str().unwrap().to_string();
    let full = phone.ok("getAlbum", &format!("&id={album_id}"));
    let songs = full["album"]["song"].as_array().unwrap().clone();
    assert_eq!(
        songs
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Opening", "Second Song"]
    );
    assert_eq!(
        (
            songs[0]["suffix"].as_str(),
            songs[0]["contentType"].as_str(),
            songs[0]["size"].as_u64()
        ),
        (Some("flac"), Some("audio/flac"), Some(40_000))
    );
    let first = songs[0]["id"].as_str().unwrap().to_string();
    let second = songs[1]["id"].as_str().unwrap().to_string();

    // Searching, and the whole library a page at a time.
    let found = phone.ok("search3", "&query=second");
    assert_eq!(
        found["searchResult3"]["song"][0]["id"].as_str(),
        Some(second.as_str())
    );
    let everything = phone.ok(
        "search3",
        "&query=&songCount=1&songOffset=1&albumCount=0&artistCount=0",
    );
    assert_eq!(
        everything["searchResult3"]["song"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Playing, from the start and from the middle.
    let stream = |range: Option<&str>| {
        let url = format!(
            "{base}/rest/stream?u=myphone&p={}&id={first}",
            phone.password
        );
        let mut asked = agent.get(&url);
        if let Some(range) = range {
            asked = asked.header("Range", range);
        }
        let mut answer = asked.call().unwrap();
        let status = answer.status().as_u16();
        let range = answer
            .headers()
            .get("content-range")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = answer
            .body_mut()
            .with_config()
            .limit(1 << 20)
            .read_to_vec()
            .unwrap();
        (status, range, bytes)
    };
    let (status, _, bytes) = stream(None);
    assert_eq!((status, bytes.len()), (200, 40_000));
    let (status, range, bytes) = stream(Some("bytes=1000-1999"));
    assert_eq!(
        (status, range.as_deref(), bytes.len()),
        (206, Some("bytes 1000-1999/40000"), 1000)
    );
    assert_eq!(bytes[0], (1000 % 251) as u8);

    // Liking, plays, playlists.
    phone.ok("star", &format!("&id={second}"));
    let starred = phone.ok("getStarred2", "");
    assert_eq!(
        starred["starred2"]["song"][0]["id"].as_str(),
        Some(second.as_str())
    );
    phone.ok("scrobble", &format!("&id={first}&submission=true"));
    let again = phone.ok("getSong", &format!("&id={first}"));
    assert_eq!(again["song"]["playCount"], 1);
    let made = phone.ok(
        "createPlaylist",
        &format!("&name=Trip&songId={second}&songId={first}"),
    );
    let list_id = made["playlist"]["id"].as_str().unwrap().to_string();
    assert_eq!(made["playlist"]["songCount"], 2);
    phone.ok(
        "updatePlaylist",
        &format!("&playlistId={list_id}&songIndexToRemove=0"),
    );
    let list = phone.ok("getPlaylist", &format!("&id={list_id}"));
    assert_eq!(
        list["playlist"]["entry"][0]["id"].as_str(),
        Some(first.as_str())
    );
    assert_eq!(
        phone.ok("getPlaylists", "")["playlists"]["playlist"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // What Stash does not do is said, not done.
    assert_eq!(phone.ask("createUser", "&username=x")["error"]["code"], 50);
    assert_eq!(phone.ask("getAlbum", "&id=al-999")["error"]["code"], 70);

    // XML, for the apps that ask for it.
    let xml = agent
        .get(&format!(
            "{base}/rest/getAlbum.view?u=myphone&p={}&id={album_id}",
            phone.password
        ))
        .call()
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    assert!(
        xml.contains("<subsonic-response ") && xml.contains(" status=\"ok\""),
        "{xml}"
    );
    // A song without a picture: no picture, plainly.
    let cover = agent
        .get(&format!(
            "{base}/rest/getCoverArt?u=myphone&p={}&id={first}",
            phone.password
        ))
        .call()
        .unwrap();
    assert_eq!(cover.status().as_u16(), 404);
    assert!(xml.contains("title=\"Second Song\""), "{xml}");

    // A removed device is answered no more; nor is anyone once sharing is off.
    let id = device["id"].as_str().unwrap();
    post(&format!("/api/lan/devices/{id}/remove"), json!({}));
    assert_eq!(phone.ask("ping", "")["error"]["code"], 40);
    let status = post("/api/lan", json!({ "on": false }));
    assert!(status["listening"].is_null(), "{status}");
    assert!(agent.get(&format!("{base}/rest/ping")).call().is_err());

    // What the listener set is kept.
    let library = Library::open(&db).unwrap();
    assert_eq!(library.setting("lan.on").unwrap().as_deref(), Some("0"));
    assert_eq!(library.setting("lan.port").unwrap(), Some(port.to_string()));
    drop(library);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_place_that_keeps_getting_the_password_wrong_is_shut_out() {
    let dir = scratch("tries");
    let db = dir.join("library.db");
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
    let local = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent = agent();
    let port = free_port();
    agent
        .post(&format!("{local}/api/lan"))
        .send_json(json!({ "on": true, "port": port }))
        .unwrap();
    let mut status: Value = agent
        .post(&format!("{local}/api/lan/devices"))
        .send_json(json!({ "name": "Tablet" }))
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    let password = status["devices"][0]["password"].take();
    let ask = |p: &str| -> Value {
        agent
            .get(&format!(
                "http://127.0.0.1:{port}/rest/ping?u=tablet&p={p}&f=json"
            ))
            .call()
            .unwrap()
            .body_mut()
            .read_json::<Value>()
            .unwrap()["subsonic-response"]
            .clone()
    };
    for _ in 0..10 {
        assert_eq!(ask("wrong")["error"]["code"], 40);
    }
    // Shut out, even with the right password.
    let shut = ask(password.as_str().unwrap());
    assert_eq!(shut["status"], "failed");
    assert!(
        shut["error"]["message"]
            .as_str()
            .unwrap()
            .contains("too many")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
