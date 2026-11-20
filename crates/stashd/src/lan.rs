//! The library on the listener's other devices: their phone or a browser on the same network plays it through an app
//! that speaks the Subsonic API (Symfonium, Feishin, Substreamer, DSub…; see [`crate::subsonic`]).
//!
//! Off until the listener switches it on. Then the core listens on the network as well, on a port of its own and apart
//! from its local API: there it answers the Subsonic API and nothing else, and only to a device the listener added,
//! each with a user name and password of its own (shown in Settings, taken back by removing the device). A place on
//! the network that keeps getting the password wrong is not answered for a while.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tiny_http::{Method, Request, Server};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

const ON: &str = "lan.on";
const PORT: &str = "lan.port";
const DEVICES: &str = "lan.devices";
/// The port the library is served on, unless the listener picks another.
pub(crate) const DEFAULT_PORT: u16 = 4747;
/// Requests answered at once (a stream holds one for as long as it plays).
const WORKERS: usize = 8;
/// Wrong passwords from one place before it is not answered…
const TRIES: u32 = 10;
/// …for this long.
const SHUT_OUT: Duration = Duration::from_secs(10 * 60);

/// A device the listener added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Device {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) user: String,
    pub(crate) password: String,
    pub(crate) added_ms: i64,
    #[serde(default)]
    pub(crate) seen_ms: Option<i64>,
}

/// A cover fetched from the web: its address, its kind, its bytes.
pub(crate) type KeptCover = (String, &'static str, Arc<Vec<u8>>);

#[derive(Default)]
struct Config {
    read: bool,
    on: bool,
    port: u16,
    devices: Vec<Device>,
}

#[derive(Default)]
pub(crate) struct Lan {
    config: Mutex<Config>,
    /// The port listened on, or why it could not be.
    listening: Mutex<Option<u16>>,
    trouble: Mutex<Option<String>>,
    /// Wrong passwords by place: how many, since when.
    wrong: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// When the devices' last seen times were last kept.
    kept_seen: Mutex<Option<Instant>>,
    /// Where songs were found to play from lately (a player asks for a song a few times as it seeks).
    pub(crate) located: Mutex<HashMap<i64, (stash_core::player::Located, Instant)>>,
    /// This computer's Tailscale name, as last asked, and when.
    tailscale: Mutex<Option<(Instant, Option<String>)>>,
    /// Covers fetched from the web lately.
    pub(crate) covers: Mutex<Vec<KeptCover>>,
}

/// A password: letters and digits a person can read out and type (no 0/O, 1/l/I).
fn password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

/// A user name from a device's name: lower case letters and digits.
fn user_of(name: &str, taken: &[Device]) -> String {
    let base: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(20)
        .collect();
    let base = if base.is_empty() {
        "device".to_string()
    } else {
        base
    };
    let mut user = base.clone();
    let mut n = 2;
    while taken.iter().any(|d| d.user == user) {
        user = format!("{base}{n}");
        n += 1;
    }
    user
}

/// This computer's address on its network: the one it would reach the internet from (nothing is sent).
pub(crate) fn address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("8.8.8.8", 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

impl Lan {
    fn config<'a>(&'a self, state: &State) -> std::sync::MutexGuard<'a, Config> {
        let mut config = self.config.lock().unwrap();
        if !config.read {
            let library = state.library.lock().unwrap();
            let setting = |key: &str| library.setting(key).ok().flatten();
            config.on = setting(ON).as_deref() == Some("1");
            config.port = setting(PORT)
                .and_then(|p| p.parse().ok())
                .filter(|p| *p >= 1024)
                .unwrap_or(DEFAULT_PORT);
            config.devices = setting(DEVICES)
                .and_then(|d| serde_json::from_str(&d).ok())
                .unwrap_or_default();
            config.read = true;
        }
        config
    }

    fn keep(&self, state: &State, config: &Config) -> Result<()> {
        let mut library = state.library.lock().unwrap();
        library.set_setting(ON, if config.on { "1" } else { "0" })?;
        library.set_setting(PORT, &config.port.to_string())?;
        library.set_setting(DEVICES, &serde_json::to_string(&config.devices)?)?;
        Ok(())
    }

    /// The port to listen on, when the listener has it on.
    fn wanted(&self, state: &State) -> Option<u16> {
        let config = self.config(state);
        config.on.then_some(config.port)
    }

    /// Whether `ip` got the password wrong too often lately.
    pub(crate) fn shut_out(&self, ip: Option<IpAddr>) -> bool {
        let Some(ip) = ip else { return false };
        let mut wrong = self.wrong.lock().unwrap();
        wrong.retain(|_, (_, since)| since.elapsed() < SHUT_OUT);
        wrong.get(&ip).is_some_and(|(n, _)| *n >= TRIES)
    }

    pub(crate) fn got_wrong(&self, ip: Option<IpAddr>) {
        if let Some(ip) = ip {
            let mut wrong = self.wrong.lock().unwrap();
            let entry = wrong.entry(ip).or_insert((0, Instant::now()));
            entry.0 += 1;
        }
    }

    /// The device whose user name is `user` and whose password passes `check`; it is noted as seen.
    pub(crate) fn device(
        &self,
        state: &State,
        user: Option<&str>,
        check: impl Fn(&str) -> bool,
    ) -> Option<Device> {
        let found = {
            let mut config = self.config(state);
            let found = config
                .devices
                .iter_mut()
                .find(|d| user.is_none_or(|u| d.user == u) && check(&d.password))?;
            found.seen_ms = Some(now_ms());
            found.clone()
        };
        // The times devices were seen are kept now and then.
        let due = {
            let mut kept = self.kept_seen.lock().unwrap();
            let due = kept.is_none_or(|at| at.elapsed() > Duration::from_secs(60));
            if due {
                *kept = Some(Instant::now());
            }
            due
        };
        if due {
            let config = self.config(state);
            let _ = state.library.lock().unwrap().set_setting(
                DEVICES,
                &serde_json::to_string(&config.devices).unwrap_or_default(),
            );
        }
        Some(found)
    }

    fn status(&self, state: &State) -> Value {
        let tailscale = self.tailscale();
        let config = self.config(state);
        let listening = *self.listening.lock().unwrap();
        let address = address();
        json!({
            "on": config.on,
            "port": config.port,
            "listening": listening,
            "trouble": *self.trouble.lock().unwrap(),
            "address": address.map(|ip| ip.to_string()),
            "url": address.zip(listening).map(|(ip, port)| format!("http://{ip}:{port}")),
            // With Tailscale on this computer (and on the phone), the share is reached from anywhere at this address.
            "tailscale": tailscale.map(|name| json!({
                "name": name,
                "url": listening.map(|port| format!("http://{name}:{port}")),
            })),
            "devices": config.devices,
            // For smaller songs on mobile data (see `transcode`); fetched as for Mornye's extensions.
            "ffmpeg": state.mornye().map(|m| m.ffmpeg_told()).unwrap_or_else(|| json!({
                "found": state.ffmpeg().is_some(),
                "can_fetch": false,
                "fetching": null,
                "trouble": null,
            })),
        })
    }

    /// This computer's name on its Tailscale network, when Tailscale runs here (asked at most once a minute).
    fn tailscale(&self) -> Option<String> {
        let mut known = self.tailscale.lock().unwrap();
        if let Some((at, name)) = &*known
            && at.elapsed() < Duration::from_secs(60)
        {
            return name.clone();
        }
        let name = tailscale_name();
        *known = Some((Instant::now(), name.clone()));
        name
    }
}

