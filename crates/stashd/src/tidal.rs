//! Stash's own Tidal: lossless and Hi-Res from Tidal through Stash's Tidal add-on, for everyone, with the same free
//! allowance a day as on the phone, and unlimited for supporters.
//!
//! The add-on is found through Stash's signed config (see [`stash_core::first_party`]), whose address and key are given
//! when Stash is built (`STASH_LOSSLESS_CONFIG_URL`, `STASH_LOSSLESS_CONFIG_PUBKEY`); a build without them has no Stash
//! Tidal. The config is read at start and every 6 hours (15 minutes after a failure), and kept, so Stash's Tidal is
//! there offline too. The add-on's link carries Stash's access token: it is kept here and never shown.
//!
//! Every song played or saved from it is signed with this computer's own install id, which the add-on counts each
//! day; a supporter's token (claimed in Settings, see [`crate::away`]) goes with it and makes it unlimited. What the
//! add-on last said of the allowance is kept, and while it is spent Stash's Tidal is not asked for songs: they come from
//! the listener's other sources until the day turns over.
//!
//! It is one of the listener's sources, after the add-ons they added and their Mornye extensions, before YouTube Music;
//! it can be switched off, not removed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::{Addon, Code, Signer};
use stash_core::first_party::{self, Config, Signing};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, reply_json};

/// Its id among the listener's sources (not the add-on's own, which another add-on could share).
pub(crate) const ID: &str = "stash.tidal";
/// "1" while the listener has it switched off.
pub(crate) const OFF: &str = "tidal.off";
/// The config as it came (its signature was checked before it was kept).
const CONFIG: &str = "stash.config";
/// The add-on's link and manifest, as last read.
const FOUND: &str = "tidal.found";
/// This computer's install id, which Stash's add-ons count by.
const INSTALL: &str = "stash.install";
/// The add-on's own id, which says the manifest is Stash's Tidal.
const TIDAL_MANIFEST_ID: &str = "tidal";
/// A config read is read again this often; one that could not be read, sooner.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);
const LOOK_AGAIN: Duration = Duration::from_secs(15 * 60);
/// The allowance is asked of the add-on at most this often while Settings shows it (each song played tells it too).
const ALLOWANCE_EVERY: Duration = Duration::from_secs(5 * 60);
/// After the add-on refused Stash's access token, the config is read again (it may give a new one), at most this often.
const REFUSED_LOOK: Duration = Duration::from_secs(10 * 60);

/// Where the config is, and the key that checks it, from the environment (for trying another) or as built.
fn built() -> Option<(String, String)> {
    let named = |name: &str, built: Option<&'static str>| {
        std::env::var(name)
            .ok()
            .or_else(|| built.map(str::to_string))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let url = named(
        "STASH_LOSSLESS_CONFIG_URL",
        option_env!("STASH_LOSSLESS_CONFIG_URL"),
    )?;
    let key = named(
        "STASH_LOSSLESS_CONFIG_PUBKEY",
        option_env!("STASH_LOSSLESS_CONFIG_PUBKEY"),
    )?;
    usable(&url).then_some((url, key))
}

/// An https address, or an http one on this computer (a test's).
fn usable(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "https" => u.host_str().is_some(),
        Ok(u) if u.scheme() == "http" => {
            matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        }
        _ => false,
    }
}

/// The add-on as it was last read: its link (which carries Stash's access token) and its manifest, under [`ID`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct Found {
    link: String,
    manifest: String,
}

pub(crate) struct Tidal {
    /// Where the config is and the key that checks it; none in a build without them.
    from: RwLock<Option<(String, String)>>,
    pub(crate) signing: Arc<Signing>,
    config: RwLock<Option<Config>>,
    found: RwLock<Option<Found>>,
    /// Why it cannot be used, when it cannot.
    trouble: Mutex<Option<String>>,
    look_now: AtomicBool,
    ask_allowance: AtomicBool,
    allowance_asked: Mutex<Option<Instant>>,
}

fn setting(library: &Library, key: &str) -> Option<String> {
    library
        .setting(key)
        .ok()
        .flatten()
        .filter(|v| !v.is_empty())
}

