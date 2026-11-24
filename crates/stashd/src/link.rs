//! "Your phone": linking Stash on this computer with the Stash app on the listener's phone (`stash_core::link`), so the phone
//! can be the remote for what this computer plays. This computer shows a QR code, the phone scans it, both show the same six
//! digits, the listener clicks Link here, and this computer joins the phone's space on the sync service.
//!
//! What this computer keeps (`phone-link.json` beside the library, never in it): its device id, token and P-256 key, its name,
//! and the space it is in (id, keys by epoch, the other devices' names and pinned keys).
//!
//! - `GET /api/link`: linked or not, the devices of the space, and the pairing's step.
//! - `POST /api/link/pair`: shows a code (a new one each time). `POST /api/link/pair/stop`: stops showing it.
//! - `POST /api/link/confirm`: the six digits match: link. `POST /api/link/cancel`: they don't: a new code.
//! - `POST /api/link/refresh`: reads the devices again. `POST /api/link/unlink`: leaves the space.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stash_core::link::crypto::{
    Envelope, b64u, data_key, open, place, random_bytes, seal, token_hash, unb64,
};
use stash_core::link::keys::{
    KeyPair, Label, label_doc, open_key_envelope, pair_code, pair_key, pair_label_key, qr_link, read_label,
    read_pair_answer,
};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, reply_json};

/// The sync service. `STASH_SYNC_URL` points a test at a local one.
fn base() -> String {
    std::env::var("STASH_SYNC_URL").unwrap_or_else(|_| "https://sync.stashfm.app".into())
}

/// A code is renewed this many times while shown, then waits for "New code".
const MAX_RENEWALS: u32 = 3;
/// How long this computer waits for the phone to let it in after the listener clicked Link.
const JOIN_WAIT: Duration = Duration::from_secs(60);
const FILE: &str = "phone-link.json";

// ------------------------------------------------------------------------------------------------ what is kept

/// This computer as a device of the sync service.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Device {
    pub(crate) id: String,
    /// 32 random bytes, base64url: what proves this device to the server.
    pub(crate) token: String,
    /// The P-256 key's 32-byte secret, base64url.
    secret: String,
    #[serde(rename = "pub")]
    pub(crate) public: String,
    pub(crate) name: String,
}

impl Device {
    fn new(name: String) -> Device {
        const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let id: String = random_bytes(16).iter().map(|b| ALNUM[*b as usize % ALNUM.len()] as char).collect();
        let key = KeyPair::generate();
        Device {
            id: format!("d_{id}"),
            token: b64u(&random_bytes(32)),
            secret: b64u(&key.secret_bytes()),
            public: b64u(&key.public),
            name,
        }
    }
    pub(crate) fn key(&self) -> Option<KeyPair> {
        KeyPair::from_secret(&unb64(&self.secret).ok()?).ok()
    }
    fn pub_bytes(&self) -> Vec<u8> {
        unb64(&self.public).unwrap_or_default()
    }
    pub(crate) fn auth(&self) -> String {
        format!("Stash-Device {}:{}", self.id, self.token)
    }
    fn label(&self) -> Label {
        Label { name: self.name.clone(), kind: "desktop".into(), device_id: self.id.clone(), public: self.pub_bytes() }
    }
    fn record(&self, label_ct: &Envelope) -> Value {
        json!({ "id": self.id, "tokenHash": token_hash(&unb64(&self.token).unwrap_or_default()), "pub": self.public, "labelCt": label_ct })
    }
}

/// Another device of the space, as this computer verified it from its sealed label.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RosterEntry {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) name: String,
    #[serde(rename = "pub")]
    public: String,
}

/// The space this computer is in.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Space {
    pub(crate) id: String,
    pub(crate) epoch: i64,
    /// The space key of each epoch still needed (this one and the one before), base64url.
    keys: BTreeMap<i64, String>,
    pub(crate) roster: BTreeMap<String, RosterEntry>,
    linked_at: i64,
}

