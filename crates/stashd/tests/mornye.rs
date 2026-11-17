//! Mornye extensions in the core: a package installed from a file is one of the listener's sources, searched like an
//! add-on, and its song is downloaded by the package itself and played through the relay. The package's service is a
//! small server of the test's own, on this computer (the package names it, and nothing else).

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

/// What the test's service was asked (paths).
type Asked = Arc<Mutex<Vec<String>>>;

/// The service: `/search` lists one song, "Quiet Tone" by Test Artist (two seconds); `/song.wav` is its sound.
fn service() -> (u16, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked: Asked = Arc::default();
    let noted = Arc::clone(&asked);
    std::thread::spawn(move || {
        let tone = wav(2);
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
            noted.lock().unwrap().push(path.clone());
            let (kind, body): (&str, Vec<u8>) = if path.starts_with("/search") {
                let found = json!([{ "id": "t1", "name": "Quiet Tone", "artists": [{ "name": "Test Artist" }],
                    "album_name": "Tones", "duration_ms": 2000, "isrc": "QZTST2600001" }]);
                ("application/json", found.to_string().into_bytes())
            } else if path == "/song.wav" {
                ("audio/wav", tone.clone())
            } else {
                ("text/plain", b"no".to_vec())
            };
            let status = if kind == "text/plain" {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let mut out = stream;
            let _ = out.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = out.write_all(&body);
        }
    });
    (port, asked)
}

/// A package that searches the service and downloads its song itself.
fn package(port: u16) -> Vec<u8> {
    let manifest = json!({
        "name": "test-tones",
        "displayName": "Test Tones",
        "version": "1.0.0",
        "description": "Tones for a test.",
        "type": ["metadata_provider", "download_provider"],
        "minAppVersion": "4.0.0",
        "permissions": { "network": ["127.0.0.1"], "storage": true, "file": true, "allowHttp": true },
        "qualityOptions": [{ "id": "LOSSLESS", "label": "Lossless", "kind": "lossless" }]
    });
    let code = format!(
        r#"const BASE = 'http://127.0.0.1:{port}';
registerExtension({{
  initialize: function (settings) {{ storage.set('ready', true); return true; }},
  searchTracks: function (query, limit) {{
    const r = http.get(BASE + '/search?q=' + encodeURIComponent(query));
    return {{ tracks: JSON.parse(r.body) }};
  }},
  getTrack: function (id) {{ return {{ id: id, name: 'Quiet Tone', artists: 'Test Artist', duration_ms: 2000 }}; }},
  download: function (id, quality, out, progress, options) {{
    const wav = out.replace(/\.flac$/, '.wav');
    let seen = 0;
    const r = file.download(BASE + '/song.wav', wav, {{ onProgress: function (got, total) {{ seen = got; }} }});
    if (!r.success) return {{ success: false, error_message: r.error }};
    progress(100);
    return {{ success: true, file_path: wav, quality: quality, seen: seen }};
  }}
}});
"#
    );
    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        zip.start_file("index.js", options).unwrap();
        zip.write_all(code.as_bytes()).unwrap();
        zip.finish().unwrap();
    }
    bytes
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

    fn post_bytes(&self, path: &str, bytes: &[u8]) -> (u16, Value) {
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
}

fn core(db: &Path, engine: Option<PathBuf>) -> Client {
    let core = Core::bind(Options {
        db: db.to_path_buf(),
        ui: None,
        port: 0,
        bench_out: None,
        audio: engine.as_ref().map(|_| Output::Silent),
        mpv: engine,
        play_counts_between_seconds: None,
    })
    .unwrap();
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    Client { agent, base }
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_mornye_extension_is_a_source_whose_songs_it_downloads_itself() {
    let dir = std::env::temp_dir().join(format!("stash-mornye-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, asked) = service();
    let engine = engine();
    let client = core(&dir.join("library.db"), engine.clone());

    // Nothing installed; Mornye's own repository is always there.
    let (status, told) = client.get("/api/mornye");
    assert_eq!(status, 200, "{told}");
    assert_eq!(told["installed"], json!([]));
    assert!(
        told["repositories"][0]["official"]
            .as_bool()
            .unwrap_or(false),
        "{told}"
    );

    // A file that is not a package is refused, and nothing is installed.
    let (status, refused) = client.post_bytes("/api/mornye/install-file", b"not a zip");
    assert_eq!(status, 400, "{refused}");

    // Installed from its file: one of the sources, built in, after the listener's add-ons.
    let (status, installed) = client.post_bytes("/api/mornye/install-file", &package(port));
    assert_eq!(status, 200, "{installed}");
    let key = installed["installed"]["key"].as_str().unwrap().to_string();
    assert_eq!(installed["installed"]["display_name"], "Test Tones");
    assert_eq!(installed["installed"]["playable"], true, "{installed}");
    let (_, listed) = client.get("/api/addons");
    let id = format!("mornye.{key}");
    let source = listed["addons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("not a source: {listed}"));
    assert_eq!(
        (source["name"].as_str(), source["builtIn"].as_bool()),
        (Some("Test Tones"), Some(true))
    );

    // Managed in its own part of Settings: not removed or checked as an add-on.
    let (status, _) = client.post("/api/addons/remove", json!({ "id": id }));
    assert_eq!(status, 400);

    // Searched like an add-on.
    let (status, found) = client.get(&format!("/api/addons/search?q=quiet%20tone&addon={id}"));
    assert_eq!(status, 200, "{found}");
    let track = &found["groups"][0]["tracks"][0];
    assert_eq!(
        (
            track["title"].as_str(),
            track["artist"].as_str(),
            track["duration_ms"].as_u64()
        ),
        (Some("Quiet Tone"), Some("Test Artist"), Some(2000)),
        "{found}"
    );

    // Switched off, it is no longer asked.
    let (status, _) = client.post("/api/mornye/enabled", json!({ "key": key, "on": false }));
    assert_eq!(status, 200);
    let (_, listed) = client.get("/api/addons");
    let off = listed["addons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == id.as_str())
        .cloned()
        .unwrap();
    assert_eq!(off["enabled"], false, "{listed}");
    client.post("/api/mornye/enabled", json!({ "key": key, "on": true }));

    // Played: the package downloads the song itself, and the engine reads the file through the relay.
    if engine.is_some() {
        client.post("/api/player/volume", json!({ "percent": 0 }));
        asked.lock().unwrap().clear();
        client.post(
            "/api/player/play",
            json!({ "addon": id, "tracks": [track.clone()], "from": "test" }),
        );
        let mut playing = Value::Null;
        until("the song playing from the extension", || {
            playing = client.get("/api/player").1;
            playing["status"] == "playing" || !playing["error"].is_null()
        });
        assert_eq!(
            (
                playing["status"].as_str(),
                playing["served_by"].as_str(),
                playing["track"]["title"].as_str()
            ),
            (Some("playing"), Some(id.as_str()), Some("Quiet Tone")),
            "{playing}"
        );
        // PCM is lossless (CD).
        assert_eq!(playing["tier"], "lossless", "{playing}");
        assert!(
            asked.lock().unwrap().iter().any(|p| p == "/song.wav"),
            "{:?}",
            asked.lock().unwrap()
        );
        client.post("/api/player/stop", json!({}));
    }

    // Removed: no longer a source.
    let (status, _) = client.post("/api/mornye/remove", json!({ "key": key }));
    assert_eq!(status, 200);
    let (_, listed) = client.get("/api/addons");
    assert!(
        listed["addons"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != id.as_str()),
        "{listed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
