//! The library away from home: a supporter's phone plays it from anywhere, through Stash's home relay.
//!
//! The core keeps one WebSocket to the relay, under a home id it made up and with a home key only it holds, and
//! shows the listener the address their Subsonic app uses (`<relay>/h/<home id>`). Each request a phone sends there
//! comes down the WebSocket; the core answers it as it answers a device on its network ([`crate::subsonic`], the
//! device's own password and all) and POSTs the answer back, streamed. The relay is for supporters: the core connects
//! with the tip jar's supporter token, claimed here by email as the phone app claims it.
//!
//! The relay's and tip jar's addresses are given when Stash is built (`STASH_HOME_RELAY`, `STASH_TIPJAR_URL`); a
//! build without them has no relay, and says so.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tiny_http::{Method, Request};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, WebSocket};

use crate::serve::{Reply, State, body_json, refuse, reply_json};
use crate::subsonic::{Asked, MOST_FORM};

const ON: &str = "away.on";
const ID: &str = "away.id";
const KEY: &str = "away.key";
const TOKEN: &str = "supporter.token";
/// Requests answered at once (a song playing holds one).
const MOST_ANSWERING: usize = 12;
/// A ping to the relay this often keeps the way open (the relay answers it without waking).
const PING_EVERY: Duration = Duration::from_secs(25);
/// No answer to pings for this long: the connection is gone.
const SILENT_FOR: Duration = Duration::from_secs(70);

/// The relay's and tip jar's addresses, from the environment (for trying a relay) or as built.
fn configured(name: &str, built: Option<&'static str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| built.map(str::to_string))
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| usable(u))
}

/// An https address, or an http one on this computer (a relay run locally to try it).
fn usable(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => parsed.host_str().is_some(),
        "http" => matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
        _ => false,
    }
}

#[derive(Default)]
struct Link {
    connected: bool,
    trouble: Option<String>,
    /// The relay refused the token: not tried again until it changes.
    refused_token: Option<String>,
}

pub(crate) struct Away {
    relay: Option<String>,
    tipjar: Option<String>,
    link: Mutex<Link>,
    answering: Arc<AtomicUsize>,
    /// The email a code was sent to, waiting for the code.
    claiming: Mutex<Option<String>>,
    /// Set when what the connection depends on changed (switched off, a new address): it starts again.
    restart: AtomicBool,
}

impl Default for Away {
    fn default() -> Self {
        Self {
            relay: configured("STASH_HOME_RELAY", option_env!("STASH_HOME_RELAY")),
            tipjar: configured("STASH_TIPJAR_URL", option_env!("STASH_TIPJAR_URL")),
            link: Mutex::default(),
            answering: Arc::default(),
            claiming: Mutex::default(),
            restart: AtomicBool::default(),
        }
    }
}

/// What the connection needs: on, a supporter token, and the home's id and key.
struct Wanted {
    token: String,
    id: String,
    key: String,
}

/// A home id: 10 random bytes, as lower case base32 (what the relay takes).
fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % 32] as char)
        .collect()
}

impl Away {
    fn setting(state: &State, key: &str) -> Option<String> {
        state
            .library
            .lock()
            .unwrap()
            .setting(key)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
    }

    /// The home's id and key, made the first time they are needed.
    fn home(state: &State) -> Result<(String, String)> {
        let mut library = state.library.lock().unwrap();
        let id = library.setting(ID)?.filter(|v| !v.is_empty());
        let key = library.setting(KEY)?.filter(|v| !v.is_empty());
        if let (Some(id), Some(key)) = (id, key) {
            return Ok((id, key));
        }
        let (id, key) = (new_id(), stash_core::cloud::auth::random_token(32));
        library.set_setting(ID, &id)?;
        library.set_setting(KEY, &key)?;
        Ok((id, key))
    }

    fn wanted(&self, state: &State) -> Option<Wanted> {
        self.relay.as_ref()?;
        if Self::setting(state, ON).as_deref() != Some("1") {
            return None;
        }
        let token = Self::setting(state, TOKEN)?;
        if self.link.lock().unwrap().refused_token.as_deref() == Some(token.as_str()) {
            return None;
        }
        let (id, key) = Self::home(state).ok()?;
        Some(Wanted { token, id, key })
    }

