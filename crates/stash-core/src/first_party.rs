//! Stash's own sources (Stash's Tidal): the signed config that names them, and Stash's own signature on what they
//! count.
//!
//! Stash publishes one config file, with its signature beside it (ECDSA P-256 over the exact bytes), that the phone app
//! reads too. It names Stash's Tidal add-on, the key Stash signs its requests with, and the access token in the
//! add-on's link. None of that is in the source: the config's address and the public key that checks it are given
//! when Stash is built, and a build without them has no Stash Tidal.
//!
//! What Stash's Tidal counts (a song played or saved, lossless or AAC) is counted per install: each install signs those
//! requests with its own id ([`Signing`]) and has a free allowance a day, the same as on the phone. A supporter's token
//! (the tip jar's, claimed by email) makes it unlimited. The add-on keeps the count; the host keeps what it last said,
//! and does not ask while the allowance is spent.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use url::Url;

use crate::addons::{Allowance, Signer, checked_url};

/// The largest config read (the phone app's limit too).
const CONFIG_CAP: u64 = 64 * 1024;
/// The most first-party add-ons one config may name.
const FIRST_PARTY_CAP: usize = 16;
/// How Stash names itself to its own servers: its add-ons tell Stash's requests from other apps' by this.
pub const USER_AGENT: &str = concat!("Stash/", env!("CARGO_PKG_VERSION"), " (desktop)");

/// The config, as far as the desktop reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// When it was published (seconds): a config older than one already read is refused, so an old copy cannot be
    /// replayed.
    pub updated_at: i64,
    /// The key Stash signs what its add-ons count with. Without it, Stash's Tidal is off.
    pub relay_key: Option<String>,
    /// Stash's Tidal server, https.
    pub tidal_relay_url: Option<String>,
    /// That server's access token, which its add-on's link carries.
    pub tidal_relay_token: Option<String>,
    /// The manifests of the first-party add-ons it names that are on, in its order.
    pub first_party: Vec<String>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    relay_key: Option<String>,
    #[serde(default)]
    tidal_relay_url: Option<String>,
    #[serde(default)]
    tidal_relay_token: Option<String>,
    #[serde(default)]
    default_addons: Option<serde_json::Value>,
}