impl Space {
    /// The data key of the current epoch.
    pub(crate) fn data_key(&self) -> Option<Vec<u8>> {
        self.data_key_at(self.epoch)
    }
    pub(crate) fn data_key_at(&self, epoch: i64) -> Option<Vec<u8>> {
        Some(data_key(&unb64(self.keys.get(&epoch)?).ok()?, &self.id))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Kept {
    device: Option<Device>,
    space: Option<Space>,
}

// ------------------------------------------------------------------------------------------------ what the window sees

#[derive(Clone, Serialize)]
#[serde(tag = "step", rename_all = "camelCase")]
enum PairStep {
    Idle,
    Opening,
    /// The QR's link, and when the code runs out (this computer's clock, ms).
    #[serde(rename_all = "camelCase")]
    Code { link: String, expires_at: i64, renewals: u32 },
    /// The code ran out too often: "New code".
    Stale,
    /// The phone answered: the six digits, and the phone's name.
    #[serde(rename_all = "camelCase")]
    Confirm { code: String, phone_name: String },
    #[serde(rename_all = "camelCase")]
    Linking { phone_name: String },
    #[serde(rename_all = "camelCase")]
    Done { phone_name: String },
    Error { message: String },
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceView {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    name: String,
    me: bool,
    /// `ok`, `unverified` (no label this computer could open yet) or `keyChanged`.
    status: &'static str,
    last_seen_at: i64,
}

/// The phone's answer, held until the listener clicks Link (nothing of it is kept before).
struct Answer {
    pair_id: String,
    kpair: Vec<u8>,
    phone: Label,
    space: stash_core::link::keys::SpaceGrant,
}

struct Inner {
    kept: Kept,
    pair: PairStep,
    answer: Option<Answer>,
    devices: Vec<DeviceView>,
    problem: Option<String>,
    busy: bool,
}

/// The link, shared by the routes, the pairing thread and the remote (`crate::remote`).
pub(crate) struct Link {
    inner: Mutex<Inner>,
    file: OnceLock<PathBuf>,
    /// Goes up with each new code or stop: a pairing thread whose number is old gives up.
    generation: AtomicU64,
    /// Goes up whenever the space or this device changes, so the remote reconnects.
    pub(crate) changed: AtomicU64,
}

impl Default for Link {
    fn default() -> Link {
        Link {
            inner: Mutex::new(Inner {
                kept: Kept::default(),
                pair: PairStep::Idle,
                answer: None,
                devices: Vec::new(),
                problem: None,
                busy: false,
            }),
            file: OnceLock::new(),
            generation: AtomicU64::new(0),
            changed: AtomicU64::new(0),
        }
    }
}

/// This computer's name, as the phone shows it.
fn computer_name() -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_default();
    if host.trim().is_empty() { "Stash on this computer".into() } else { format!("Stash on {}", host.trim()) }
}

impl Link {
    /// Reads what was kept, from the folder beside the library. Once, as the core starts.
    pub(crate) fn keep_in(&self, folder: PathBuf) {
        let file = folder.join(FILE);
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Ok(kept) = serde_json::from_str::<Kept>(&text)
        {
            self.inner.lock().unwrap().kept = kept;
        }
        let _ = self.file.set(file);
    }

    fn save(&self, kept: &Kept) {
        let Some(file) = self.file.get() else { return };
        let Ok(text) = serde_json::to_string_pretty(kept) else { return };
        let part = file.with_extension("json.part");
        if std::fs::write(&part, text).is_ok() {
            let _ = std::fs::rename(&part, file);
        }
    }

    /// This device and its space, when linked: what the remote connects with.
    pub(crate) fn linked(&self) -> Option<(Device, Space)> {
        let inner = self.inner.lock().unwrap();
        Some((inner.kept.device.clone()?, inner.kept.space.clone()?))
    }

    /// Keeps a newer space (keys walked forward by the remote, say).
    pub(crate) fn put_space(&self, space: Space) {
        let mut inner = self.inner.lock().unwrap();
        inner.kept.space = Some(space);
        self.save(&inner.kept);
    }

    /// The link is gone (this device was removed, or the space deleted): forget it.
    pub(crate) fn forget(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.kept.space = None;
        inner.devices.clear();
        self.save(&inner.kept);
        self.changed.fetch_add(1, Ordering::SeqCst);
    }

    fn view(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        let has_phone = inner.kept.space.as_ref().is_some_and(|s| s.roster.values().any(|r| r.kind == "phone"));
        json!({
            "linked": inner.kept.space.is_some(),
            "name": inner.kept.device.as_ref().map(|d| d.name.clone()).unwrap_or_else(computer_name),
            "hasPhone": has_phone,
            "devices": inner.devices,
            "pair": inner.pair,
            "problem": inner.problem,
            "busy": inner.busy,
        })
    }

    fn set_pair(&self, generation: u64, step: PairStep) -> bool {
        if self.generation.load(Ordering::SeqCst) != generation {
            return false;
        }
        self.inner.lock().unwrap().pair = step;
        true
    }
}

// ------------------------------------------------------------------------------------------------ the sync service

/// A refusal from the sync service: its HTTP status and `error.code`.
#[derive(Debug)]
pub(crate) struct Refused {
    pub(crate) status: u16,
    pub(crate) code: String,
    pub(crate) message: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code, self.message)
    }
}
impl std::error::Error for Refused {}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            // A long-poll is answered within 25 s.
            .timeout_global(Some(Duration::from_secs(35)))
            .build()
            .into()
    })
}