    fn told(&self, connected: bool, trouble: Option<String>) {
        let mut link = self.link.lock().unwrap();
        link.connected = connected;
        link.trouble = trouble;
    }

    fn status(&self, state: &State) -> Value {
        let link = self.link.lock().unwrap();
        let on = Self::setting(state, ON).as_deref() == Some("1");
        let supporter = Self::setting(state, TOKEN).is_some();
        let id = Self::setting(state, ID);
        json!({
            "available": self.relay.is_some(),
            "can_claim": self.tipjar.is_some(),
            "on": on,
            "supporter": supporter,
            "claiming": *self.claiming.lock().unwrap(),
            "connected": link.connected,
            "trouble": link.trouble,
            "url": self.relay.as_ref().zip(id).filter(|_| on && supporter).map(|(relay, id)| format!("{relay}/h/{id}")),
            "answering": self.answering.load(Ordering::Relaxed),
        })
    }
}

// ---- the connection

/// The way to the relay: plain on this computer, else TLS.
enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

/// A read that times out is "nothing yet" (Windows says TimedOut where others say WouldBlock).
fn calm(e: std::io::Error) -> std::io::Error {
    if e.kind() == std::io::ErrorKind::TimedOut {
        std::io::Error::from(std::io::ErrorKind::WouldBlock)
    } else {
        e
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
        .map_err(calm)
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => s.get_ref(),
        }
    }
}

/// Why connecting failed: in words, and whether to try again (a token the relay refused is not tried again).
struct NotConnected {
    why: String,
    token_refused: bool,
    home_taken: bool,
}

fn not_connected(why: impl Into<String>) -> NotConnected {
    NotConnected {
        why: why.into(),
        token_refused: false,
        home_taken: false,
    }
}

fn open(relay: &str, wanted: &Wanted) -> Result<WebSocket<Conn>, NotConnected> {
    let base =
        url::Url::parse(relay).map_err(|_| not_connected("the relay's address is not right"))?;
    let tls = base.scheme() == "https";
    let host = base.host_str().unwrap_or_default().to_string();
    let port = base.port_or_known_default().unwrap_or(443);
    let address = format!(
        "{}://{}/home/{}/connect",
        if tls { "wss" } else { "ws" },
        &relay[relay.find("://").map_or(0, |i| i + 3)..],
        wanted.id
    );
    let socket = (host.as_str(), port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| not_connected("the relay could not be found: is this computer online?"))?;
    let tcp = TcpStream::connect_timeout(&socket, Duration::from_secs(10))
        .map_err(|_| not_connected("the relay could not be reached"))?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = tcp.set_nodelay(true);
    let conn = if tls {
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| not_connected("a secure connection could not be set up"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| not_connected("the relay's address is not right"))?;
        let session = rustls::ClientConnection::new(Arc::new(config), name)
            .map_err(|_| not_connected("a secure connection could not be set up"))?;
        Conn::Tls(Box::new(rustls::StreamOwned::new(session, tcp)))
    } else {
        Conn::Plain(tcp)
    };
    let mut request = address
        .into_client_request()
        .map_err(|_| not_connected("the relay's address is not right"))?;
    let headers = request.headers_mut();
    let value = |v: &str| {
        HeaderValue::from_str(v).map_err(|_| not_connected("the supporter token is not right"))
    };
    headers.insert("Authorization", value(&format!("Bearer {}", wanted.key))?);
    headers.insert("X-Stash-Entitlement", value(&wanted.token)?);
    match tungstenite::client::client(request, conn) {
        Ok((socket, _)) => {
            let _ = socket
                .get_ref()
                .tcp()
                .set_read_timeout(Some(Duration::from_millis(500)));
            Ok(socket)
        }
        Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(answer))) => {
            let status = answer.status().as_u16();
            let body = answer
                .body()
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or_default()
                .to_string();
            Err(
                match (status, body.contains("revoked"), body.contains("not_yours")) {
                    (402, ..) => NotConnected {
                        why: "the relay didn't take your supporter status: claim it again".into(),
                        token_refused: true,
                        home_taken: false,
                    },
                    (403, true, _) => NotConnected {
                        why: "this supporter status was withdrawn".into(),
                        token_refused: true,
                        home_taken: false,
                    },
                    (403, _, true) => NotConnected {
                        why: "this address belongs to another Stash: a new one is being made"
                            .into(),
                        token_refused: false,
                        home_taken: true,
                    },
                    _ => not_connected(format!("the relay answered {status}")),
                },
            )
        }
        Err(_) => Err(not_connected("the relay could not be reached")),
    }
}