/// A server's address as the config may give it: https (or http on this computer, for a test), with no user name, no
/// query and no trailing slash. Anything else is no address.
fn server(text: Option<String>) -> Option<String> {
    let text = text?;
    let url = checked_url(text.trim(), "the server").ok()?;
    if url.query().is_some() || url.fragment().is_some() || url.host_str().is_none() {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// A boolean the config gives, or `default` when it gives none; `None` when it gives something else.
fn flag(value: Option<&serde_json::Value>, default: bool) -> Option<bool> {
    match value {
        None => Some(default),
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => None,
    }
}

/// Text that is something: trimmed, not empty.
fn some_text(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

impl Config {
    /// The config from its text; `None` when it is not one. Fields it does not know are passed over, as the phone app
    /// passes them over.
    pub fn read(text: &str) -> Option<Config> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        if !value.is_object() {
            return None;
        }
        let raw: Raw = serde_json::from_value(value).ok()?;
        let first_party = raw
            .default_addons
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .take(FIRST_PARTY_CAP)
                    .filter_map(|entry| {
                        let entry = entry.as_object()?;
                        let enabled = flag(entry.get("enabled"), true)?;
                        let first = flag(entry.get("first_party"), false)?;
                        let manifest = entry.get("manifest")?.as_str()?;
                        let manifest = checked_url(manifest.trim(), "the manifest").ok()?;
                        (enabled && first).then(|| manifest.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Config {
            updated_at: raw.updated_at,
            relay_key: some_text(raw.relay_key),
            tidal_relay_url: server(raw.tidal_relay_url),
            tidal_relay_token: some_text(raw.tidal_relay_token),
            first_party,
        })
    }

    /// Where Stash's Tidal add-on may be, in the order to try: each first-party add-on the config names (the one
    /// whose manifest is Tidal's is it), then the add-on face of its Tidal server (`/v1/<token>/manifest.json`).
    /// Empty when the config switches Stash's Tidal off (no key to sign with, or no server).
    pub fn tidal_links(&self) -> Vec<String> {
        if self.relay_key.is_none() {
            return Vec::new();
        }
        let mut links = self.first_party.clone();
        if let (Some(base), Some(token)) = (&self.tidal_relay_url, &self.tidal_relay_token)
            && let Ok(mut url) = Url::parse(base)
        {
            let pushed = url
                .path_segments_mut()
                .map(|mut path| {
                    path.pop_if_empty()
                        .push("v1")
                        .push(token)
                        .push("manifest.json");
                })
                .is_ok();
            let link = url.to_string();
            if pushed && !links.contains(&link) {
                links.push(link);
            }
        }
        links
    }
}

/// The DER head of a P-256 public key as `SubjectPublicKeyInfo` (the form the config's key is published in); the
/// 65-byte point follows it.
const P256_SPKI_HEAD: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// Whether `signature` (base64 of an ASN.1 ECDSA P-256 signature over SHA-256) is `public_key`'s (base64 of its
/// `SubjectPublicKeyInfo`) over exactly these `bytes`.
pub fn verified(bytes: &[u8], signature: &str, public_key: &str) -> bool {
    let (Ok(key), Ok(signature)) = (
        STANDARD.decode(public_key.trim()),
        STANDARD.decode(signature.trim()),
    ) else {
        return false;
    };
    if key.len() != P256_SPKI_HEAD.len() + 65 || key[..P256_SPKI_HEAD.len()] != P256_SPKI_HEAD {
        return false;
    }
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::ECDSA_P256_SHA256_ASN1,
        &key[P256_SPKI_HEAD.len()..],
    )
    .verify(bytes, &signature)
    .is_ok()
}

/// Reads the config at `url` and its signature beside it (`<url>.sig`), and checks it with `public_key`. Answers its
/// text as it came, and what it says. Refused when the signature is not right, or when it is older than `floor` (the
/// `updated_at` of the one read before): a good signature does not stop an old copy being served again. The words of
/// a failure never name the address.
pub fn fetch(url: &str, public_key: &str, floor: i64) -> Result<(String, Config), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .into();
    let get = |url: &str| -> Result<Vec<u8>, String> {
        let mut answer = agent
            .get(url)
            .call()
            .map_err(|_| "Stash's servers could not be reached".to_string())?;
        let status = answer.status().as_u16();
        if status != 200 {
            return Err(format!("Stash's servers answered {status}"));
        }
        let bytes = answer
            .body_mut()
            .with_config()
            .limit(CONFIG_CAP + 1)
            .read_to_vec()
            .map_err(|_| "Stash's config could not be read".to_string())?;
        if bytes.len() as u64 > CONFIG_CAP {
            return Err("Stash's config is too large".into());
        }
        Ok(bytes)
    };
    let bytes = get(url)?;
    let signature = get(&format!("{url}.sig"))?;
    // Checked over the bytes exactly as they came, never as read back.
    if !verified(&bytes, &String::from_utf8_lossy(&signature), public_key) {
        return Err("Stash's config is not signed right".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "Stash's config is not text".to_string())?;
    let config =
        Config::read(&text).ok_or_else(|| "Stash's config could not be read".to_string())?;
    if config.updated_at < floor {
        return Err("Stash's config is older than the one this computer has".into());
    }
    Ok((text, config))
}

/// The signature over a request about `what` (a track's id as it is sent, or `quota`) by `install` at `ts` (seconds):
/// hex HMAC-SHA256 with `key` (its UTF-8 bytes) over `<install>:<what>:<ts>`, as the phone app signs.
pub fn signature(key: &str, install: &str, what: &str, ts: i64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    let tag = ring::hmac::sign(&key, format!("{install}:{what}:{ts}").as_bytes());
    tag.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A new install id: random, in a UUID's form (what the phone app makes, and what Stash's servers take).
pub fn new_install_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the system gives random bytes");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Whether `id` is an install id Stash's servers take.
pub fn install_id_ok(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn system_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The next UTC midnight after `secs`: when Stash's servers start a new day.
fn next_day(secs: i64) -> i64 {
    (secs.div_euclid(86_400) + 1) * 86_400
}

impl Allowance {
    /// The allowance as it stands at `now` (seconds): a day that has ended since it was told reads nothing used.
    pub fn as_of(&self, now: i64) -> Allowance {
        if now < self.resets_at {
            return self.clone();
        }
        Allowance {
            used: 0,
            resets_at: next_day(now),
            ..self.clone()
        }
    }

    /// Whether it is spent (at `now`): a limit, reached, in a day that has not ended.
    pub fn spent(&self, now: i64) -> bool {
        self.limit.is_some_and(|limit| self.used >= limit) && now < self.resets_at
    }
}

/// Stash's own signature for one install, and what its add-on last said of the allowance.
pub struct Signing {
    install: String,
    key: RwLock<Option<String>>,
    /// The supporter's token, when one is kept: sent with what is counted, and the allowance is unlimited.
    entitlement: RwLock<Option<String>>,
    /// The add-on's clock less this computer's, in seconds, learned from a refusal that gave its time.
    offset: AtomicI64,
    told: Mutex<Option<Allowance>>,
    /// The add-on refused Stash's access token (a 401 that did not give its clock): the config may be newer.
    refused: AtomicBool,
}

impl Signing {
    pub fn new(install: String) -> Self {
        Self {
            install,
            key: RwLock::new(None),
            entitlement: RwLock::new(None),
            offset: AtomicI64::new(0),
            told: Mutex::new(None),
            refused: AtomicBool::new(false),
        }
    }

    /// The key to sign with, from the config (none: nothing is signed).
    pub fn set_key(&self, key: Option<String>) {
        *self.key.write().unwrap() = key;
    }

    /// The supporter's token (none: the free allowance). What the add-on said of the allowance before is forgotten
    /// when it changes: it is asked again.
    pub fn set_entitlement(&self, token: Option<String>) -> bool {
        let token = token
            .filter(|t| !t.is_empty() && t.len() < 512 && t.bytes().all(|b| b.is_ascii_graphic()));
        let mut kept = self.entitlement.write().unwrap();
        if *kept == token {
            return false;
        }
        *kept = token;
        *self.told.lock().unwrap() = None;
        true
    }

    /// Whether a supporter's token is sent.
    pub fn entitled(&self) -> bool {
        self.entitlement.read().unwrap().is_some()
    }

    /// The time on the add-on's clock, in seconds.
    pub fn now(&self) -> i64 {
        system_secs() + self.offset.load(Ordering::Relaxed)
    }

    /// A time on the add-on's clock (seconds), on this computer's.
    pub fn on_this_clock(&self, secs: i64) -> i64 {
        secs - self.offset.load(Ordering::Relaxed)
    }

    /// The allowance as the add-on last said it, as it stands now; `None` before it said.
    pub fn allowance(&self) -> Option<Allowance> {
        let now = self.now();
        self.told.lock().unwrap().as_ref().map(|a| a.as_of(now))
    }

    /// Whether the add-on refused Stash's access token since this was last asked.
    pub fn take_refused(&self) -> bool {
        self.refused.swap(false, Ordering::Relaxed)
    }
}

impl Signer for Signing {
    fn headers(&self, what: &str) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();
        if let Some(key) = self.key.read().unwrap().as_deref() {
            let ts = self.now();
            headers.push(("X-Stash-Install", self.install.clone()));
            headers.push(("X-Stash-Ts", ts.to_string()));
            headers.push(("X-Stash-Auth", signature(key, &self.install, what, ts)));
        }
        if let Some(token) = self.entitlement.read().unwrap().as_deref() {
            headers.push(("X-Stash-Entitlement", token.to_string()));
        }
        headers
    }

    fn clock_from(&self, server_secs: i64) {
        self.offset
            .store(server_secs - system_secs(), Ordering::Relaxed);
    }

    fn allowance(&self, told: &Allowance) {
        *self.told.lock().unwrap() = Some(told.clone());
    }

    fn spent_for(&self) -> Option<u32> {
        let now = self.now();
        let told = self.told.lock().unwrap();
        let told = told.as_ref().filter(|a| a.spent(now))?;
        Some((told.resets_at - now).clamp(1, 86_400) as u32)
    }

    fn refused(&self) {
        self.refused.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    /// A key pair, and its public key as the config's key is published (base64 `SubjectPublicKeyInfo`).
    fn keys() -> (EcdsaKeyPair, String) {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let mut spki = P256_SPKI_HEAD.to_vec();
        spki.extend_from_slice(pair.public_key().as_ref());
        (pair, STANDARD.encode(spki))
    }

    #[test]
    fn a_config_is_believed_only_as_its_key_signed_it() {
        let (pair, public) = keys();
        let bytes = br#"{"v":1,"updated_at":5,"relay_key":"k"}"#;
        let signed = STANDARD.encode(pair.sign(&SystemRandom::new(), bytes).unwrap().as_ref());
        assert!(verified(bytes, &signed, &public));
        assert!(verified(bytes, &format!("{signed}\n"), &public));
        let mut changed = bytes.to_vec();
        changed[20] = b'6';
        assert!(!verified(&changed, &signed, &public));
        let (_, other) = keys();
        assert!(!verified(bytes, &signed, &other));
        assert!(!verified(bytes, "not base64!", &public));
        assert!(!verified(bytes, &signed, "AAAA"));
    }

    #[test]
    fn a_config_names_stash_s_tidal_add_on_only_with_a_key_to_sign_with() {
        let text = r#"{"v":1,"updated_at":9,"relays":[],"relay_key":" k ","tidal_relay_url":"https://tidal.example/",
            "tidal_relay_token":"t/k n","default_addons":[
              {"manifest":"https://a.example/v1/x/manifest.json","enabled":true,"first_party":true},
              {"manifest":"https://b.example/manifest.json"},
              {"manifest":"https://c.example/manifest.json","first_party":true,"enabled":false},
              {"manifest":"http://d.example/manifest.json","first_party":true},
              {"manifest":"https://e.example/manifest.json","first_party":"yes"}],"later":{"x":1}}"#;
        let config = Config::read(text).unwrap();
        assert_eq!(config.updated_at, 9);
        assert_eq!(config.relay_key.as_deref(), Some("k"));
        assert_eq!(
            config.tidal_relay_url.as_deref(),
            Some("https://tidal.example")
        );
        assert_eq!(
            config.tidal_links(),
            vec![
                "https://a.example/v1/x/manifest.json".to_string(),
                "https://tidal.example/v1/t%2Fk%20n/manifest.json".to_string(),
            ]
        );
        let without_key = Config {
            relay_key: None,
            ..config.clone()
        };
        assert!(without_key.tidal_links().is_empty());
        // Plain http is no server (on this computer it is, for a test).
        let plain = Config::read(
            r#"{"relay_key":"k","tidal_relay_url":"http://tidal.example","tidal_relay_token":"t"}"#,
        )
        .unwrap();
        assert!(plain.tidal_links().is_empty());
        let here = Config::read(
            r#"{"relay_key":"k","tidal_relay_url":"http://127.0.0.1:9/x","tidal_relay_token":"t"}"#,
        )
        .unwrap();
        assert_eq!(
            here.tidal_links(),
            vec!["http://127.0.0.1:9/x/v1/t/manifest.json".to_string()]
        );
        assert!(Config::read("[]").is_none());
    }

    #[test]
    fn requests_are_signed_as_the_phone_app_signs_them() {
        // HMAC-SHA256("relay-key", "inst-1234:td:42:1700000000"), as Node's createHmac and the phone's Mac give it.
        assert_eq!(
            signature("relay-key", "inst-1234", "td:42", 1_700_000_000),
            "f39cf3afcceaa64c88884dbad04cfad008c82e4f735bf46d924b7e7261fcef29"
        );
        let id = new_install_id();
        assert!(install_id_ok(&id) && id.len() == 36, "{id}");
        assert_ne!(id, new_install_id());

        let signing = Signing::new("inst-1234".into());
        assert!(
            signing.headers("td:42").is_empty(),
            "no key: nothing is signed"
        );
        signing.set_key(Some("relay-key".into()));
        signing.set_entitlement(Some("supporter.token".into()));
        let headers = signing.headers("td:42");
        let named = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
        };
        let ts: i64 = named("X-Stash-Ts").unwrap().parse().unwrap();
        assert_eq!(named("X-Stash-Install").as_deref(), Some("inst-1234"));
        assert_eq!(
            named("X-Stash-Auth"),
            Some(signature("relay-key", "inst-1234", "td:42", ts))
        );
        assert_eq!(
            named("X-Stash-Entitlement").as_deref(),
            Some("supporter.token")
        );
        // A clock the add-on said is used from then on.
        signing.clock_from(system_secs() + 3_600);
        let later: i64 = signing
            .headers("quota")
            .iter()
            .find(|(n, _)| *n == "X-Stash-Ts")
            .unwrap()
            .1
            .parse()
            .unwrap();
        assert!((later - ts - 3_600).abs() <= 2);
    }

    #[test]
    fn a_spent_allowance_rests_until_the_day_turns_over() {
        let signing = Signing::new("inst-1234".into());
        assert_eq!(signing.spent_for(), None);
        let now = signing.now();
        let reset = next_day(now);
        Signer::allowance(
            &signing,
            &Allowance {
                used: 12,
                limit: Some(300),
                resets_at: reset,
                label: None,
            },
        );
        assert_eq!(signing.spent_for(), None);
        Signer::allowance(
            &signing,
            &Allowance {
                used: 300,
                limit: Some(300),
                resets_at: reset,
                label: None,
            },
        );
        let wait = signing.spent_for().unwrap() as i64;
        assert!((wait - (reset - now)).abs() <= 2);
        // A day that has ended reads nothing used.
        Signer::allowance(
            &signing,
            &Allowance {
                used: 300,
                limit: Some(300),
                resets_at: now - 5,
                label: None,
            },
        );
        assert_eq!(signing.spent_for(), None);
        let fresh = signing.allowance().unwrap();
        assert_eq!((fresh.used, fresh.resets_at), (0, next_day(signing.now())));
        // Unlimited is never spent; a new supporter token forgets what was said.
        Signer::allowance(
            &signing,
            &Allowance {
                used: 9_000,
                limit: None,
                resets_at: reset,
                label: None,
            },
        );
        assert_eq!(signing.spent_for(), None);
        assert!(signing.set_entitlement(Some("t".into())));
        assert!(!signing.set_entitlement(Some("t".into())));
        assert!(signing.allowance().is_none());
        // A token that could not go in a header is no token.
        assert!(signing.set_entitlement(Some("bad token".into())));
        assert!(!signing.entitled());
    }
}
