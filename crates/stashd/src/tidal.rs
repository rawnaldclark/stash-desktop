//! Stash's own Tidal: lossless and Hi-Res from Tidal through Stash's Tidal add-on, an add-on the listener chooses to
//! add (Settings › Add-ons offers it), with the same free allowance a day as on the phone, unlimited for supporters.
//!
//! Until it is added, nothing is asked of Stash's servers. Adding it reads Stash's signed config (see
//! [`stash_core::first_party`]), whose address and key are given when Stash is built (`STASH_LOSSLESS_CONFIG_URL`,
//! `STASH_LOSSLESS_CONFIG_PUBKEY`; a build without them does not offer it), and the add-on the config names. It is then
//! one of the listener's add-ons, kept in the add-ons table under [`ID`] like the others: in their order, switched off
//! or removed as they choose. Its row's link is [`LINK`], not the add-on's: that carries Stash's access token, and is
//! kept apart, never shown. While it is added, the config is read again every 6 hours (15 minutes after a failure).
//!
//! Every song played or saved from it is signed with this computer's own install id, which the add-on counts each
//! day; a supporter's token (claimed in Settings, see [`crate::away`]) goes with it and makes it unlimited. What the
//! add-on last said of the allowance is kept, and while it is spent Stash's Tidal is not asked for songs: they come from
//! the listener's other sources until the day turns over.
//!
//! When the add-on offers its quick check (a Cloudflare Turnstile page that a person passes once), Add opens it in a
//! window of its own; passing it gives this computer its own pass, which takes the access token's place in the add-on's
//! link from then on (see [`stash_core::first_party::register`]). Until the add-on offers it, the access token is used
//! as before. A pass the add-on refuses is let go, and Settings offers the check again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::{Addon, Code, Signer};
use stash_core::first_party::{self, Config, Signing};
use tiny_http::{Method, Request};

use crate::mornye::{VerifyShell, VerifyWindow};
use crate::serve::{Reply, State, offline, refuse, reply_json};

/// Its id among the listener's add-ons (not the add-on's own, which another add-on could share).
pub(crate) const ID: &str = "stash.tidal";
/// What its row in the add-ons table keeps as its link.
pub(crate) const LINK: &str = "stash:tidal";
/// What an added Stash's Tidal says while the add-on it names is not at hand.
pub(crate) const NOT_AT_HAND: &str = "Stash's Tidal is not available right now";
/// The config as it came (its signature was checked before it was kept).
const CONFIG: &str = "stash.config";
/// The add-on's link and manifest, as last read.
const FOUND: &str = "tidal.found";
/// This computer's install id, which Stash's add-ons count by.
const INSTALL: &str = "stash.install";
/// This computer's own pass, from the add-on's check.
const PASS: &str = "stash.tidal.pass";
/// What the check window is called.
const CHECK_TITLE: &str = "Add Tidal to Stash";
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
    /// When the config is read again.
    next_look: Mutex<Instant>,
    ask_allowance: AtomicBool,
    allowance_asked: Mutex<Option<Instant>>,
    /// This computer's own pass (none: Stash's access token, as before).
    pass: RwLock<Option<String>>,
    /// Opens the check's window; none in a core without one (the access token is then used).
    pub(crate) shell: OnceLock<VerifyShell>,
    /// The check's window is open.
    checking: Arc<AtomicBool>,
    /// What the check's window came back with (the pass, or words), for [`run`] to take.
    handed: Arc<Mutex<Option<Result<String, String>>>>,
    /// Why the last check did not go through.
    check_trouble: Mutex<Option<String>>,
    /// The add-on refused this computer's pass: Settings offers the check again.
    needs_check: AtomicBool,
}

