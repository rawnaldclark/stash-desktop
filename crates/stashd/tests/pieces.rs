//! The engine reaches an add-on's server only through the relay: an HLS stream and its pieces, and a stream the add-on
//! calls plain that is a playlist all the same, are all fetched by the core (its own user agent), never by the
//! engine. The add-on is a small server of the test's own, which notes who asked it for what.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use stash_core::audio::Output;
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

type Asked = Arc<Mutex<Vec<(String, String)>>>;

/// The add-on: its manifest, a search, two streams, a playlist and its piece. Each request noted with its user agent.
fn addon() -> (u16, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked: Asked = Arc::default();
    let noted = Arc::clone(&asked);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let noted = Arc::clone(&noted);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    return;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                let mut agent = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("user-agent")
                    {
                        agent = value.trim().to_string();
                    }
                }
                noted.lock().unwrap().push((path.clone(), agent));
                let base = format!("http://127.0.0.1:{port}");
                let playlist = |piece: &str| {
                    format!(
                        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:2.0,\n{piece}\n#EXT-X-ENDLIST\n"
                    )
                };
                let (kind, body): (&str, Vec<u8>) = match path.split('?').next().unwrap_or("") {
                    "/manifest.json" => ("application/json", json!({ "stash": 1, "id": "pieces.test", "name": "Pieces", "version": "1.0.0", "kind": "server", "resources": ["search", "stream"] }).to_string().into_bytes()),
                    "/search" => ("application/json", json!({ "tracks": [
                        { "id": "hls", "title": "Pieces", "artist": "Test", "durationMs": 2000, "artwork": [{ "url": format!("{base}/cover.png") }] },
                        { "id": "sneaky", "title": "Sneaky", "artist": "Test", "durationMs": 2000 }
                    ] }).to_string().into_bytes()),
                    "/stream/hls" => ("application/json", json!({ "url": format!("{base}/hls/master.m3u8"), "manifest": "hls", "tier": "lossless", "codec": "pcm" }).to_string().into_bytes()),
                    "/stream/sneaky" => ("application/json", json!({ "url": format!("{base}/sneaky.flac"), "manifest": "none", "tier": "lossless", "codec": "pcm" }).to_string().into_bytes()),
                    "/hls/master.m3u8" => ("application/vnd.apple.mpegurl", playlist("seg.wav").into_bytes()),
                    // Called plain sound by its server, and a playlist all the same, naming its piece by its whole address.
                    "/sneaky.flac" => ("audio/flac", playlist(&format!("{base}/hls/other.wav")).into_bytes()),
                    "/hls/seg.wav" | "/hls/other.wav" => ("audio/wav", wav(2)),
                    "/cover.png" => ("image/png", [&[0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..], b" a picture"].concat()),
                    _ => ("text/plain", b"no".to_vec()),
                };
                let mut out = stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = out.write_all(&body);
            });
        }
    });
    (port, asked)
}

#[test]
fn the_engine_reaches_an_add_on_s_server_only_through_the_relay() {
    let Some(engine) = engine() else {
        eprintln!("SKIPPED pieces: no libmpv found");
        return;
    };
    let dir = std::env::temp_dir().join(format!("stash-pieces-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, asked) = addon();
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
    let post = |path: &str, body: Value| -> Value {
        agent
            .post(&format!("{base}{path}"))
            .send_json(body)
            .unwrap()
            .body_mut()
            .read_json()
            .unwrap_or(Value::Null)
    };
    post("/api/player/volume", json!({ "percent": 0 }));
    let added = post(
        "/api/addons",
        json!({ "link": format!("http://127.0.0.1:{port}/manifest.json") }),
    );
    assert_eq!(added["addons"][0]["id"], "pieces.test", "{added}");

    for (track, piece) in [("hls", "/hls/seg.wav"), ("sneaky", "/hls/other.wav")] {
        post(
            "/api/player/play",
            json!({ "addon": "pieces.test", "tracks": [{ "id": track, "title": track, "artist": "Test" }] }),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !asked.lock().unwrap().iter().any(|(path, _)| path == piece) {
            assert!(
                Instant::now() < deadline,
                "{track}: its piece was never asked for: {:?}",
                asked.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        post("/api/player/stop", json!({}));
    }
    let asked = asked.lock().unwrap().clone();
    for (path, agent) in &asked {
        assert!(
            agent.starts_with("ureq") || agent.starts_with("Stash-desktop"),
            "{path} was asked by {agent:?}, not the core: {asked:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cover_is_fetched_by_the_core_held_to_the_reach_of_the_add_on_that_named_it() {
    let dir = std::env::temp_dir().join(format!("stash-cover-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, asked) = addon();
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
    let cover = format!("http://127.0.0.1:{port}/cover.png");
    let route = format!(
        "{base}/api/art/remote?u={}",
        url::form_urlencoded::byte_serialize(cover.as_bytes()).collect::<String>()
    );
    let ask = || {
        let mut answer = agent.get(&route).call().unwrap();
        let kind = answer
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        (
            answer.status().as_u16(),
            kind,
            answer.body_mut().read_to_vec().unwrap(),
        )
    };

    // Named by no add-on: held to the internet, and this computer is not; nothing is asked of it.
    assert_eq!(ask().0, 404);
    assert!(!asked.lock().unwrap().iter().any(|(p, _)| p == "/cover.png"));

    // Named by an add-on on this computer, in its search: fetched, and given as the picture it is.
    agent
        .post(&format!("{base}/api/addons"))
        .send_json(json!({ "link": format!("http://127.0.0.1:{port}/manifest.json") }))
        .unwrap();
    let found: Value = agent
        .get(&format!("{base}/api/addons/search?q=pieces"))
        .call()
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
    assert_eq!(
        found["groups"][0]["tracks"][0]["art"].as_str(),
        Some(cover.as_str()),
        "{found}"
    );
    let (status, kind, bytes) = ask();
    assert_eq!((status, kind.as_deref()), (200, Some("image/png")));
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    let _ = std::fs::remove_dir_all(&dir);
}
