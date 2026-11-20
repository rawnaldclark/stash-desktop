//! The listener's cloud storage in the core (see `stash_core::cloud`): connecting an account in a window of Stash's
//! own, the folders picked as music and their scans (on a thread of their own, each folder again every few hours, and
//! at once when asked), and each connected provider as a source built into Stash, first of all the sources: a song a
//! file of theirs holds plays from that file.
//!
//! The window is told how every step goes: the sign-in, each folder's scan (files met, songs added, the file being
//! read), and what went wrong, in words.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::{
    Addon, AddonError, BuiltIn, Code, Delivery, Manifest, StreamInfo, Tier, Track,
};
use stash_core::cloud::scan::{Progress, Scanner};
use stash_core::cloud::store::{Account, Folder};
use stash_core::cloud::{self, CloudError, Drive, Keys, Kind, TokenFn, auth};
use stash_core::reach::Reach;
use tiny_http::{Method, Request};

use crate::mornye::{VerifyShell, VerifyWindow};
use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

/// How often a folder is scanned again by itself.
const SCAN_EVERY: Duration = Duration::from_secs(3 * 3600);
/// A folder whose scan went wrong is tried again after this.
const TRY_AGAIN: Duration = Duration::from_secs(15 * 60);
/// An access token is taken as good until this long before the provider says it runs out.
const TOKEN_MARGIN: Duration = Duration::from_secs(120);
/// A token renewed this recently is not renewed again (several asks refused at once renew it once).
const JUST_RENEWED: Duration = Duration::from_secs(30);

/// What came of the last connecting, for the window.
#[derive(Debug, Clone, Serialize)]
struct Told {
    provider: &'static str,
    ok: bool,
    message: String,
    at_ms: i64,
}

#[derive(Default)]
struct Scans {
    /// The folder being scanned, and how far it has come.
    now: Option<(i64, Progress)>,
    /// Folders the listener asked to scan now.
    asked: HashSet<i64>,
    /// When each folder was last tried this run.
    tried: HashMap<i64, Instant>,
    /// What each folder's last scan this run found.
    last: HashMap<i64, Progress>,
}

pub(crate) struct Cloud {
    keys: Keys,
    db: PathBuf,
    covers: PathBuf,
    library: Arc<Mutex<Library>>,
    agent: ureq::Agent,
    /// Access tokens by account: the token, until when it is good, and when it was got.
    access: Mutex<HashMap<i64, (String, Instant, Instant)>>,
    renewing: Mutex<()>,
    /// Links to files by (account, file): good until.
    links: Mutex<HashMap<(i64, String), (String, Instant)>>,
    connecting: Mutex<Option<Kind>>,
    told: Mutex<Option<Told>>,
    scans: Mutex<Scans>,
    /// Stops the scan running (its folder or account was taken away, Stash went offline, the core is stopping).
    cancel: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    /// An account was connected or signed out: the sources are to be read again.
    changed: AtomicBool,
    pub(crate) shell: OnceLock<VerifyShell>,
}

/// The listener's words for what went wrong with `kind`.
fn words(kind: Kind, e: &CloudError) -> String {
    match e {
        CloudError::SignedOut => format!("{} needs you to sign in again", kind.name()),
        CloudError::Gone => "it is not there any more".to_string(),
        CloudError::CursorGone => "the folder has to be read again".to_string(),
        CloudError::Busy(m) | CloudError::Failed(m) => m.clone(),
    }
}

fn drive_of(kind: Kind, agent: ureq::Agent, token: TokenFn) -> Box<dyn Drive> {
    match kind {
        Kind::Dropbox => Box::new(cloud::dropbox::Dropbox::new(agent, token)),
        Kind::OneDrive => Box::new(cloud::onedrive::OneDrive::new(agent, token)),
    }
}