/// Keeps the home connected while the listener has it on. Answers are made on threads of this scope, so a song still
/// streaming up when the connection drops goes on to its end.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let Some(relay) = state.away.relay.clone() else {
        return;
    };
    std::thread::scope(|scope| keep_connected(scope, state, &relay, stop));
}

fn keep_connected<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    state: &'env State,
    relay: &str,
    stop: &'env AtomicBool,
) {
    let away = &state.away;
    let mut wait = Duration::from_secs(1);
    while !stop.load(Ordering::Relaxed) {
        away.restart.store(false, Ordering::Relaxed);
        let Some(wanted) = away.wanted(state) else {
            if Away::setting(state, ON).as_deref() != Some("1") {
                away.told(false, None);
            } else {
                away.link.lock().unwrap().connected = false;
            }
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        if state.offline.load(Ordering::Relaxed) {
            away.told(false, Some("Stash is offline".into()));
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        match open(relay, &wanted) {
            Ok(socket) => {
                away.told(true, None);
                wait = Duration::from_secs(1);
                let why = serve(scope, state, relay, &wanted, socket, stop);
                away.told(false, why);
            }
            Err(e) => {
                if e.token_refused {
                    away.link.lock().unwrap().refused_token = Some(wanted.token.clone());
                }
                if e.home_taken {
                    let mut library = state.library.lock().unwrap();
                    let _ = library.set_setting(ID, "");
                    let _ = library.set_setting(KEY, "");
                }
                away.told(false, Some(e.why));
                // Tried again before long, a little later each time (up to a minute), or at once when asked.
                let until = Instant::now() + wait;
                while Instant::now() < until
                    && !stop.load(Ordering::Relaxed)
                    && !away.restart.load(Ordering::Relaxed)
                {
                    std::thread::sleep(Duration::from_millis(250));
                }
                wait = (wait * 2).min(Duration::from_secs(60));
            }
        }
    }
}

/// Answers what comes down the WebSocket until it closes, Stash stops, or the listener switches it off; why it ended,
/// when that is worth saying.
fn serve<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    state: &'env State,
    relay: &str,
    wanted: &Wanted,
    mut socket: WebSocket<Conn>,
    stop: &'env AtomicBool,
) -> Option<String> {
    let away = &state.away;
    let mut pinged = Instant::now();
    let mut heard = Instant::now();
    let answers = Answers {
        state,
        stop,
        agent: answer_agent(relay),
        to: format!("{relay}/home/{}/answer/", wanted.id),
        key: wanted.key.clone(),
    };
    loop {
        if stop.load(Ordering::Relaxed)
            || away.restart.load(Ordering::Relaxed)
            || state.offline.load(Ordering::Relaxed)
        {
            let _ = socket.close(None);
            let _ = socket.flush();
            return None;
        }
        if pinged.elapsed() >= PING_EVERY {
            if socket.send(Message::Text("ping".into())).is_err() {
                return Some("the connection to the relay dropped: reconnecting".into());
            }
            pinged = Instant::now();
        }
        if heard.elapsed() >= SILENT_FOR {
            return Some("the relay stopped answering: reconnecting".into());
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                heard = Instant::now();
                if text.as_str() == "pong" {
                    continue;
                }
                let Ok(asked) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if asked["t"] != "req" {
                    continue;
                }
                let Some(id) = asked["id"]
                    .as_str()
                    .filter(|id| {
                        !id.is_empty()
                            && id.len() <= 24
                            && id.bytes().all(|b| b.is_ascii_alphanumeric())
                    })
                    .map(str::to_string)
                else {
                    continue;
                };
                if away.answering.load(Ordering::Relaxed) >= MOST_ANSWERING {
                    let _ = socket.send(Message::Text(
                        json!({ "t": "fail", "id": id }).to_string().into(),
                    ));
                    continue;
                }
                answer_in_turn(scope, &answers, id, asked);
            }
            Ok(Message::Close(_)) => {
                return Some("the relay closed the connection: reconnecting".into());
            }
            Ok(_) => heard = Instant::now(),
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return Some("the connection to the relay dropped: reconnecting".into()),
        }
    }
}