/// Whether the listener has added Stash's Tidal.
fn added(state: &State) -> bool {
    state
        .addons
        .read()
        .unwrap()
        .iter()
        .any(|a| a.stored.id == ID)
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
        let pass = setting(library, PASS).filter(|p| first_party::pass_ok(p));
        Self {
            pass: RwLock::new(pass),
            shell: OnceLock::new(),
            checking: Arc::default(),
            handed: Arc::default(),
            check_trouble: Mutex::default(),
            needs_check: AtomicBool::new(false),
            from: RwLock::new(built()),
            signing,
            config: RwLock::new(config),
            found: RwLock::new(found),
            trouble: Mutex::default(),
            look_now: AtomicBool::new(true),
            next_look: Mutex::new(Instant::now()),
            ask_allowance: AtomicBool::new(true),
            allowance_asked: Mutex::default(),
        }
    }

    /// Reads the config at `url`, checked with `key`, in place of the one this build names (a test's).
    pub(crate) fn read_config_from(&self, url: &str, key: &str) {
        *self.from.write().unwrap() = Some((url.to_string(), key.to_string()));
        self.look_now.store(true, Ordering::Relaxed);
    }

    /// Whether this build offers Stash's Tidal at all.
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
        // This computer's own pass, where it has one, in the access token's place.
        let link = self
            .pass
            .read()
            .unwrap()
            .as_deref()
            .and_then(|pass| first_party::pass_link(&found.link, pass))
            .unwrap_or(found.link);
        let addon = Addon::from_stored(&link, &found.manifest).ok()?;
        Some(addon.signed_by(Arc::clone(&self.signing) as Arc<dyn Signer>))
    }

    fn has_pass(&self) -> bool {
        self.pass.read().unwrap().is_some()
    }

    /// Keeps this computer's pass (none: lets it go, and the access token is used again).
    fn keep_pass(&self, library: &Mutex<Library>, pass: Option<String>) {
        let _ = library
            .lock()
            .unwrap()
            .set_setting(PASS, pass.as_deref().unwrap_or(""));
        if pass.is_some() {
            self.needs_check.store(false, Ordering::Relaxed);
        }
        *self.pass.write().unwrap() = pass;
    }

    /// The add-on's check page, when this computer has no pass yet, a window can open it, and the add-on offers it.
    fn check_due(&self) -> Option<(VerifyShell, String)> {
        if self.has_pass() {
            return None;
        }
        let shell = self.shell.get()?.clone();
        let link = self.found.read().unwrap().as_ref()?.link.clone();
        let page = first_party::register_page(&link)?;
        // Not offered yet, or not reached: Stash's access token, as before.
        matches!(first_party::check_offered(&page), Ok(true)).then_some((shell, page))
    }

    /// Opens the check's window on `page`. Passing it hands the pass to [`run`], which keeps it (and finishes adding
    /// Stash's Tidal when that is what asked).
    fn open_check(&self, shell: &VerifyShell, page: String) -> Result<(), String> {
        if self.checking.swap(true, Ordering::Relaxed) {
            return Ok(()); // one at a time: the open window is the one
        }
        *self.check_trouble.lock().unwrap() = None;
        let catches = {
            let page = page.clone();
            Arc::new(move |url: &str| first_party::is_hand_back(&page, url))
        };
        let caught = {
            let (handed, page) = (Arc::clone(&self.handed), page.clone());
            Arc::new(move |url: String| {
                let outcome = match first_party::turnstile_token(&page, &url) {
                    Some(token) => first_party::register(&page, &token),
                    None => Err("The check didn't finish. Press Add to try again.".to_string()),
                };
                *handed.lock().unwrap() = Some(outcome);
            })
        };
        let closed = {
            let checking = Arc::clone(&self.checking);
            Box::new(move || checking.store(false, Ordering::Relaxed))
        };
        let opened = shell(VerifyWindow {
            url: page,
            title: CHECK_TITLE.to_string(),
            catches,
            caught,
            closed,
        });
        if !opened {
            self.checking.store(false, Ordering::Relaxed);
            return Err("The check's window did not open".into());
        }
        Ok(())
    }

    /// Settings' "Check": opens the check again for an added Stash's Tidal whose pass was refused.
    fn check(&self, state: &State) -> Result<Reply> {
        if offline(state) {
            return Ok(refuse(409, "Stash is offline"));
        }
        let Some((shell, page)) = self.check_due() else {
            return Ok(reply_json(200, self.status(state)));
        };
        if let Err(why) = self.open_check(&shell, page) {
            return Ok(refuse(502, &why));
        }
        Ok(reply_json(200, self.status(state)))
    }

    /// The supporter's token changed (claimed, or let go): the allowance is asked again.
    pub(crate) fn supporter(&self, token: Option<String>) {
        if self.signing.set_entitlement(token) {
            self.ask_allowance.store(true, Ordering::Relaxed);
        }
    }

    /// Reads the config, and the add-on it names (kept for the add-ons table's row, when there is one). Answers whether
    /// the add-on changed.
    fn look(&self, library: &Mutex<Library>) -> bool {
        let Some((url, key)) = self.from.read().unwrap().clone() else {
            return false;
        };
        let (read, changed) = self.read(library, &url, &key);
        *self.next_look.lock().unwrap() =
            Instant::now() + if read { LOOK_EVERY } else { LOOK_AGAIN };
        changed
    }

    /// [`Tidal::look`]'s reading: whether the config was read, and whether the add-on changed.
    fn read(&self, library: &Mutex<Library>, url: &str, key: &str) -> (bool, bool) {
        let floor = self
            .config
            .read()
            .unwrap()
            .as_ref()
            .map_or(0, |c| c.updated_at);
        let (text, config) = match first_party::fetch(url, key, floor) {
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
            let mut library = library.lock().unwrap();
            let _ = library.set_setting(FOUND, &kept);
            // The listing keeps the add-on's manifest as it now is (its place and switch stay).
            if let Some(found) = &found
                && library
                    .addons()
                    .is_ok_and(|kept| kept.iter().any(|a| a.id == ID))
            {
                let _ = library.save_addon(ID, LINK, &found.manifest);
            }
            drop(library);
            *self.found.write().unwrap() = found;
        }
        (true, changed)
    }

    /// Adds Stash's Tidal to the listener's add-ons (last, as an add-on added by its link goes): reads the config now,
    /// and the add-on it names.
    fn add(&self, state: &State) -> Result<Reply> {
        if !self.available() {
            return Ok(refuse(404, "this copy of Stash does not offer Tidal"));
        }
        if offline(state) {
            return Ok(refuse(
                409,
                "Stash is offline: Tidal is found online when it is added",
            ));
        }
        self.look(&state.library);
        let Some(found) = self.found.read().unwrap().clone() else {
            let why = self
                .trouble
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| NOT_AT_HAND.to_string());
            return Ok(refuse(502, &why));
        };
        // A computer without a pass passes the add-on's check first, when the add-on offers it: adding finishes in
        // [`run`] once the check is passed (Settings reads `checking` meanwhile).
        if let Some((shell, page)) = self.check_due() {
            if let Err(why) = self.open_check(&shell, page) {
                return Ok(refuse(502, &why));
            }
            return Ok(state.addons_reply());
        }
        self.finish_add(state, &found)?;
        Ok(state.addons_reply())
    }

    /// Puts Stash's Tidal in the listener's add-ons (last; one already there keeps its place and switch), and reads
    /// them again.
    fn finish_add(&self, state: &State, found: &Found) -> Result<()> {
        state
            .library
            .lock()
            .unwrap()
            .save_addon(ID, LINK, &found.manifest)?;
        state.reload_addons()?;
        state.lacks.forget_addon(ID);
        state.saving.release();
        state.saving.ring();
        self.ask_allowance.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Takes what the check's window came back with: a pass is kept and Stash's Tidal is added (or read again with
    /// it); words are kept for Settings.
    fn take_check(&self, state: &State) {
        let Some(outcome) = self.handed.lock().unwrap().take() else {
            return;
        };
        match outcome {
            Ok(pass) => {
                self.keep_pass(&state.library, Some(pass));
                let found = self.found.read().unwrap().clone();
                let done = match found {
                    Some(found) => self.finish_add(state, &found).map_err(|e| format!("{e:#}")),
                    None => Err(NOT_AT_HAND.to_string()),
                };
                if let Err(why) = done {
                    *self.check_trouble.lock().unwrap() = Some(why);
                }
            }
            Err(why) => *self.check_trouble.lock().unwrap() = Some(why),
        }
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
        let row = state
            .addons
            .read()
            .unwrap()
            .iter()
            .find(|a| a.stored.id == ID)
            .cloned();
        let added = row.is_some();
        let on = row.as_ref().is_some_and(|r| r.stored.enabled);
        let ready = added && self.source().is_some();
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
            "added": added,
            "ready": ready,
            "on": on,
            "trouble": if ready || !added { None } else { self.trouble.lock().unwrap().clone() },
            "allowance": allowance,
            "supporter": self.signing.entitled(),
            "can_claim": state.away.can_claim(),
            "claiming": state.away.claiming(),
            // The check's window is open, or what it came back with is still being taken.
            "checking": self.checking.load(Ordering::Relaxed) || self.handed.lock().unwrap().is_some(),
            "check_trouble": self.check_trouble.lock().unwrap().clone(),
            "needs_check": added && !self.has_pass() && self.needs_check.load(Ordering::Relaxed),
        })
    }
}