/// One call to the sync service: `(status, body)` for a 2xx, else [`Refused`]. Not reached at all is an `io` error.
pub(crate) fn call(method: &str, path: &str, auth: Option<&str>, body: Option<&Value>) -> Result<(u16, Value)> {
    let url = format!("{}/v1{path}", base());
    let agent = agent();
    let mut response = match (method, body) {
        ("GET", _) => {
            let mut r = agent.get(&url);
            if let Some(a) = auth {
                r = r.header("authorization", a);
            }
            r.call()?
        }
        ("DELETE", _) => {
            let mut r = agent.delete(&url);
            if let Some(a) = auth {
                r = r.header("authorization", a);
            }
            r.call()?
        }
        (_, body) => {
            let mut r = if method == "PUT" { agent.put(&url) } else { agent.post(&url) };
            if let Some(a) = auth {
                r = r.header("authorization", a);
            }
            r.header("content-type", "application/json").send(serde_json::to_vec(body.unwrap_or(&json!({})))?)?
        }
    };
    let status = response.status().as_u16();
    let text = response.body_mut().with_config().limit(4 << 20).read_to_string().unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok((status, value));
    }
    Err(Refused {
        status,
        code: value["error"]["code"].as_str().unwrap_or("unavailable").to_string(),
        message: value["error"]["message"].as_str().unwrap_or("").to_string(),
    }
    .into())
}

/// The `error.code` of a refusal, when it is one.
pub(crate) fn code_of(e: &anyhow::Error) -> Option<&str> {
    e.downcast_ref::<Refused>().map(|r| r.code.as_str())
}

