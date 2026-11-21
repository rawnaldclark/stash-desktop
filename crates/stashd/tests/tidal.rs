//! Stash's own Tidal, from the outside: a stand-in for Stash's servers on this computer serves a signed config that
//! names a stand-in Tidal add-on, which keeps the rules the real one keeps. It counts each song played per install
//! (a free allowance of two a day here), checks Stash's signature on every song and on the allowance, takes a
//! supporter's token as unlimited, and its clock runs ahead of this computer's (Stash signs again at its time). No test
//! asks Stash's real servers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::audio::Output;
use stashd::serve::{Core, Options, VerifyShell, VerifyWindow};

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

/// Three seconds of silence, 16-bit stereo at 44.1 kHz.
fn wav() -> Vec<u8> {
    let data: u32 = 44_100 * 4 * 3;
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
    bytes
}

const RELAY_KEY: &str = "test-relay-key";
/// Stash's access token, with a character a path must escape.
const TOKEN: &str = "tok en";
const SUPPORTER: &str = "good-supporter-token";
/// The free allowance a day, here.
const FREE: u64 = 2;
/// How far ahead of this computer the stand-in's clock is.
const AHEAD: i64 = 1_000;
const SONGS: &[&str] = &["Alpha", "Bravo", "Charlie", "Delta"];
/// The pass the stand-in's check gives, and the token its page hands back when it is passed.
const PASS_GIVEN: &str = "eyJ2IjoxfQ.cGFzcy1naXZlbi1ieS10aGUtY2hlY2s";
const SOLVED: &str = "0.solved_turnstile-token";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn hmac(key: &str, message: &str) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    ring::hmac::sign(&key, message.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the stand-in saw.
#[derive(Default)]
struct Seen {
    /// Songs answered, by install.
    counted: HashMap<String, u64>,
    /// Each stream request: (install, signed right, the supporter's token sent).
    streams: Vec<(String, bool, bool)>,
    /// Requests refused for the clock, then signed again right.
    skewed: usize,
    quota_asked: usize,
    agents: Vec<String>,
    paths: Vec<String>,
    /// What was posted to the check's /register.
    registered: Vec<Value>,
}

/// Stash's servers, standing in: the config and its signature, and the Tidal add-on it names.
struct Servers {
    base: String,
    public_key: String,
    seen: Arc<Mutex<Seen>>,
    /// The add-on refuses the pass it gave (as when it is withdrawn).
    refuse_pass: Arc<std::sync::atomic::AtomicBool>,
}

impl Servers {
    /// `tamper`: the config's signature is not its key's.
    fn start(tamper: bool) -> Self {
        Self::start_with(tamper, false)
    }

    /// `check`: the add-on offers its check (`/register`), and takes the pass it gives in its link.
    fn start_with(tamper: bool, check: bool) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(pair.public_key().as_ref());
        let config = json!({
            "v": 1, "updated_at": 10, "relays": [], "relay_key": RELAY_KEY,
            "tidal_relay_url": base, "tidal_relay_token": TOKEN,
        })
        .to_string();
        let signed = if tamper {
            format!("{config} ")
        } else {
            config.clone()
        };
        let signature = STANDARD.encode(pair.sign(&rng, signed.as_bytes()).unwrap().as_ref());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let kept = Arc::clone(&seen);
        let refuse_pass = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let refusing = Arc::clone(&refuse_pass);
        let audio = wav();
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut posted = String::new();
                if *request.method() == tiny_http::Method::Post {
                    let _ = std::io::Read::read_to_string(request.as_reader(), &mut posted);
                }
                let get = *request.method() == tiny_http::Method::Get;
                let header = |name: &'static str| {
                    request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv(name))
                        .map(|h| h.value.as_str().to_string())
                };
                let url = url::Url::parse(&format!("http://local{}", request.url())).unwrap();
                let path = url.path().to_string();
                let query: HashMap<String, String> = url
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                {
                    let mut seen = kept.lock().unwrap();
                    seen.agents.push(header("User-Agent").unwrap_or_default());
                    seen.paths.push(path.clone());
                }
                let answer = |status: u16, body: Value, extra: Vec<(&str, String)>| {
                    let mut response =
                        tiny_http::Response::from_string(body.to_string()).with_status_code(status);
                    for (name, value) in extra {
                        response.add_header(tiny_http::Header::from_bytes(name, value).unwrap());
                    }
                    response
                };
                // The add-on's own clock, and whether a request is signed right over `what` by it.
                let server_now = now() + AHEAD;
                // A request in the pass's link: the pass names the install, nothing is signed.
                let v1_pass = format!("/v1/{PASS_GIVEN}/");
                let by_pass = check && path.starts_with(&v1_pass);
                let signed = |what: &str| -> Result<String, &'static str> {
                    if by_pass {
                        return Ok("pass".to_string());
                    }
                    let (Some(install), Some(ts), Some(auth)) = (
                        header("X-Stash-Install"),
                        header("X-Stash-Ts"),
                        header("X-Stash-Auth"),
                    ) else {
                        return Err("missing");
                    };
                    let ts: i64 = ts.parse().map_err(|_| "missing")?;
                    if (server_now - ts).abs() > 300 {
                        return Err("skew");
                    }
                    if hmac(RELAY_KEY, &format!("{install}:{what}:{ts}")) != auth {
                        return Err("wrong");
                    }
                    Ok(install)
                };
                let reset = (server_now / 86_400 + 1) * 86_400;
                let supporter = header("X-Stash-Entitlement").as_deref() == Some(SUPPORTER);
                let limit = if supporter { Value::Null } else { json!(FREE) };
                let refused = |why: &str| match why {
                    "skew" => answer(
                        401,
                        json!({ "error": { "code": "unauthorized", "message": "clock" } }),
                        vec![("X-Stash-Server-Time", server_now.to_string())],
                    ),
                    _ => answer(
                        400,
                        json!({ "error": { "code": "bad_request", "message": "signature" } }),
                        vec![],
                    ),
                };
                let v1 = if by_pass {
                    v1_pass.clone()
                } else {
                    format!("/v1/{}/", TOKEN.replace(' ', "%20"))
                };
                let track = |title: &str, i: usize| json!({ "id": format!("td:{}", i + 1), "title": title, "artist": "Tester", "durationMs": 3000, "maxTier": "lossless" });
                let response = match path.as_str() {
                    "/lossless.json" => tiny_http::Response::from_string(config.clone()),
                    "/lossless.json.sig" => tiny_http::Response::from_string(signature.clone()),
                    p if p.starts_with("/audio/") => tiny_http::Response::from_data(audio.clone()),
                    // Another add-on, calling itself by Stash's Tidal's name.
                    "/impostor/manifest.json" => answer(
                        200,
                        json!({
                            "stash": 1, "id": "stash.tidal", "name": "Tidal", "version": "1.0.0", "kind": "server",
                            "resources": ["search", "stream"],
                            "stream": { "cors": false, "manifests": ["none"], "tiers": ["lossless"] }
                        }),
                        vec![],
                    ),
                    "/register" if check && get => answer(200, json!("the check page"), vec![]),
                    "/register" if check => {
                        kept.lock()
                            .unwrap()
                            .registered
                            .push(serde_json::from_str(&posted).unwrap_or(Value::Null));
                        answer(
                            200,
                            json!({ "pass": PASS_GIVEN, "install": "picked-by-the-server" }),
                            vec![],
                        )
                    }
                    _ if by_pass && refusing.load(std::sync::atomic::Ordering::Relaxed) => answer(
                        401,
                        json!({ "error": { "code": "unauthorized", "message": "Missing or wrong token" } }),
                        vec![],
                    ),
                    p if !p.starts_with(&v1) => answer(
                        404,
                        json!({ "error": { "code": "not_found", "message": "" } }),
                        vec![],
                    ),
                    p => match &p[v1.len()..] {
                        "manifest.json" => answer(
                            200,
                            json!({
                                "stash": 1, "id": "tidal", "name": "Tidal", "version": "2.0.0", "kind": "server",
                                "description": "Lossless and Hi-Res from Tidal",
                                "resources": ["search", "resolve", "stream", "download", "quota"],
                                "stream": { "cors": false, "manifests": ["none", "dash"], "tiers": ["high", "lossless", "hires"], "headers": false },
                                "auth": { "token": "path" }
                            }),
                            vec![],
                        ),
                        "search" => answer(
                            200,
                            json!({ "tracks": SONGS.iter().enumerate().map(|(i, t)| track(t, i)).collect::<Vec<_>>() }),
                            vec![],
                        ),
                        "resolve" => {
                            let wanted = query.get("title").cloned().unwrap_or_default();
                            let item = SONGS
                                .iter()
                                .position(|t| t.eq_ignore_ascii_case(&wanted))
                                .map(|i| track(SONGS[i], i));
                            answer(200, json!({ "item": item }), vec![])
                        }
                        "quota" => match signed("quota") {
                            Err(why) => {
                                if why == "skew" {
                                    kept.lock().unwrap().skewed += 1;
                                }
                                refused(why)
                            }
                            Ok(install) => {
                                let mut seen = kept.lock().unwrap();
                                seen.quota_asked += 1;
                                let used = seen.counted.get(&install).copied().unwrap_or(0);
                                answer(
                                    200,
                                    json!({ "used": used, "limit": limit, "resetsAt": reset, "label": "Tidal plays" }),
                                    vec![],
                                )
                            }
                        },
                        rest => {
                            let id = rest
                                .trim_start_matches("stream/")
                                .trim_start_matches("download/")
                                .to_string();
                            match signed(&id) {
                                Err(why) => {
                                    let mut seen = kept.lock().unwrap();
                                    if why == "skew" {
                                        seen.skewed += 1;
                                    } else {
                                        seen.streams.push((
                                            header("X-Stash-Install").unwrap_or_default(),
                                            false,
                                            supporter,
                                        ));
                                    }
                                    refused(why)
                                }
                                Ok(install) => {
                                    let mut seen = kept.lock().unwrap();
                                    seen.streams.push((install.clone(), true, supporter));
                                    let used = seen.counted.get(&install).copied().unwrap_or(0);
                                    if !supporter && used >= FREE {
                                        let quota = json!({ "used": used, "limit": FREE, "resetsAt": reset, "label": "Tidal plays" });
                                        answer(
                                            429,
                                            json!({ "error": { "code": "quota_exceeded", "message": "Today's Tidal plays are used up", "retryAfter": reset - server_now, "quota": quota } }),
                                            vec![("Retry-After", (reset - server_now).to_string())],
                                        )
                                    } else {
                                        let used = used + 1;
                                        seen.counted.insert(install, used);
                                        let n = id.trim_start_matches("td:");
                                        answer(
                                            200,
                                            json!({
                                                "url": format!("{base_url}/audio/{n}.wav", base_url = format!("http://127.0.0.1:{port}")),
                                                "manifest": "none", "tier": "lossless", "codec": "pcm", "container": "wav",
                                                "bitDepth": 16, "sampleRate": 44_100, "cors": false,
                                                "quota": { "used": used, "limit": limit, "resetsAt": reset, "label": "Tidal plays" }
                                            }),
                                            vec![],
                                        )
                                    }
                                }
                            }
                        }
                    },
                };
                let _ = request.respond(response);
            }
        });
        Servers {
            base,
            public_key: STANDARD.encode(spki),
            seen,
            refuse_pass,
        }
    }

    fn config_url(&self) -> String {
        format!("{}/lossless.json", self.base)
    }

    /// Stream requests signed right, and of those, how many carried the supporter's token.
    fn streams(&self) -> (usize, usize) {
        let seen = self.seen.lock().unwrap();
        let right: Vec<_> = seen.streams.iter().filter(|(_, ok, _)| *ok).collect();
        (right.len(), right.iter().filter(|(.., s)| *s).count())
    }
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

    fn text(&self, path: &str) -> String {
        let mut response = self
            .agent
            .get(&format!("{}{path}", self.base))
            .call()
            .unwrap();
        response.body_mut().read_to_string().unwrap_or_default()
    }

    fn post(&self, path: &str, body: Value) -> Value {
        let (status, answer) = self.post_any(path, body);
        assert_eq!(status, 200, "{path}: {answer}");
        answer
    }

    fn post_any(&self, path: &str, body: Value) -> (u16, Value) {
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

    /// Stash's Tidal among the listener's add-ons, if it is.
    fn listed(&self) -> Option<Value> {
        self.get("/api/addons").1["addons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == "stash.tidal")
            .cloned()
    }

    fn upload(&self, path: &str, bytes: &[u8]) -> u16 {
        self.agent
            .post(&format!("{}{path}", self.base))
            .header("Content-Type", "application/octet-stream")
            .send(bytes)
            .unwrap()
            .status()
            .as_u16()
    }

    /// Asks `path` until its answer satisfies `wanted`.
    fn until(&self, path: &str, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (_, answer) = self.get(path);
            if wanted(&answer) {
                return answer;
            }
            assert!(
                Instant::now() < deadline,
                "waited 20 s for: {what}; {path} says {answer}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A core of its own, on a new library (with `supporter`'s token kept, as a claim keeps it), reading the stand-in's
/// config when `servers` is given; with a silent player when `engine` is.
fn core(
    name: &str,
    servers: Option<&Servers>,
    supporter: Option<&str>,
    engine: Option<&Path>,
) -> (Client, PathBuf) {
    core_with(name, servers, supporter, engine, None)
}

/// [`core`], with `shell` opening the windows (the add-on's check) as the desktop app's shell does.
fn core_with(
    name: &str,
    servers: Option<&Servers>,
    supporter: Option<&str>,
    engine: Option<&Path>,
    shell: Option<VerifyShell>,
) -> (Client, PathBuf) {
    let dir = std::env::temp_dir().join(format!("stash-tidal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("library.db");
    {
        let mut library = Library::open(&db).unwrap();
        // Autoplay off: it would add songs after the ones a test plays.
        library.set_setting("autoplay", "0").unwrap();
        if let Some(token) = supporter {
            library.set_setting("supporter.token", token).unwrap();
        }
    }
    let core = Core::bind(Options {
        db,
        ui: None,
        port: 0,
        bench_out: None,
        audio: engine.map(|_| Output::Silent),
        mpv: engine.map(Path::to_path_buf),
        play_counts_between_seconds: None,
    })
    .unwrap();
    let core = match servers {
        Some(s) => core.with_stash_config(&s.config_url(), &s.public_key),
        None => core,
    };
    let core = match shell {
        Some(shell) => core.with_verify(shell),
        None => core,
    };
    let base = format!("http://127.0.0.1:{}", core.port());
    std::thread::spawn(move || core.run());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    (Client { agent, base }, dir)
}

/// The library's liked songs: the first `n` of the stand-in's.
fn like(client: &Client, n: usize) {
    let likes: Vec<Value> = SONGS
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, t)| json!({ "item": { "title": t, "artist": "Tester", "durationMs": 3000 }, "likedAt": 1_759_838_400_000i64 + i as i64 }))
        .collect();
    let file =
        json!({ "kind": "stash-web-library", "v": 1, "generator": "a test", "likes": likes })
            .to_string();
    assert_eq!(client.upload("/api/import", file.as_bytes()), 200);
}

/// Adds Stash's Tidal, as Settings' Add button does, and waits for its allowance.
fn add(client: &Client) -> Value {
    let added = client.post("/api/tidal/add", json!({}));
    assert!(
        added["addons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == "stash.tidal"),
        "{added}"
    );
    client.until(
        "/api/tidal",
        "Stash's Tidal ready, its allowance known",
        |t| t["ready"] == true && t["allowance"].is_object(),
    )
}

#[test]
fn stash_s_tidal_is_added_by_the_listener_and_signed_songs_count_against_the_free_allowance() {
    let Some(engine) = engine() else {
        eprintln!("skipped: no libmpv");
        return;
    };
    let servers = Servers::start(false);
    let (client, dir) = core("free", Some(&servers), None, Some(&engine));

    // Offered, not there: until the listener adds it, nothing is asked of Stash's servers.
    let tidal = client.get("/api/tidal").1;
    assert_eq!(
        (tidal["available"].clone(), tidal["added"].clone()),
        (json!(true), json!(false)),
        "{tidal}"
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert!(client.listed().is_none());
    assert!(
        servers.seen.lock().unwrap().paths.is_empty(),
        "Stash's servers were asked before it was added"
    );

    // Added: found through the config, one of the listener's add-ons, never its link or Stash's token.
    let tidal = add(&client);
    assert_eq!(
        (
            tidal["added"].clone(),
            tidal["on"].clone(),
            tidal["supporter"].clone(),
            tidal["allowance"]["limit"].clone(),
            tidal["allowance"]["used"].clone(),
        ),
        (
            json!(true),
            json!(true),
            json!(false),
            json!(FREE),
            json!(0)
        ),
        "{tidal}"
    );
    let listed = client.text("/api/addons");
    let row = client.listed().unwrap();
    assert_eq!(
        (
            row["name"].clone(),
            row["builtIn"].clone(),
            row["byStash"].clone(),
            row["host"].clone()
        ),
        (json!("Tidal"), json!(false), json!(true), Value::Null),
        "{row}"
    );
    for secret in [TOKEN, "tok%20en", RELAY_KEY, servers.base.as_str()] {
        assert!(
            !listed.contains(secret) && !tidal.to_string().contains(secret),
            "{secret} is shown"
        );
    }
    // It names itself as Stash. Stash keeps it up to date: there is no new version to check for.
    let agents = servers.seen.lock().unwrap().agents.clone();
    assert!(agents.iter().all(|a| a.starts_with("Stash/")), "{agents:?}");
    let (status, _) = client.post_any("/api/addons/check", json!({ "id": "stash.tidal" }));
    assert_eq!(status, 400);

    // Liked songs play from it, each one signed (at its own clock, once it said the clocks differ) and counted.
    like(&client, 3);
    client.post("/api/player/play", json!({ "liked": true }));
    let playing = client.until(
        "/api/player",
        "a liked song playing from Stash's Tidal",
        |s| s["status"] == "playing",
    );
    assert_eq!(playing["tier"], "lossless", "{playing}");
    // Two songs is the free allowance here: the third is not asked of Stash's Tidal at all.
    let tidal = client.until("/api/tidal", "the allowance spent", |t| {
        t["allowance"]["used"] == json!(FREE)
    });
    let resets = tidal["allowance"]["resets_at_ms"].as_i64().unwrap();
    assert!(
        (resets / 1000 - (((now() + AHEAD) / 86_400 + 1) * 86_400 - AHEAD)).abs() <= 2,
        "the reset on this computer's clock: {tidal}"
    );
    client.until(
        "/api/player",
        "the queue to end on the song no source could play",
        |s| s["resting"] == true,
    );
    std::thread::sleep(Duration::from_millis(500));
    let (right, _) = servers.streams();
    let seen = servers.seen.lock().unwrap();
    assert_eq!(right as u64, FREE, "songs asked for: {:?}", seen.streams);
    assert!(
        seen.streams.iter().all(|(_, ok, _)| *ok),
        "every song request is signed right: {:?}",
        seen.streams
    );
    assert!(seen.skewed >= 1, "the first request met the add-on's clock");
    assert_eq!(seen.counted.len(), 1, "one install");
    drop(seen);

    // Switched off and removed as any add-on is.
    client.post(
        "/api/addons/enabled",
        json!({ "id": "stash.tidal", "enabled": false }),
    );
    assert_eq!(client.get("/api/tidal").1["on"], false);
    client.post("/api/addons/remove", json!({ "id": "stash.tidal" }));
    assert!(client.listed().is_none());
    assert_eq!(client.get("/api/tidal").1["added"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_supporter_plays_without_a_limit() {
    let Some(engine) = engine() else {
        eprintln!("skipped: no libmpv");
        return;
    };
    let servers = Servers::start(false);
    let (client, dir) = core("supporter", Some(&servers), Some(SUPPORTER), Some(&engine));
    let tidal = add(&client);
    assert_eq!(
        (
            tidal["supporter"].clone(),
            tidal["allowance"]["limit"].clone()
        ),
        (json!(true), Value::Null),
        "{tidal}"
    );
    like(&client, 4);
    client.post("/api/player/play", json!({ "liked": true }));
    client.until("/api/tidal", "four songs played", |t| {
        t["allowance"]["used"] == json!(4)
    });
    let (right, entitled) = servers.streams();
    assert!(
        right >= 4 && entitled == right,
        "every song carried the supporter's token: {right} {entitled}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_config_signed_by_stash_there_is_no_stash_tidal() {
    let servers = Servers::start(true);
    // A build that names no config.
    let (client, dir) = core("none", None, None, None);
    let tidal = client.get("/api/tidal").1;
    assert_eq!(
        (tidal["available"].clone(), tidal["ready"].clone()),
        (json!(false), json!(false)),
        "{tidal}"
    );
    assert_eq!(client.post_any("/api/tidal/add", json!({})).0, 404);
    assert!(client.listed().is_none());
    // An add-on that calls itself by Stash's Tidal's name is not added by its link.
    let (status, refused) = client.post_any(
        "/api/addons",
        json!({ "link": format!("{}/impostor/manifest.json", servers.base) }),
    );
    assert_eq!(status, 400, "{refused}");
    assert!(client.listed().is_none());
    servers.seen.lock().unwrap().paths.clear();
    let _ = std::fs::remove_dir_all(&dir);

    // A config whose signature is not Stash's key's is not believed: Tidal is not added.
    let (client, dir) = core("tampered", Some(&servers), None, None);
    let (status, refused) = client.post_any("/api/tidal/add", json!({}));
    assert_eq!(status, 502, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not signed right"),
        "{refused}"
    );
    assert!(client.listed().is_none());
    assert!(
        !servers
            .seen
            .lock()
            .unwrap()
            .paths
            .iter()
            .any(|p| p.starts_with("/v1/")),
        "the add-on it names was never asked"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A shell that "passes" the add-on's check at once: it notes the page it was asked to open, and hands back the page's
/// solved token, as the desktop app's window does when the listener passes it.
fn passing_shell(opened: Arc<Mutex<Vec<String>>>) -> VerifyShell {
    Arc::new(move |window: VerifyWindow| {
        opened.lock().unwrap().push(window.url.clone());
        let done = format!("{}/done#turnstile={SOLVED}", window.url);
        assert!((window.catches)(&done));
        assert!(!(window.catches)(&window.url), "the page itself loads");
        assert!(!(window.catches)(
            "https://challenges.cloudflare.com/register/done#turnstile=x"
        ));
        std::thread::spawn(move || {
            (window.caught)(done);
            (window.closed)();
        });
        true
    })
}

#[test]
fn where_the_add_on_offers_its_check_add_opens_it_and_the_pass_takes_the_token_s_place() {
    let servers = Servers::start_with(false, true);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let (client, dir) = core_with(
        "pass",
        Some(&servers),
        None,
        None,
        Some(passing_shell(Arc::clone(&opened))),
    );

    // Add opens the check; once it is passed, Stash's Tidal is added, and asked in the pass's link.
    client.post("/api/tidal/add", json!({}));
    let tidal = client.until("/api/tidal", "added with this computer's pass", |t| {
        t["added"] == true
            && t["ready"] == true
            && t["allowance"].is_object()
            && t["checking"] == false
    });
    assert_eq!(tidal["needs_check"], false, "{tidal}");
    assert_eq!(
        *opened.lock().unwrap(),
        [format!("{}/register", servers.base)]
    );
    let token_quota = format!("/v1/{}/quota", TOKEN.replace(' ', "%20"));
    let pass_quota = format!("/v1/{PASS_GIVEN}/quota");
    {
        let seen = servers.seen.lock().unwrap();
        assert_eq!(
            seen.registered,
            [json!({ "app": "desktop", "proof": { "kind": "turnstile", "token": SOLVED } })]
        );
        assert!(seen.paths.contains(&pass_quota), "{:?}", seen.paths);
        assert!(
            !seen.paths.contains(&token_quota),
            "the access token is not used once there is a pass"
        );
        assert!(
            seen.agents.iter().all(|a| a.starts_with("Stash/")),
            "{:?}",
            seen.agents
        );
    }

    // The add-on refuses the pass (withdrawn): it is let go, the access token is used again, and Settings offers the
    // check. Adding again asks the allowance, which is where the add-on says so.
    servers
        .refuse_pass
        .store(true, std::sync::atomic::Ordering::Relaxed);
    client.post("/api/tidal/add", json!({}));
    let tidal = client.until("/api/tidal", "the check offered again", |t| {
        t["needs_check"] == true
    });
    assert_eq!(tidal["added"], true);
    client.until(
        "/api/tidal",
        "the allowance asked with the access token",
        |_| servers.seen.lock().unwrap().paths.contains(&token_quota),
    );

    // Passing the check again gives a pass again.
    servers
        .refuse_pass
        .store(false, std::sync::atomic::Ordering::Relaxed);
    servers.seen.lock().unwrap().paths.clear();
    client.post("/api/tidal/check", json!({}));
    client.until(
        "/api/tidal",
        "a new pass, the check no longer offered",
        |t| t["needs_check"] == false && t["checking"] == false,
    );
    assert_eq!(opened.lock().unwrap().len(), 2);
    assert_eq!(servers.seen.lock().unwrap().registered.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn where_the_add_on_does_not_offer_its_check_yet_add_uses_the_access_token_as_before() {
    let servers = Servers::start_with(false, false);
    let opened = Arc::new(Mutex::new(Vec::new()));
    let (client, dir) = core_with(
        "no-check",
        Some(&servers),
        None,
        None,
        Some(passing_shell(Arc::clone(&opened))),
    );
    let tidal = add(&client);
    assert_eq!(tidal["checking"], false);
    assert!(
        opened.lock().unwrap().is_empty(),
        "no window: the add-on gives no check"
    );
    assert!(
        servers
            .seen
            .lock()
            .unwrap()
            .paths
            .contains(&format!("/v1/{}/quota", TOKEN.replace(' ', "%20")))
    );
    let _ = std::fs::remove_dir_all(&dir);
}