fn manifest(kind: Kind) -> Manifest {
    Manifest {
        stash: 1,
        id: kind.id().to_string(),
        name: kind.name().to_string(),
        version: "1".to_string(),
        kind: "server".to_string(),
        description: Some(format!(
            "Built into Stash. Plays the music in your {}.",
            kind.name()
        )),
        resources: vec!["stream".to_string(), "download".to_string()],
        dialect: Default::default(),
        resolve_by_isrc: false,
        quality: None,
        no_downloads: false,
        sent: Vec::new(),
        reach: Some(Reach::Internet),
    }
}

/// The quality tier of a file, from what its scan read of its audio.
fn tier_of(codec: Option<&str>, sample_rate: Option<u32>, bit_depth: Option<u32>) -> Tier {
    match codec {
        Some("flac" | "alac" | "pcm" | "ape" | "wavpack") => {
            if bit_depth.unwrap_or(16) > 16 || sample_rate.unwrap_or(44_100) > 48_000 {
                Tier::Hires
            } else {
                Tier::Lossless
            }
        }
        _ => Tier::High,
    }
}

impl Cloud {
    /// The cloud storage of the library at `db` (whose connection the core holds is `library`), with this build's
    /// `keys`. Pictures of its songs are kept beside the library.
    pub(crate) fn new(library: Arc<Mutex<Library>>, db: &Path, keys: Keys) -> Arc<Self> {
        let folder = db.parent().unwrap_or_else(|| Path::new("."));
        Arc::new(Self {
            keys,
            db: db.to_path_buf(),
            covers: folder.join("Cloud covers"),
            library,
            agent: cloud::agent(),
            access: Mutex::new(HashMap::new()),
            renewing: Mutex::new(()),
            links: Mutex::new(HashMap::new()),
            connecting: Mutex::new(None),
            told: Mutex::new(None),
            scans: Mutex::new(Scans::default()),
            cancel: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            changed: AtomicBool::new(false),
            shell: OnceLock::new(),
        })
    }