fn answer_agent(relay: &str) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(relay.starts_with("https://"))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build()
        .into()
}

/// What answering needs, the same for every request on one connection.
struct Answers<'env> {
    state: &'env State,
    stop: &'env AtomicBool,
    agent: ureq::Agent,
    /// The address answers go to, but for the request's id.
    to: String,
    key: String,
}

/// Answers one request on a thread of its own: the answer is made as for a device on the network, and its body
/// streams up to the relay as it is read (a song is never read whole first).
fn answer_in_turn<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    answers: &Answers<'env>,
    id: String,
    asked: Value,
) {
    let (state, stop) = (answers.state, answers.stop);
    let form = asked["form"]
        .as_str()
        .and_then(|f| STANDARD.decode(f).ok())
        .filter(|f| f.len() <= MOST_FORM);
    let request = Asked {
        url: asked["url"].as_str().unwrap_or_default().to_string(),
        form,
        range: asked["range"].as_str().map(str::to_string),
        ip: asked["ip"]
            .as_str()
            .and_then(|ip| ip.parse::<IpAddr>().ok()),
    };
    let head = asked["method"] == "HEAD";
    let to = format!("{}{id}", answers.to);
    let key = answers.key.clone();
    let agent = answers.agent.clone();
    let answering = Arc::clone(&state.away.answering);
    answering.fetch_add(1, Ordering::Relaxed);
    scope.spawn(move || {
        struct Done(Arc<AtomicUsize>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Relaxed);
            }
        }
        let _done = Done(answering);
        if !request.url.starts_with("/rest/") {
            return;
        }
        let out = crate::subsonic::respond(state, &request);
        let headers = serde_json::to_string(&out.headers).unwrap_or_else(|_| "[]".into());
        let mut post = agent
            .post(&to)
            .header("Authorization", &format!("Bearer {key}"))
            .header("X-Stash-Status", &out.status.to_string())
            .header("X-Stash-Headers", &headers);
        if let Some(length) = out.length {
            post = post.header("X-Stash-Length", &length.to_string());
        }
        let _ = if head {
            post.send(&[][..])
        } else {
            let mut body = Stoppable {
                inner: out.body,
                stop,
            };
            post.send(ureq::SendBody::from_reader(&mut body))
        };
    });
}

/// A body that ends (with an error) once Stash stops, so quitting never waits for a song to finish streaming up.
struct Stoppable<'a> {
    inner: crate::subsonic::Body,
    stop: &'a AtomicBool,
}

impl Read for Stoppable<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("Stash is stopping"));
        }
        self.inner.read(buf)
    }
}

// ---- the supporter claim (the tip jar's /claim/email/start and /verify, as the phone app does it)

fn tipjar_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into()
}