/// What the listener reads for an error.
fn message_of(e: &anyhow::Error) -> String {
    match e.downcast_ref::<Refused>() {
        Some(r) => match r.code.as_str() {
            "used" => "This code was already used. Show a new one.".into(),
            "expired" => "This code expired. Show a new one.".into(),
            "rate_limited" | "daily_limit" => "Too many tries. Try again in a few minutes.".into(),
            "full" | "member" => r.message.clone(),
            _ => "Something went wrong. Try again.".into(),
        },
        None if e.downcast_ref::<stash_core::link::crypto::CryptoError>().is_some() => {
            "Linking didn't finish. Show a new code.".into()
        }
        None => "Can't reach Stash's link service. Check the connection and try again.".into(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------------ pairing

impl Link {
    /// Shows a code: a new device on the first link, a new slot, the QR; then waits for the phone's answer on a thread.
    fn start(self: &Arc<Self>, renewals: u32) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pair = PairStep::Opening;
            inner.answer = None;
            if inner.kept.device.is_none() {
                inner.kept.device = Some(Device::new(computer_name()));
                let kept = std::mem::take(&mut inner.kept);
                self.save(&kept);
                inner.kept = kept;
            }
        }
        let link = Arc::clone(self);
        let _ = std::thread::Builder::new().name("stash-link-pair".into()).spawn(move || {
            if let Err(e) = link.pairing(generation, renewals) {
                link.set_pair(generation, PairStep::Error { message: message_of(&e) });
            }
        });
    }

    fn pairing(self: &Arc<Self>, generation: u64, renewals: u32) -> Result<()> {
        let device = self.inner.lock().unwrap().kept.device.clone().expect("made above");
        let secret = random_bytes(16);
        let e_b = KeyPair::generate();
        let label_ct = seal(&pair_label_key(&secret), "pair", 0, &place::label(&device.id), &label_doc(&device.label()))?;
        let started = now_ms();
        let (_, body) = call("POST", "/desktop/pair", None, Some(&json!({ "device": device.record(&label_ct) })))?;
        let pair_id = body["pairId"].as_str().unwrap_or_default().to_string();
        // A code lives 3 minutes; its end is counted on this computer's clock, from when it was asked for.
        let expires_at = started + 3 * 60_000;
        if pair_id.len() != 22 {
            anyhow::bail!("no code");
        }
        if !self.set_pair(generation, PairStep::Code { link: qr_link(&pair_id, &secret, &e_b.public), expires_at, renewals }) {
            return Ok(());
        }
        loop {
            if self.generation.load(Ordering::SeqCst) != generation {
                return Ok(());
            }
            match call("GET", &format!("/pair/{pair_id}"), Some(&device.auth()), None) {
                Ok((200, answer)) => return self.answered(generation, &device, &pair_id, &secret, &e_b, &answer),
                Ok(_) => {}
                Err(e) if code_of(&e) == Some("expired") => {
                    if self.generation.load(Ordering::SeqCst) == generation {
                        if renewals >= MAX_RENEWALS {
                            self.set_pair(generation, PairStep::Stale);
                        } else {
                            self.start(renewals + 1);
                        }
                    }
                    return Ok(());
                }
                Err(e) if e.downcast_ref::<Refused>().is_none() => {
                    // A dropped long-poll: again shortly, unless the code ran out meanwhile.
                    if now_ms() > expires_at {
                        self.set_pair(generation, PairStep::Stale);
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// The phone answered: Kpair, its answer opened, the six digits shown. Nothing is kept yet.
    fn answered(&self, generation: u64, device: &Device, pair_id: &str, secret: &[u8], e_b: &KeyPair, body: &Value) -> Result<()> {
        let phone_pub = unb64(body["phonePub"].as_str().unwrap_or_default())?;
        let kpair = pair_key(e_b, &phone_pub, secret, pair_id, &device.id, &device.pub_bytes())?;
        let ct: Envelope = serde_json::from_value(body["ct"].clone())?;
        if ct.e != 0 {
            anyhow::bail!(stash_core::link::crypto::CryptoError("bad answer"));
        }
        let (phone, space) = read_pair_answer(&open(&kpair, "pair", &place::pair_answer(pair_id), &ct)?)?;
        if phone.device_id == device.id {
            anyhow::bail!(stash_core::link::crypto::CryptoError("bad answer"));
        }
        // The phone always names the space (it holds one, or minted one): this computer never hands its key to a phone.
        let space = space.ok_or(stash_core::link::crypto::CryptoError("no space"))?;
        let code = pair_code(&kpair);
        if self.generation.load(Ordering::SeqCst) != generation {
            return Ok(());
        }
        let mut inner = self.inner.lock().unwrap();
        inner.pair = PairStep::Confirm { code: format!("{} {}", &code[..3], &code[3..]), phone_name: phone.name.clone() };
        inner.answer = Some(Answer { pair_id: pair_id.into(), kpair, phone, space });
        Ok(())
    }

    /// The listener saw the same six digits on the phone and clicked Link: reply, wait to be let in, keep the space.
    fn confirm(self: &Arc<Self>) {
        let generation = self.generation.load(Ordering::SeqCst);
        let answer = {
            let mut inner = self.inner.lock().unwrap();
            if !matches!(inner.pair, PairStep::Confirm { .. }) {
                return;
            }
            let Some(answer) = inner.answer.take() else { return };
            inner.pair = PairStep::Linking { phone_name: answer.phone.name.clone() };
            inner.busy = true;
            answer
        };
        let link = Arc::clone(self);
        let _ = std::thread::Builder::new().name("stash-link-join".into()).spawn(move || {
            let name = answer.phone.name.clone();
            let step = match link.join(&answer) {
                Ok(()) => PairStep::Done { phone_name: name },
                Err(e) => PairStep::Error { message: message_of(&e) },
            };
            link.set_pair(generation, step);
            link.inner.lock().unwrap().busy = false;
            let _ = link.refresh();
        });
    }

    fn join(&self, a: &Answer) -> Result<()> {
        let device = self.inner.lock().unwrap().kept.device.clone().expect("a device showed the code");
        let reply = seal(&a.kpair, "pair", 0, &place::pair_reply(&a.pair_id), &json!({ "kind": "stash-pair-reply", "v": 1 }))?;
        // One reply: a lost answer is tried again, and `used` after a lost try means that try landed.
        let mut lost = false;
        for attempt in 0.. {
            match call("POST", &format!("/pair/{}/reply", a.pair_id), Some(&device.auth()), Some(&json!({ "ct": reply }))) {
                Ok(_) => break,
                Err(e) if lost && code_of(&e) == Some("used") => break,
                Err(e) if e.downcast_ref::<Refused>().is_none() && attempt < 2 => {
                    lost = true;
                    std::thread::sleep(Duration::from_millis(1_500));
                }
                Err(e) => return Err(e),
            }
        }
        // The phone now adds this computer to its space (or makes the space): wait until the space lets it in.
        let until = std::time::Instant::now() + JOIN_WAIT;
        loop {
            match call("GET", &format!("/spaces/{}", a.space.id), Some(&device.auth()), None) {
                Ok(_) => break,
                Err(e) if code_of(&e) == Some("revoked") && std::time::Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(1_500));
                }
                Err(e) => return Err(e),
            }
        }
        let old = {
            let mut inner = self.inner.lock().unwrap();
            let old = inner.kept.space.take();
            let mut roster = BTreeMap::new();
            roster.insert(
                a.phone.device_id.clone(),
                RosterEntry { kind: "phone".into(), name: a.phone.name.clone(), public: b64u(&a.phone.public) },
            );
            roster.insert(
                device.id.clone(),
                RosterEntry { kind: "desktop".into(), name: device.name.clone(), public: device.public.clone() },
            );
            inner.kept.space = Some(Space {
                id: a.space.id.clone(),
                epoch: a.space.epoch,
                keys: BTreeMap::from([(a.space.epoch, b64u(&a.space.k))]),
                roster,
                linked_at: now_ms(),
            });
            let kept = std::mem::take(&mut inner.kept);
            self.save(&kept);
            inner.kept = kept;
            old
        };
        if let Some(old) = old.filter(|o| o.id != a.space.id) {
            let _ = call("DELETE", &format!("/spaces/{}/devices/me", old.id), Some(&device.auth()), None);
        }
        self.changed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap();
        inner.answer = None;
        if !matches!(inner.pair, PairStep::Done { .. }) {
            inner.pair = PairStep::Idle;
        }
    }
}

// ------------------------------------------------------------------------------------------------ the space

impl Link {
    /// Walks the key envelopes up to the space's epoch (§3.5). False when a step is missing: the link can't be read any more.
    /// A rotation that is due is left to the phone, which makes them: until then this computer waits.
    pub(crate) fn catch_up(&self, device: &Device, space: &mut Space, target: i64) -> Result<bool> {
        let key = device.key().ok_or_else(|| anyhow::anyhow!("no device key"))?;
        while space.epoch < target {
            let next = space.epoch + 1;
            let env = match call("GET", &format!("/spaces/{}/key/{next}", space.id), Some(&device.auth()), None) {
                Ok((_, body)) => serde_json::from_value::<Envelope>(body["ct"].clone())?,
                Err(e) if code_of(&e) == Some("no_key") => return Ok(false),
                Err(e) => return Err(e),
            };
            let prev = unb64(space.keys.get(&space.epoch).map(String::as_str).unwrap_or_default())?;
            let (k, epoch, _) = open_key_envelope(&env, &space.id, &device.id, &key, &prev, space.epoch)?;
            space.keys.insert(epoch, b64u(&k));
            space.epoch = epoch;
        }
        let keep_from = space.epoch - 1;
        space.keys.retain(|e, _| *e >= keep_from);
        Ok(true)
    }

    /// Reads the space: walks keys forward, opens every device's label (pinning new keys), and shows the list.
    fn refresh(&self) -> Result<()> {
        let Some((device, mut space)) = self.linked() else { return Ok(()) };
        let info = match call("GET", &format!("/spaces/{}", space.id), Some(&device.auth()), None) {
            Ok((_, info)) => info,
            Err(e) if code_of(&e) == Some("revoked") => {
                self.forget();
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let epoch = info["epoch"].as_i64().unwrap_or(space.epoch);
        if epoch > space.epoch && !self.catch_up(&device, &mut space, epoch)? {
            if info["rotationDue"].as_bool() != Some(true) {
                // No key for this computer: it can't read the space any more. Leave it, to be linked again.
                let _ = call("DELETE", &format!("/spaces/{}/devices/me", space.id), Some(&device.auth()), None);
                self.forget();
                return Ok(());
            }
            self.inner.lock().unwrap().problem = Some("Finishing the link. This takes a moment.".into());
        }
        let mut roster = BTreeMap::new();
        let mut devices = Vec::new();
        for d in info["devices"].as_array().cloned().unwrap_or_default() {
            let id = d["id"].as_str().unwrap_or_default().to_string();
            let kind = d["type"].as_str().unwrap_or_default().to_string();
            let last_seen_at = d["lastSeenAt"].as_i64().unwrap_or(0);
            if id == device.id {
                devices.push(DeviceView { id, kind, name: device.name.clone(), me: true, status: "ok", last_seen_at });
                roster.insert(device.id.clone(), RosterEntry { kind: "desktop".into(), name: device.name.clone(), public: device.public.clone() });
                continue;
            }
            let pinned = space.roster.get(&id).cloned();
            let label = serde_json::from_value::<Envelope>(d["labelCt"].clone()).ok().and_then(|env| {
                let key = space.data_key_at(env.e)?;
                read_label(&open(&key, &space.id, &place::label(&id), &env).ok()?, &id).ok()
            });
            let (status, name) = match (&label, &pinned) {
                (Some(l), Some(p)) if b64u(&l.public) != p.public => ("keyChanged", l.name.clone()),
                (Some(l), _) if l.kind != kind => ("unverified", l.name.clone()),
                (Some(l), p) => {
                    let public = p.as_ref().map(|p| p.public.clone()).unwrap_or_else(|| b64u(&l.public));
                    roster.insert(id.clone(), RosterEntry { kind: l.kind.clone(), name: l.name.clone(), public });
                    ("ok", l.name.clone())
                }
                (None, Some(p)) => {
                    roster.insert(id.clone(), p.clone());
                    ("ok", p.name.clone())
                }
                (None, None) => ("unverified", if kind == "phone" { "Phone".into() } else { "Browser".into() }),
            };
            devices.push(DeviceView { id, kind, name, me: false, status, last_seen_at });
        }
        space.roster = roster;
        self.put_space(space);
        let mut inner = self.inner.lock().unwrap();
        inner.devices = devices;
        Ok(())
    }

    fn unlink(&self) -> Result<()> {
        if let Some((device, space)) = self.linked() {
            match call("DELETE", &format!("/spaces/{}/devices/me", space.id), Some(&device.auth()), None) {
                Ok(_) => {}
                Err(e) if code_of(&e) == Some("revoked") => {}
                Err(e) => return Err(e),
            }
        }
        self.forget();
        self.inner.lock().unwrap().pair = PairStep::Idle;
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ routes

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/link") {
        return Ok(None);
    }
    let link = &state.link;
    let method = request.method().clone();
    let offline = state.offline.load(Ordering::Relaxed);
    let busy_reply = |link: &Arc<Link>, e: anyhow::Error| {
        link.inner.lock().unwrap().problem = Some(message_of(&e));
        reply_json(200, link.view())
    };
    Ok(Some(match (method, path) {
        (Method::Get, "/api/link") => reply_json(200, link.view()),
        (Method::Post, "/api/link/pair") if offline => {
            link.inner.lock().unwrap().pair =
                PairStep::Error { message: "Stash is set to work offline. Linking your phone needs the internet.".into() };
            reply_json(200, link.view())
        }
        (Method::Post, "/api/link/pair") => {
            link.start(0);
            reply_json(200, link.view())
        }
        (Method::Post, "/api/link/pair/stop") => {
            link.stop();
            reply_json(200, link.view())
        }
        (Method::Post, "/api/link/confirm") => {
            link.confirm();
            reply_json(200, link.view())
        }
        (Method::Post, "/api/link/cancel") => {
            link.start(0);
            reply_json(200, link.view())
        }
        (Method::Post, "/api/link/refresh") => {
            link.inner.lock().unwrap().problem = None;
            match link.refresh() {
                Ok(()) => reply_json(200, link.view()),
                Err(e) => busy_reply(link, e),
            }
        }
        (Method::Post, "/api/link/unlink") => match link.unlink() {
            Ok(()) => reply_json(200, link.view()),
            Err(e) => busy_reply(link, e),
        },
        _ => return Ok(None),
    }))
}