    /// The providers this build can connect.
    fn kinds(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|k| self.keys.of(*k).is_some())
            .collect()
    }

    /// Whether an account was connected or signed out since this was last asked.
    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }

    /// The pictures of the cloud's songs, as one at `/api/art/cloud/<name>`.
    pub(crate) fn cover(&self, name: &str) -> Option<stash_core::covers::Cover> {
        stash_core::cloud::scan::cover(&self.covers, name)
    }

    /// Each provider with an account Stash may read, as a source built into Stash.
    pub(crate) fn sources(self: &Arc<Self>) -> Vec<Addon> {
        let accounts = self
            .library
            .lock()
            .unwrap()
            .cloud_accounts()
            .unwrap_or_default();
        self.kinds()
            .into_iter()
            .filter(|kind| accounts.iter().any(|a| a.kind == *kind && a.signed_in))
            .filter_map(|kind| {
                let source = Arc::new(Source {
                    cloud: Arc::clone(self),
                    kind,
                });
                Addon::built_in(manifest(kind), source, Reach::Internet).ok()
            })
            .collect()
    }

    fn wake_scans(&self) {
        let (lock, ring) = &self.wake;
        *lock.lock().unwrap() = true;
        ring.notify_all();
    }

    /// Waits until woken, or `at_most`.
    fn wait(&self, at_most: Duration) {
        let (lock, ring) = &self.wake;
        let mut woken = lock.lock().unwrap();
        if !*woken {
            woken = ring.wait_timeout(woken, at_most).unwrap().0;
        }
        *woken = false;
    }

    // ---- signing in

    /// A good access token for the account (`fresh`: the last one was refused).
    fn access_token(&self, account: &Account, fresh: bool) -> Result<String, CloudError> {
        let cached = |fresh: bool| {
            self.access
                .lock()
                .unwrap()
                .get(&account.id)
                .filter(|(_, until, got)| {
                    if fresh {
                        got.elapsed() < JUST_RENEWED
                    } else {
                        *until > Instant::now()
                    }
                })
                .map(|(token, _, _)| token.clone())
        };
        if let Some(token) = cached(fresh) {
            return Ok(token);
        }
        let _one = self.renewing.lock().unwrap();
        // Renewed by another ask while this one waited.
        if let Some(token) = cached(true).or_else(|| cached(fresh)) {
            return Ok(token);
        }
        let key = self.keys.of(account.kind).ok_or_else(|| {
            CloudError::Failed(format!("this Stash cannot reach {}", account.kind.name()))
        })?;
        let refresh = self
            .library
            .lock()
            .unwrap()
            .cloud_token(account.id)
            .map_err(|e| CloudError::Failed(e.to_string()))?
            .ok_or(CloudError::SignedOut)?;
        match auth::refresh(&self.agent, account.kind, key, &refresh) {
            Ok(tokens) => {
                if let Some(new) = tokens.refresh.as_deref().filter(|new| *new != refresh) {
                    let _ = self
                        .library
                        .lock()
                        .unwrap()
                        .set_cloud_token(account.id, Some(new));
                }
                let now = Instant::now();
                let until =
                    now + Duration::from_secs(tokens.expires_in).saturating_sub(TOKEN_MARGIN);
                self.access
                    .lock()
                    .unwrap()
                    .insert(account.id, (tokens.access.clone(), until, now));
                Ok(tokens.access)
            }
            Err(CloudError::SignedOut) => {
                let _ = self
                    .library
                    .lock()
                    .unwrap()
                    .set_cloud_token(account.id, None);
                self.access.lock().unwrap().remove(&account.id);
                self.changed.store(true, Ordering::SeqCst);
                Err(CloudError::SignedOut)
            }
            Err(e) => Err(e),
        }
    }

    fn drive(self: &Arc<Self>, account: &Account) -> Box<dyn Drive> {
        let (cloud, account_now) = (Arc::clone(self), account.clone());
        let token: TokenFn = Arc::new(move |fresh| cloud.access_token(&account_now, fresh));
        drive_of(account.kind, self.agent.clone(), token)
    }

    /// Opens the provider's sign-in in the window. What came of it is told (`told`) once the window is back.
    fn connect(self: &Arc<Self>, kind: Kind) -> std::result::Result<(), String> {
        let key = self
            .keys
            .of(kind)
            .ok_or_else(|| format!("this Stash cannot connect {}", kind.name()))?
            .to_string();
        let shell = self
            .shell
            .get()
            .ok_or("connecting needs Stash's own window")?
            .clone();
        let begun = Arc::new(auth::begin(kind, &key));
        *self.connecting.lock().unwrap() = Some(kind);
        *self.told.lock().unwrap() = None;
        let catches = {
            let key = key.clone();
            Arc::new(move |url: &str| auth::is_redirect(kind, &key, url))
        };
        let caught = {
            let (cloud, key, begun) = (Arc::clone(self), key.clone(), Arc::clone(&begun));
            Arc::new(move |url: String| {
                let outcome = cloud.finish(kind, &key, &url, &begun);
                *cloud.told.lock().unwrap() = Some(Told {
                    provider: kind.id(),
                    ok: outcome.is_ok(),
                    message: match outcome {
                        Ok(name) => format!("{} is connected as {name}", kind.name()),
                        Err(why) => why,
                    },
                    at_ms: now_ms(),
                });
            })
        };
        let closed = {
            let cloud = Arc::clone(self);
            Box::new(move || {
                cloud.connecting.lock().unwrap().take();
            })
        };
        let opened = shell(VerifyWindow {
            url: begun.url.clone(),
            title: format!("Connect {}", kind.name()),
            catches,
            caught,
            closed,
        });
        if !opened {
            *self.connecting.lock().unwrap() = None;
            return Err("the sign-in window did not open".into());
        }
        Ok(())
    }

    /// Takes the sign-in the window came back with: the account is kept, and it can be read.
    fn finish(
        self: &Arc<Self>,
        kind: Kind,
        key: &str,
        url: &str,
        begun: &auth::Begun,
    ) -> std::result::Result<String, String> {
        let code = auth::code_of(kind, url, &begun.state)?;
        let tokens = auth::exchange(&self.agent, kind, key, &code, &begun.verifier)
            .map_err(|e| words(kind, &e))?;
        let refresh = tokens.refresh.clone().unwrap_or_default();
        let access = tokens.access.clone();
        let me = drive_of(
            kind,
            self.agent.clone(),
            Arc::new(move |_| Ok(access.clone())),
        )
        .me()
        .map_err(|e| words(kind, &e))?;
        let id = self
            .library
            .lock()
            .unwrap()
            .keep_cloud_account(
                kind,
                &me.id,
                &me.name,
                me.email.as_deref(),
                &refresh,
                now_ms(),
            )
            .map_err(|e| e.to_string())?;
        let now = Instant::now();
        self.access.lock().unwrap().insert(
            id,
            (
                tokens.access,
                now + Duration::from_secs(tokens.expires_in).saturating_sub(TOKEN_MARGIN),
                now,
            ),
        );
        self.changed.store(true, Ordering::SeqCst);
        self.wake_scans();
        Ok(me.name)
    }

    // ---- playing

    /// A link to the account's file `item`, kept while it is good.
    fn link(self: &Arc<Self>, account: &Account, item: &str) -> Result<String, CloudError> {
        let key = (account.id, item.to_string());
        if let Some((url, until)) = self.links.lock().unwrap().get(&key)
            && *until > Instant::now()
        {
            return Ok(url.clone());
        }
        let link = self.drive(account).link(item)?;
        let mut links = self.links.lock().unwrap();
        links.retain(|_, (_, until)| *until > Instant::now());
        links.insert(key, (link.url.clone(), Instant::now() + link.good_for));
        Ok(link.url)
    }

    // ---- the window

    fn status(&self, state: &State) -> Result<Value> {
        let library = self.library.lock().unwrap();
        let accounts = library.cloud_accounts()?;
        let scans = self.scans.lock().unwrap();
        let mut listed = Vec::new();
        for account in &accounts {
            let folders: Vec<Value> = library
                .cloud_folders(account.id)?
                .into_iter()
                .map(|f| {
                    let scanning = scans.now.as_ref().filter(|(id, _)| *id == f.id).map(|(_, p)| p.clone());
                    json!({
                        "id": f.id,
                        "path": f.path,
                        "songs": f.songs,
                        "scanned_at_ms": f.scanned_at_ms,
                        "trouble": f.trouble,
                        "scanning": scanning,
                        "waiting": scans.asked.contains(&f.id) || (f.scanned_at_ms.is_none() && f.trouble.is_none()),
                        "last": scans.last.get(&f.id),
                    })
                })
                .collect();
            listed.push(json!({
                "id": account.id,
                "provider": account.kind.id(),
                "name": account.name,
                "email": account.email,
                "signed_in": account.signed_in,
                "connected_at_ms": account.connected_at_ms,
                "folders": folders,
            }));
        }
        Ok(json!({
            "providers": self.kinds().iter().map(|k| json!({ "id": k.id(), "name": k.name() })).collect::<Vec<_>>(),
            "window": self.shell.get().is_some(),
            "connecting": self.connecting.lock().unwrap().map(Kind::id),
            "told": self.told.lock().unwrap().clone(),
            "offline": crate::serve::offline(state),
            "accounts": listed,
        }))
    }

    fn account(&self, id: i64) -> Result<Option<Account>> {
        Ok(self.library.lock().unwrap().cloud_account(id)?)
    }

    /// Stops the scan running when it is one of `folders` (none: whichever is running).
    fn stop_scan_of(&self, folders: &[i64]) {
        let scans = self.scans.lock().unwrap();
        if let Some((id, _)) = &scans.now
            && (folders.is_empty() || folders.contains(id))
        {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    // ---- scanning

    /// The folders whose scan is due: asked for, never scanned, or not for a while (a folder whose scan went wrong
    /// waits a little before it is tried again). Only of accounts Stash may read.
    fn due(&self) -> Vec<(Account, Folder)> {
        let library = self.library.lock().unwrap();
        let Ok(accounts) = library.cloud_accounts() else {
            return Vec::new();
        };
        let scans = self.scans.lock().unwrap();
        let now = now_ms();
        let mut out = Vec::new();
        for account in accounts
            .into_iter()
            .filter(|a| a.signed_in && self.keys.of(a.kind).is_some())
        {
            for folder in library.cloud_folders(account.id).unwrap_or_default() {
                let asked = scans.asked.contains(&folder.id);
                let tried_lately = scans
                    .tried
                    .get(&folder.id)
                    .is_some_and(|at| at.elapsed() < TRY_AGAIN);
                let stale = folder
                    .scanned_at_ms
                    .is_none_or(|at| now - at >= SCAN_EVERY.as_millis() as i64);
                if asked || (!tried_lately && stale) {
                    out.push((account.clone(), folder));
                }
            }
        }
        out
    }

    fn scan_one(
        self: &Arc<Self>,
        state: &State,
        account: &Account,
        folder: &Folder,
        stop: &AtomicBool,
    ) {
        {
            let mut scans = self.scans.lock().unwrap();
            scans.asked.remove(&folder.id);
            scans.tried.insert(folder.id, Instant::now());
            scans.now = Some((folder.id, Progress::default()));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let outcome = (|| -> std::result::Result<Progress, CloudError> {
            let mut library =
                Library::open(&self.db).map_err(|e| CloudError::Failed(e.to_string()))?;
            let drive = self.drive(account);
            let scanner = Scanner {
                drive: &*drive,
                agent: &self.agent,
                covers: &self.covers,
                stop: &self.cancel,
            };
            let mut tell = |p: &Progress| {
                if stop.load(Ordering::Relaxed) || crate::serve::offline(state) {
                    self.cancel.store(true, Ordering::SeqCst);
                }
                self.scans.lock().unwrap().now = Some((folder.id, p.clone()));
            };
            let done = cloud::scan::scan(&mut library, &scanner, account, folder, &mut tell)?;
            let _ = library.cloud_folder_done(folder.id, now_ms(), None);
            Ok(done)
        })();
        let stopped = self.cancel.swap(false, Ordering::SeqCst);
        let mut scans = self.scans.lock().unwrap();
        scans.now = None;
        match outcome {
            Ok(done) => {
                if done.added > 0 || done.left > 0 {
                    state.edited();
                }
                scans.last.insert(folder.id, done);
            }
            // Stopped on purpose: nothing went wrong, and it goes on from there next time.
            Err(_) if stopped => {
                scans.tried.remove(&folder.id);
            }
            Err(e) => {
                drop(scans);
                let why = words(account.kind, &e);
                eprintln!(
                    "cloud: a scan of a {} folder stopped: {why}",
                    account.kind.name()
                );
                let _ =
                    self.library
                        .lock()
                        .unwrap()
                        .cloud_folder_done(folder.id, now_ms(), Some(&why));
                // Songs written before it stopped are in the library.
                state.edited();
            }
        }
    }
}

/// A provider as a source of songs: it plays the files of its accounts, and finds nothing else.
struct Source {
    cloud: Arc<Cloud>,
    kind: Kind,
}

impl Source {
    fn error(&self, code: Code, message: String) -> AddonError {
        AddonError {
            code,
            message,
            retry_after: None,
            who: Some(self.kind.name()),
        }
    }
}

impl BuiltIn for Source {
    fn search(&self, _query: &str, _limit: u32) -> std::result::Result<Vec<Track>, AddonError> {
        Ok(Vec::new())
    }

    fn stream(&self, id: &str) -> std::result::Result<StreamInfo, AddonError> {
        let (accounts, audio) = {
            let library = self.cloud.library.lock().unwrap();
            let accounts: Vec<Account> = library
                .cloud_accounts_with(self.kind, id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|a| library.cloud_account(a).ok().flatten())
                .collect();
            (accounts, library.cloud_audio(self.kind, id).ok().flatten())
        };
        let mut trouble: Option<CloudError> = None;
        for account in &accounts {
            match self.cloud.link(account, id) {
                Ok(url) => {
                    let (codec, sample_rate, bit_depth) =
                        audio.clone().unwrap_or((None, None, None));
                    return Ok(StreamInfo {
                        url,
                        manifest: Delivery::None,
                        tier: tier_of(codec.as_deref(), sample_rate, bit_depth),
                        codec,
                        container: None,
                        bit_depth,
                        sample_rate,
                        bitrate: None,
                        duration_ms: None,
                        expires_at: None,
                        encrypted: None,
                        quota: None,
                    });
                }
                Err(CloudError::Gone) => {}
                Err(e) => {
                    trouble.get_or_insert(e);
                }
            }
        }
        Err(match trouble {
            None => self.error(
                Code::NotFound,
                format!("the file is no longer in your {}", self.kind.name()),
            ),
            Some(CloudError::SignedOut) => {
                self.error(Code::Unauthorized, words(self.kind, &CloudError::SignedOut))
            }
            Some(CloudError::Busy(m)) => self.error(Code::Unavailable, m),
            Some(e) => self.error(Code::Unavailable, words(self.kind, &e)),
        })
    }
}

/// Scans the folders that are due, on the core's thread for it.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let Some(cloud) = state.cloud() else {
        return;
    };
    while !stop.load(Ordering::Relaxed) {
        cloud.wait(Duration::from_secs(2));
        if crate::serve::offline(state) {
            continue;
        }
        for (account, folder) in cloud.due() {
            if stop.load(Ordering::Relaxed) || crate::serve::offline(state) {
                break;
            }
            cloud.scan_one(state, &account, &folder, stop);
        }
    }
}