/// `tailscale status --json`'s own name (its MagicDNS name, else its first address), when Tailscale is running.
fn tailscale_name() -> Option<String> {
    let program = if cfg!(windows) {
        let installed = std::path::Path::new(r"C:\Program Files\Tailscale\tailscale.exe");
        if installed.is_file() {
            installed.to_path_buf()
        } else {
            "tailscale.exe".into()
        }
    } else {
        "tailscale".into()
    };
    let mut command = std::process::Command::new(program);
    command
        .args(["status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    tailscale_of(&serde_json::from_slice(&output.stdout).ok()?)
}

fn tailscale_of(status: &Value) -> Option<String> {
    if status["BackendState"] != "Running" {
        return None;
    }
    let me = &status["Self"];
    me["DNSName"]
        .as_str()
        .map(|n| n.trim_end_matches('.').to_string())
        .filter(|n| !n.is_empty())
        .or_else(|| me["TailscaleIPs"][0].as_str().map(str::to_string))
}

/// Listens on the network while the listener has it on, and stops when they switch it off (or change the port).
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let lan = &state.lan;
    while !stop.load(Ordering::Relaxed) {
        let Some(port) = lan.wanted(state) else {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        };
        let server = match Server::http(("0.0.0.0", port)) {
            Ok(server) => server,
            Err(e) => {
                *lan.trouble.lock().unwrap() = Some(format!(
                    "Stash could not use port {port} ({e}): another program may have it. Pick another port."
                ));
                // Tried again when the port changes, or in a while.
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(500));
                    if stop.load(Ordering::Relaxed) || lan.wanted(state) != Some(port) {
                        break;
                    }
                }
                continue;
            }
        };
        *lan.trouble.lock().unwrap() = None;
        *lan.listening.lock().unwrap() = Some(port);
        let quit = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    while !quit.load(Ordering::Relaxed) {
                        match server.recv_timeout(Duration::from_millis(250)) {
                            Ok(Some(request)) => answer(state, request),
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
            while !stop.load(Ordering::Relaxed) && lan.wanted(state) == Some(port) {
                std::thread::sleep(Duration::from_millis(500));
            }
            quit.store(true, Ordering::Relaxed);
            server.unblock();
        });
        *lan.listening.lock().unwrap() = None;
    }
}