impl Tidal {
    /// Stash's Tidal as this computer kept it: its install id (made the first time), the config and the add-on read
    /// last, and the supporter's token.
    pub(crate) fn open(library: &mut Library) -> Self {
        let install = match setting(library, INSTALL).filter(|id| first_party::install_id_ok(id)) {
            Some(id) => id,
            None => {
                let id = first_party::new_install_id();
                let _ = library.set_setting(INSTALL, &id);
                id
            }
        };
        let signing = Arc::new(Signing::new(install));
        signing.set_entitlement(setting(library, crate::away::SUPPORTER_TOKEN));
        let config = setting(library, CONFIG).and_then(|text| Config::read(&text));
        if let Some(config) = &config {
            signing.set_key(config.relay_key.clone());
        }
        let found = setting(library, FOUND).and_then(|text| serde_json::from_str(&text).ok());
        Self {
            from: RwLock::new(built()),
            signing,
            config: RwLock::new(config),
            found: RwLock::new(found),
            trouble: Mutex::default(),
            look_now: AtomicBool::new(true),
            ask_allowance: AtomicBool::new(true),
            allowance_asked: Mutex::default(),
        }
    }

    /// Reads the config at `url`, checked with `key`, in place of the one this build names (a test's).
    pub(crate) fn read_config_from(&self, url: &str, key: &str) {
        *self.from.write().unwrap() = Some((url.to_string(), key.to_string()));
        self.look_now.store(true, Ordering::Relaxed);
    }

    /// Whether this build can have Stash's Tidal at all.
    fn available(&self) -> bool {
        self.from.read().unwrap().is_some()
    }