/// One of the cloud storage routes; `None`: the path is not one of them.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/cloud") && !path.starts_with(cloud::scan::ART) {
        return Ok(None);
    }
    let Some(cloud) = state.cloud() else {
        return Ok(Some(refuse(404, "this Stash has no cloud storage")));
    };
    if let Some(name) = path.strip_prefix(cloud::scan::ART) {
        if *request.method() != Method::Get {
            return Ok(None);
        }
        return Ok(Some(match cloud::scan::cover(&cloud.covers, name) {
            Some(cover) => crate::serve::reply_picture(cover.content_type, cover.bytes),
            None => refuse(404, "no such picture"),
        }));
    }
    let method = request.method().clone();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (Method::Get, ["api", "cloud"]) => Ok(Some(reply_json(200, cloud.status(state)?))),
        (Method::Post, ["api", "cloud", "connect"]) => {
            let body = body_json(request)?;
            let Some(kind) = body
                .get("provider")
                .and_then(Value::as_str)
                .and_then(Kind::from_id)
            else {
                return Ok(Some(refuse(400, "which provider?")));
            };
            Ok(Some(match cloud.connect(kind) {
                Ok(()) => reply_json(200, cloud.status(state)?),
                Err(why) => refuse(409, &why),
            }))
        }
        (Method::Post, ["api", "cloud", "accounts", id, "disconnect"]) => {
            let Ok(id) = id.parse::<i64>() else {
                return Ok(Some(refuse(400, "no such account")));
            };
            let folders: Vec<i64> = cloud
                .library
                .lock()
                .unwrap()
                .cloud_folders(id)?
                .iter()
                .map(|f| f.id)
                .collect();
            if !folders.is_empty() {
                cloud.stop_scan_of(&folders);
            }
            let left = cloud.library.lock().unwrap().forget_cloud_account(id)?;
            cloud.access.lock().unwrap().remove(&id);
            cloud
                .links
                .lock()
                .unwrap()
                .retain(|(account, _), _| *account != id);
            state.reload_addons()?;
            state.edited();
            Ok(Some(reply_json(
                200,
                json!({ "left": left, "cloud": cloud.status(state)? }),
            )))
        }
        (Method::Get, ["api", "cloud", "accounts", id, "browse"]) => {
            let Some(account) = id
                .parse::<i64>()
                .ok()
                .map(|id| cloud.account(id))
                .transpose()?
                .flatten()
            else {
                return Ok(Some(refuse(404, "no such account")));
            };
            let url = url::Url::parse(&format!("http://local{}", request.url()))?;
            let folder = url
                .query_pairs()
                .find(|(k, _)| k == "folder")
                .map(|(_, v)| v.into_owned())
                .filter(|f| !f.is_empty());
            Ok(Some(
                match cloud.drive(&account).browse(folder.as_deref()) {
                    Ok(entries) => {
                        let songs = entries
                            .iter()
                            .filter(|e| !e.folder && cloud::is_audio(&e.name))
                            .count();
                        let mut folders: Vec<_> =
                            entries.into_iter().filter(|e| e.folder).collect();
                        folders.sort_by_key(|e| e.name.to_lowercase());
                        reply_json(200, json!({ "folders": folders, "songs_here": songs }))
                    }
                    Err(e) => refuse(502, &words(account.kind, &e)),
                },
            ))
        }
        (Method::Post, ["api", "cloud", "accounts", id, "folders"]) => {
            let Some(account) = id
                .parse::<i64>()
                .ok()
                .map(|id| cloud.account(id))
                .transpose()?
                .flatten()
            else {
                return Ok(Some(refuse(404, "no such account")));
            };
            let body = body_json(request)?;
            let item = body
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let path = body
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if item.len() > 1024 || path.len() > 4096 || (item.is_empty() != path.is_empty()) {
                return Ok(Some(refuse(400, "that is not a folder")));
            }
            // The folder is there, and is a folder of this account.
            if !item.is_empty()
                && let Err(e) = cloud.drive(&account).path_of(&item)
            {
                return Ok(Some(refuse(409, &words(account.kind, &e))));
            }
            let id = cloud.library.lock().unwrap().add_cloud_folder(
                account.id,
                &item,
                &path,
                now_ms(),
            )?;
            cloud.scans.lock().unwrap().asked.insert(id);
            cloud.wake_scans();
            Ok(Some(reply_json(200, cloud.status(state)?)))
        }
        (Method::Post, ["api", "cloud", "folders", id, "remove"]) => {
            let Ok(id) = id.parse::<i64>() else {
                return Ok(Some(refuse(400, "no such folder")));
            };
            cloud.stop_scan_of(&[id]);
            let left = cloud.library.lock().unwrap().forget_cloud_folder(id)?;
            cloud.scans.lock().unwrap().asked.remove(&id);
            state.edited();
            Ok(Some(reply_json(
                200,
                json!({ "left": left, "cloud": cloud.status(state)? }),
            )))
        }
        (Method::Post, ["api", "cloud", "scan"]) => {
            let body = body_json(request).unwrap_or(Value::Null);
            let library = cloud.library.lock().unwrap();
            let mut asked = Vec::new();
            for account in library
                .cloud_accounts()?
                .into_iter()
                .filter(|a| a.signed_in)
            {
                for folder in library.cloud_folders(account.id)? {
                    if body
                        .get("folder")
                        .and_then(Value::as_i64)
                        .is_none_or(|only| only == folder.id)
                    {
                        asked.push(folder.id);
                    }
                }
            }
            drop(library);
            cloud.scans.lock().unwrap().asked.extend(asked);
            cloud.wake_scans();
            Ok(Some(reply_json(200, cloud.status(state)?)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_tier_is_read_from_its_audio() {
        assert_eq!(
            tier_of(Some("flac"), Some(44_100), Some(16)),
            Tier::Lossless
        );
        assert_eq!(tier_of(Some("flac"), Some(96_000), Some(24)), Tier::Hires);
        assert_eq!(tier_of(Some("alac"), None, None), Tier::Lossless);
        assert_eq!(tier_of(Some("mp3"), Some(44_100), None), Tier::High);
        assert_eq!(tier_of(None, None, None), Tier::High);
        assert!(
            manifest(Kind::Dropbox)
                .resources
                .iter()
                .all(|r| r != "search")
        );
    }
}