/// One request from the network: the Subsonic API, and nothing else.
fn answer(state: &State, request: Request) {
    let path = request.url().split('?').next().unwrap_or("").to_string();
    if path.starts_with("/rest/") {
        crate::subsonic::answer(state, request);
        return;
    }
    let text = if path == "/" {
        "Stash is sharing its library here. Connect a Subsonic app to this address with the user name and password \
         shown in Stash: Settings, Library, Your library on other devices."
    } else {
        "Not found"
    };
    let status = if path == "/" { 200 } else { 404 };
    let _ = request.respond(
        tiny_http::Response::from_string(text)
            .with_status_code(status)
            .with_header(
                tiny_http::Header::from_bytes("Content-Type", "text/plain; charset=utf-8")
                    .expect("a well-formed header"),
            ),
    );
}

/// Settings' routes for sharing on the network; `None`: the path is not one of them.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/lan") {
        return Ok(None);
    }
    let lan = &state.lan;
    let method = request.method().clone();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (Method::Get, ["api", "lan"]) => Ok(Some(reply_json(200, lan.status(state)))),
        (Method::Post, ["api", "lan"]) => {
            let body = body_json(request)?;
            {
                let mut config = lan.config(state);
                if let Some(on) = body.get("on").and_then(Value::as_bool) {
                    config.on = on;
                }
                if let Some(port) = body.get("port").and_then(Value::as_u64) {
                    if !(1024..=65535).contains(&port) {
                        return Ok(Some(refuse(400, "pick a port from 1024 to 65535")));
                    }
                    config.port = port as u16;
                }
                lan.keep(state, &config)?;
            }
            // Started (or stopped) by its thread within the second: the answer says how it went.
            for _ in 0..12 {
                std::thread::sleep(Duration::from_millis(250));
                let wanted = lan.wanted(state);
                let (listening, trouble) = (
                    *lan.listening.lock().unwrap(),
                    lan.trouble.lock().unwrap().is_some(),
                );
                if listening == wanted || trouble {
                    break;
                }
            }
            Ok(Some(reply_json(200, lan.status(state))))
        }
        (Method::Post, ["api", "lan", "devices"]) => {
            let body = body_json(request)?;
            let name: String = body
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .chars()
                .take(40)
                .collect();
            if name.is_empty() {
                return Ok(Some(refuse(400, "give the device a name")));
            }
            let mut config = lan.config(state);
            if config.devices.len() >= 32 {
                return Ok(Some(refuse(
                    409,
                    "that is a lot of devices: remove one first",
                )));
            }
            let device = Device {
                id: stash_core::cloud::auth::random_token(9),
                user: user_of(&name, &config.devices),
                name,
                password: password(),
                added_ms: now_ms(),
                seen_ms: None,
            };
            config.devices.push(device);
            lan.keep(state, &config)?;
            drop(config);
            Ok(Some(reply_json(200, lan.status(state))))
        }
        (Method::Post, ["api", "lan", "devices", id, what @ ("remove" | "password")]) => {
            let mut config = lan.config(state);
            let Some(at) = config.devices.iter().position(|d| d.id == *id) else {
                return Ok(Some(refuse(404, "no such device")));
            };
            if *what == "remove" {
                config.devices.remove(at);
            } else {
                config.devices[at].password = password();
            }
            lan.keep(state, &config)?;
            drop(config);
            Ok(Some(reply_json(200, lan.status(state))))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_devices_name_gives_a_user_name_of_its_own_and_a_readable_password() {
        let taken = vec![Device {
            id: "a".into(),
            name: "Phone".into(),
            user: "phone".into(),
            password: "x".into(),
            added_ms: 0,
            seen_ms: None,
        }];
        assert_eq!(user_of("Phone", &taken), "phone2");
        assert_eq!(user_of("Joe's Laptop!", &taken), "joeslaptop");
        assert_eq!(user_of("…", &taken), "device");
        let p = password();
        assert_eq!(p.len(), 16);
        assert!(
            p.chars()
                .all(|c| c.is_ascii_alphanumeric() && !"0O1lI".contains(c))
        );
        assert_ne!(p, password());
    }

    #[test]
    fn tailscales_name_for_this_computer_is_its_own() {
        let running = json!({ "BackendState": "Running", "Self": { "DNSName": "desk.tail1234.ts.net.", "TailscaleIPs": ["100.64.0.2"] } });
        assert_eq!(
            tailscale_of(&running).as_deref(),
            Some("desk.tail1234.ts.net")
        );
        let bare = json!({ "BackendState": "Running", "Self": { "DNSName": "", "TailscaleIPs": ["100.64.0.2"] } });
        assert_eq!(tailscale_of(&bare).as_deref(), Some("100.64.0.2"));
        assert_eq!(tailscale_of(&json!({ "BackendState": "Stopped" })), None);
    }
}