    /// Stash's Tidal as a source, ready to be asked; none while it is not known (no config, or it names none).
    pub(crate) fn source(&self) -> Option<Addon> {
        if !self.available() {
            return None;
        }
        let named = self
            .config
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|c| !c.tidal_links().is_empty());
        let found = self.found.read().unwrap().clone().filter(|_| named)?;
        let addon = Addon::from_stored(&found.link, &found.manifest).ok()?;
        Some(addon.signed_by(Arc::clone(&self.signing) as Arc<dyn Signer>))
    }

    /// The supporter's token changed (claimed, or let go): the allowance is asked again.
    pub(crate) fn supporter(&self, token: Option<String>) {
        if self.signing.set_entitlement(token) {
            self.ask_allowance.store(true, Ordering::Relaxed);
        }
    }

    /// Reads the config, and the add-on it names. Answers whether the config was read, and whether the add-on as the
    /// listener's sources hold it changed.
    fn look(&self, library: &Mutex<Library>) -> (bool, bool) {
        let Some((url, key)) = self.from.read().unwrap().clone() else {
            return (false, false);
        };
        let floor = self
            .config
            .read()
            .unwrap()
            .as_ref()
            .map_or(0, |c| c.updated_at);
        let (text, config) = match first_party::fetch(&url, &key, floor) {
            Ok(read) => read,
            Err(why) => {
                // What was read before still holds; the trouble is said only when there is nothing.
                if self.found.read().unwrap().is_none() {
                    *self.trouble.lock().unwrap() = Some(why);
                }
                return (false, false);
            }
        };
        let _ = library.lock().unwrap().set_setting(CONFIG, &text);
        self.signing.set_key(config.relay_key.clone());
        let links = config.tidal_links();
        *self.config.write().unwrap() = Some(config);
        let before = self.found.read().unwrap().clone();
        let mut trouble = None;
        let mut found = None;
        if links.is_empty() {
            trouble = Some("Stash's Tidal is switched off for now".to_string());
        }
        for link in &links {
            match Addon::from_own_link(link) {
                Ok(addon)
                    if addon.manifest.id == TIDAL_MANIFEST_ID
                        && addon.offers_resource("stream") =>
                {
                    let mut manifest = addon.manifest.clone();
                    manifest.id = ID.to_string();
                    found = serde_json::to_string(&manifest).ok().map(|manifest| Found {
                        link: link.clone(),
                        manifest,
                    });
                    trouble = None;
                    break;
                }
                Ok(_) => {
                    trouble =
                        Some("Stash's config names something that is not Stash's Tidal".into())
                }
                Err(e) => trouble = Some(e.in_words()),
            }
        }
        // Not reached now: the add-on read before stays, as long as the config still names it.
        if found.is_none()
            && let Some(before) = &before
            && links.contains(&before.link)
        {
            found = Some(before.clone());
        }
        *self.trouble.lock().unwrap() = trouble;
        let changed = found != before;
        if changed {
            let kept = found
                .as_ref()
                .and_then(|f| serde_json::to_string(f).ok())
                .unwrap_or_default();
            let _ = library.lock().unwrap().set_setting(FOUND, &kept);
            *self.found.write().unwrap() = found;
        }
        (true, changed)
    }

    /// Asks the add-on how the listener's allowance stands (it is told with every song too).
    fn ask(&self, state: &State) {
        *self.allowance_asked.lock().unwrap() = Some(Instant::now());
        let loaded = state
            .addons
            .read()
            .unwrap()
            .iter()
            .find(|a| a.stored.id == ID)
            .cloned();
        if let Some(addon) = loaded.as_ref().and_then(|l| l.usable()) {
            match addon.allowance() {
                Ok(_) => {}
                // An add-on that keeps no count says so: there is nothing to show.
                Err(e) if e.code == Code::NotFound => {}
                Err(e) => state.lacks.troubled(ID, &e),
            }
        }
    }

    /// How it stands, for Settings.
    fn status(&self, state: &State) -> Value {
        let on = setting(&state.library.lock().unwrap(), OFF).as_deref() != Some("1");
        let ready = self.source().is_some();
        // Asked again when Settings looks and it was not asked for a while.
        let stale = self
            .allowance_asked
            .lock()
            .unwrap()
            .is_none_or(|at| at.elapsed() >= ALLOWANCE_EVERY);
        if ready && on && stale {
            self.ask_allowance.store(true, Ordering::Relaxed);
        }
        let allowance = self.signing.allowance().map(|a| {
            json!({
                "used": a.used,
                "limit": a.limit,
                "resets_at_ms": self.signing.on_this_clock(a.resets_at) * 1000,
            })
        });
        json!({
            "available": self.available(),
            "ready": ready,
            "on": on,
            "trouble": if ready { None } else { self.trouble.lock().unwrap().clone() },
            "allowance": allowance,
            "supporter": self.signing.entitled(),
            "can_claim": state.away.can_claim(),
            "claiming": state.away.claiming(),
        })
    }
}

/// Reads the config (and the add-on) as it falls due, and the allowance when it is asked for, until Stash stops.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let tidal = &state.tidal;
    let mut next_look = Instant::now();
    let mut refused_at: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let online = !state.offline.load(Ordering::Relaxed);
        if tidal.signing.take_refused() && refused_at.is_none_or(|at| at.elapsed() >= REFUSED_LOOK)
        {
            refused_at = Some(Instant::now());
            tidal.look_now.store(true, Ordering::Relaxed);
        }
        if online
            && tidal.available()
            && (tidal.look_now.swap(false, Ordering::Relaxed) || Instant::now() >= next_look)
        {
            let (read, changed) = tidal.look(&state.library);
            next_look = Instant::now() + if read { LOOK_EVERY } else { LOOK_AGAIN };
            if changed {
                if let Err(e) = state.reload_addons() {
                    eprintln!("Stash's Tidal could not be listed: {e:#}");
                }
                state.lacks.forget_addon(ID);
                state.saving.ring();
                tidal.ask_allowance.store(true, Ordering::Relaxed);
            }
        }
        if online && tidal.ask_allowance.swap(false, Ordering::Relaxed) {
            tidal.ask(state);
        }
        for _ in 0..4 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// Settings' route for Stash's Tidal; `None`: the path is not it.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    match (request.method(), path) {
        (Method::Get, "/api/tidal") => Ok(Some(reply_json(200, state.tidal.status(state)))),
        _ => Ok(None),
    }
}