fn claim(tipjar: &str, path: &str, body: Value) -> Result<(u16, Value), String> {
    let mut answer = tipjar_agent()
        .post(&format!("{tipjar}{path}"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|_| "the tip jar could not be reached: is this computer online?".to_string())?;
    let status = answer.status().as_u16();
    let value = answer
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    Ok((status, value))
}

/// Settings' routes for away from home; `None`: the path is not one of them.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/away") {
        return Ok(None);
    }
    let away = &state.away;
    let method = request.method().clone();
    match (method, path) {
        (Method::Get, "/api/away") => Ok(Some(reply_json(200, away.status(state)))),
        (Method::Post, "/api/away") => {
            let body = body_json(request)?;
            if let Some(on) = body.get("on").and_then(Value::as_bool) {
                if on && away.relay.is_none() {
                    return Ok(Some(refuse(409, "this build of Stash has no relay")));
                }
                state
                    .library
                    .lock()
                    .unwrap()
                    .set_setting(ON, if on { "1" } else { "0" })?;
                if on {
                    Away::home(state)?;
                }
                away.restart.store(true, Ordering::Relaxed);
            }
            // Connected (or not) within a few seconds: the answer says how it went.
            for _ in 0..24 {
                std::thread::sleep(Duration::from_millis(250));
                let link = away.link.lock().unwrap();
                if link.connected
                    || link.trouble.is_some()
                    || body.get("on") != Some(&Value::Bool(true))
                {
                    break;
                }
            }
            Ok(Some(reply_json(200, away.status(state))))
        }
        (Method::Post, "/api/away/new-address") => {
            {
                let mut library = state.library.lock().unwrap();
                library.set_setting(ID, "")?;
                library.set_setting(KEY, "")?;
            }
            Away::home(state)?;
            away.restart.store(true, Ordering::Relaxed);
            Ok(Some(reply_json(200, away.status(state))))
        }
        (Method::Post, "/api/away/claim/start") => {
            let Some(tipjar) = &away.tipjar else {
                return Ok(Some(refuse(
                    409,
                    "this build of Stash can't claim supporter status",
                )));
            };
            let body = body_json(request)?;
            let email = body
                .get("email")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !email.contains('@') || email.len() > 254 {
                return Ok(Some(refuse(400, "that doesn't look like an email address")));
            }
            match claim(tipjar, "/claim/email/start", json!({ "email": email })) {
                Ok((202 | 200, _)) => {
                    *away.claiming.lock().unwrap() = Some(email);
                    Ok(Some(reply_json(200, away.status(state))))
                }
                Ok((400, _)) => Ok(Some(refuse(400, "that doesn't look like an email address"))),
                Ok((429, _)) => Ok(Some(refuse(
                    429,
                    "too many tries: wait an hour and try again",
                ))),
                Ok((status, _)) => Ok(Some(refuse(
                    502,
                    &format!("the tip jar answered {status}: try again later"),
                ))),
                Err(why) => Ok(Some(refuse(502, &why))),
            }
        }
        (Method::Post, "/api/away/claim/verify") => {
            let Some(tipjar) = &away.tipjar else {
                return Ok(Some(refuse(
                    409,
                    "this build of Stash can't claim supporter status",
                )));
            };
            let body = body_json(request)?;
            let code = body
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let Some(email) = away.claiming.lock().unwrap().clone() else {
                return Ok(Some(refuse(409, "ask for a code first")));
            };
            match claim(
                tipjar,
                "/claim/email/verify",
                json!({ "email": email, "code": code }),
            ) {
                Ok((200, answer)) => {
                    let Some(token) = answer
                        .get("token")
                        .and_then(Value::as_str)
                        .filter(|t| t.len() < 512)
                    else {
                        return Ok(Some(refuse(
                            502,
                            "the tip jar's answer was not right: try again later",
                        )));
                    };
                    state.library.lock().unwrap().set_setting(TOKEN, token)?;
                    *away.claiming.lock().unwrap() = None;
                    away.link.lock().unwrap().refused_token = None;
                    away.restart.store(true, Ordering::Relaxed);
                    Ok(Some(reply_json(200, away.status(state))))
                }
                Ok((401, _)) => Ok(Some(refuse(
                    401,
                    "that code didn't work. Check the newest email from Stash, or ask for a new code. The code comes \
                     only if that address gave $5 or more on Ko-fi.",
                ))),
                Ok((429, _)) => Ok(Some(refuse(
                    429,
                    "too many tries: wait a minute and try again",
                ))),
                Ok((status, _)) => Ok(Some(refuse(
                    502,
                    &format!("the tip jar answered {status}: try again later"),
                ))),
                Err(why) => Ok(Some(refuse(502, &why))),
            }
        }
        (Method::Post, "/api/away/claim/cancel") => {
            *away.claiming.lock().unwrap() = None;
            Ok(Some(reply_json(200, away.status(state))))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_is_https_or_on_this_computer() {
        assert!(usable("https://relay.example"));
        assert!(usable("http://127.0.0.1:8787"));
        assert!(!usable("http://relay.example"));
        assert!(!usable("ftp://relay.example"));
        assert!(!usable("nonsense"));
        let id = new_id();
        assert_eq!(id.len(), 16);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
        );
        assert_eq!(stash_core::cloud::auth::random_token(32).len(), 43);
    }
}