/// While the listener has Stash's Tidal added: reads the config (and the add-on) as it falls due, and the allowance
/// when it is asked for, until Stash stops. Nothing is asked of Stash's servers while it is not added.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let tidal = &state.tidal;
    let mut refused_at: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let online = !state.offline.load(Ordering::Relaxed);
        tidal.take_check(state);
        let added = added(state);
        let refused = tidal.signing.take_refused();
        if refused && tidal.has_pass() {
            // The add-on refused this computer's pass: it is let go (the access token is used again, while the add-on
            // takes it) and Settings offers the check again.
            tidal.keep_pass(&state.library, None);
            tidal.needs_check.store(true, Ordering::Relaxed);
            if let Err(e) = state.reload_addons() {
                eprintln!("Stash's Tidal could not be listed: {e:#}");
            }
            state.lacks.forget_addon(ID);
            tidal.ask_allowance.store(true, Ordering::Relaxed);
        } else if refused && refused_at.is_none_or(|at| at.elapsed() >= REFUSED_LOOK) {
            refused_at = Some(Instant::now());
            tidal.look_now.store(true, Ordering::Relaxed);
            // Refused without a pass: once the add-on offers its check, Settings offers it too.
            if online && tidal.check_due().is_some() {
                tidal.needs_check.store(true, Ordering::Relaxed);
            }
        }
        let due = Instant::now() >= *tidal.next_look.lock().unwrap();
        if online
            && added
            && tidal.available()
            && (tidal.look_now.swap(false, Ordering::Relaxed) || due)
            // The add-on changed: the listener's add-ons are read again.
            && tidal.look(&state.library)
        {
            if let Err(e) = state.reload_addons() {
                eprintln!("Stash's Tidal could not be listed: {e:#}");
            }
            state.lacks.forget_addon(ID);
            state.saving.ring();
            tidal.ask_allowance.store(true, Ordering::Relaxed);
        }
        if online && added && tidal.ask_allowance.swap(false, Ordering::Relaxed) {
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
        (Method::Post, "/api/tidal/add") => state.tidal.add(state).map(Some),
        (Method::Post, "/api/tidal/check") => state.tidal.check(state).map(Some),
        _ => Ok(None),
    }
}
