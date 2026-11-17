//! The add-ons installed (spec §5, §7, §8, §9, §12; Lokal `morneye_packages.dart`): repositories and their
//! catalogue, installing (from a repository, its SHA-256 checked, or from a file), updating, removing, switching off,
//! settings, logins and verification, actions, and asking an add-on for songs.
//!
//! On disk, under `root`: `index.json` (repositories and packages), `packages/<key>/` (each package's files),
//! `staging/` (installs being checked), `work/` (downloads), `data/` (what add-ons keep, [`Store`]).
//!
//! Each add-on has one shared runtime (64 MiB, `initialize(settings)` on creation) for searching, links, logins and
//! actions; each download gets one of its own ([`crate::download`]). Both share the add-on's [`Network`] and
//! [`Session`]. A call that timed out or was cancelled closes the runtime; the next call makes a new one.

use std::cmp::Ordering as Order;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::Trouble;
use crate::engine::Engine;
use crate::host::{Hooks, Host, HostSetup, Transfer};
use crate::manifest::{Manifest, Package, compare_versions, is_package_name, key_of, text_of};
use crate::net::{Network, NetworkSetup};
use crate::registry::{self, Entry, OFFICIAL};
use crate::session::{Session, SessionConfig, rate_limit_message};
use crate::store::Store;
use crate::track::{MTrack, atmos_mode, rows};

const OFFICIAL_NAME: &str = "Mornye official";
const REGISTRY_LIMIT: u64 = 2 * 1024 * 1024;
const PACKAGE_LIMIT: u64 = 32 * 1024 * 1024;
const SHARED_HEAP_MB: u64 = 64;
const DOWNLOAD_HEAP_MB: u64 = 256;
/// The default time an add-on has for a call.
pub(crate) const CALL: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Tidal's add-ons wait longer on their service (pkgs.dart:1436-1438).
const TIDAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(75);
const REMEMBERED: usize = 2000;
const SEARCH_LIMIT: usize = 20;
const FORM_LIFE: Duration = Duration::from_secs(5 * 60);
const FORM_STEPS: u32 = 8;
const FORM_FIELDS: usize = 12;
const SETTING_TEXT: usize = 8000;
const OFF: &str = "This addon is not installed or is turned off";
const NOT_FOUND: &str = "Addon not found";
const UNSAFE_PATH: &str = "Addon package contains an unsafe archive path";

/// A repository, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryView {
    pub id: String,
    pub url: String,
    pub name: Option<String>,
    pub official: bool,
    /// When it was last read (ms).
    pub refreshed_at: Option<i64>,
    /// Why it could not be read the last time.
    pub error: Option<String>,
    /// How many packages it lists.
    pub count: usize,
}

/// A package a repository lists, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogueEntry {
    pub repository: String,
    pub id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    /// Its `min_app_version` is one this host takes.
    pub compatible: bool,
    /// The version installed, when it is.
    pub installed: Option<String>,
    /// A newer version than the one installed.
    pub update: bool,
}

/// What a person must do before an add-on can be used (pkgs.dart:1027-1089).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Access {
    /// `verification` (a signed session), `login` (an action that signs in), `settings`, or `none`.
    pub kind: String,
    pub verified: bool,
    /// The labels of required settings left empty.
    pub missing: Vec<String>,
    pub ready: bool,
}

/// An installed add-on, for the window.
#[derive(Debug, Clone, Serialize)]
pub struct InstalledView {
    pub key: String,
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    /// It can be searched and played (a download provider with `searchTracks` or `customSearch`).
    pub playable: bool,
    pub methods: Vec<String>,
    pub repository: Option<String>,
    pub installed_at: i64,
    /// The manifest's `settings`, `qualityOptions` and `globalActions`, for the settings form.
    pub settings: Value,
    pub quality_options: Value,
    pub global_actions: Value,
    pub access: Access,
}

/// How far a download is, for the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    /// `initializing`, `metadata`, `availability`, `preparing_media`, `first_byte`, `download_complete`,
    /// `decrypting_media`, `verifying_media`, `ready` (spec §10.2).
    pub stage: Option<String>,
    pub percent: Option<f64>,
    pub message: Option<String>,
}

/// A download asked of an add-on.
#[derive(Clone, Default)]
pub struct DownloadRequest {
    /// A quality option's id; else the one set, else the best (spec §10.8).
    pub quality: Option<String>,
    /// To be played (else to be kept).
    pub for_playback: bool,
    /// The song as a search gave it, when known (`{id, name, artists, duration_ms}` at least).
    pub hint: Option<Value>,
    pub progress: Option<Arc<dyn Fn(&Progress) + Send + Sync>>,
    /// Each step of a single-file download, for playing it as it comes in.
    pub transfer: Option<Arc<dyn Fn(&Transfer) + Send + Sync>>,
    /// Set to stop the download.
    pub cancel: Arc<AtomicBool>,
}

/// A song an add-on downloaded: the file, the folder it is in (deleted by [`Downloaded::cleanup`]), what the add-on
/// said of it, and what the file is ([`crate::probe::probe`]).
#[derive(Debug, Clone)]
pub struct Downloaded {
    pub file: PathBuf,
    pub work: PathBuf,
    pub metadata: Value,
    pub info: Value,
    /// When the file is still encrypted and no FFmpeg was there to open it: the key (32 hex) the player opens it
    /// with (`decryption_key` of the mov demuxer).
    pub decryption_key: Option<String>,
    /// The quality option asked for.
    pub quality: String,
}

impl Downloaded {
    /// Deletes its work folder.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

/// One add-on's running pieces, made when first needed.
pub(crate) struct Running {
    pub(crate) engine: Arc<Engine>,
    pub(crate) host: Arc<Host>,
}

/// An installed package, as the index keeps it.
#[derive(Clone)]
struct Addon {
    key: String,
    name: String,
    manifest: Arc<Manifest>,
    digest: String,
    installed_at: i64,
    methods: Vec<String>,
    repository: Option<String>,
    enabled: bool,
}

impl Addon {
    fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "name": self.name,
            "manifest": self.manifest.raw,
            "digest": self.digest,
            "installed_at": self.installed_at,
            "methods": self.methods,
            "repository_id": self.repository,
            "enabled": self.enabled,
        })
    }

    /// One the index kept; `None` when its manifest no longer passes the checks, or does not match its key.
    fn from_json(value: &Value) -> Option<Addon> {
        let key = value.get("key")?.as_str()?.to_string();
        let manifest = Manifest::read(value.get("manifest")?.clone()).ok()?;
        if key_of(&manifest.name) != key {
            return None;
        }
        Some(Addon {
            key,
            name: manifest.name.clone(),
            digest: text_of(value.get("digest")),
            installed_at: value
                .get("installed_at")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            methods: value
                .get("methods")
                .and_then(Value::as_array)
                .map(|methods| methods.iter().map(|m| text_of(Some(m))).collect())
                .unwrap_or_default(),
            repository: value
                .get("repository_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            enabled: value.get("enabled") != Some(&Value::Bool(false)),
            manifest: Arc::new(manifest),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Repo {
    id: String,
    url: String,
    name: String,
    refreshed_at: Option<i64>,
    entries: Vec<Entry>,
    error: Option<String>,
}

impl Repo {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "url": self.url,
            "name": self.name,
            "refreshed_at": self.refreshed_at,
            "entries": self.entries,
            "error": self.error,
        })
    }

    fn from_json(value: &Value) -> Option<Repo> {
        Some(Repo {
            id: value.get("id")?.as_str()?.to_string(),
            url: value.get("url")?.as_str()?.to_string(),
            name: text_of(value.get("name")),
            refreshed_at: value.get("refreshed_at").and_then(Value::as_i64),
            entries: value
                .get("entries")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                        .collect()
                })
                .unwrap_or_default(),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn view(&self) -> RepositoryView {
        RepositoryView {
            id: self.id.clone(),
            url: self.url.clone(),
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            official: self.url == OFFICIAL,
            refreshed_at: self.refreshed_at,
            error: self.error.clone(),
            count: self.entries.len(),
        }
    }
}

/// `index.json`.
#[derive(Clone, Default)]
struct Index {
    repositories: Vec<Repo>,
    addons: HashMap<String, Addon>,
}

impl Index {
    fn to_json(&self) -> Value {
        let mut addons: Vec<&Addon> = self.addons.values().collect();
        addons.sort_by_key(|addon| (addon.installed_at, addon.key.clone()));
        json!({
            "repositories": self.repositories.iter().map(Repo::to_json).collect::<Vec<_>>(),
            "packages": addons.iter().map(|addon| addon.to_json()).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Index {
        let list = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Index {
            repositories: list("repositories")
                .iter()
                .filter_map(Repo::from_json)
                .collect(),
            addons: list("packages")
                .iter()
                .filter_map(Addon::from_json)
                .map(|addon| (addon.key.clone(), addon))
                .collect(),
        }
    }
}

/// An add-on's network and signed session, shared by all its runtimes.
#[derive(Clone)]
struct Link {
    network: Arc<Network>,
    session: Option<Arc<Session>>,
}

#[derive(Default)]
struct Links {
    live: HashMap<String, Link>,
    /// Whether an add-on's connection record may still be kept: off once it is removed, so a download still running
    /// does not write it back.
    alive: HashMap<String, Arc<AtomicBool>>,
}

/// Songs searches gave, the most recent last (at most [`REMEMBERED`]).
#[derive(Default)]
struct Remembered {
    tracks: HashMap<String, MTrack>,
    order: VecDeque<String>,
}

impl Remembered {
    fn put(&mut self, key: String, track: MTrack) {
        if self.tracks.insert(key.clone(), track).is_some() {
            self.order.retain(|known| *known != key);
        }
        self.order.push_back(key);
        while self.order.len() > REMEMBERED {
            if let Some(oldest) = self.order.pop_front() {
                self.tracks.remove(&oldest);
            }
        }
    }

    fn get(&mut self, key: &str) -> Option<MTrack> {
        let track = self.tracks.get(key).cloned()?;
        self.order.retain(|known| known != key);
        self.order.push_back(key.to_string());
        Some(track)
    }

    fn forget(&mut self, prefix: &str) {
        self.tracks.retain(|key, _| !key.starts_with(prefix));
        self.order.retain(|key| !key.starts_with(prefix));
    }
}

/// A form an action returned, waiting for its `submit_action`.
struct Form {
    key: String,
    action: String,
    steps: u32,
    expires: Instant,
}

type Slot = Arc<Mutex<Option<Arc<Running>>>>;

pub struct Packages {
    root: PathBuf,
    store: Arc<Store>,
    app_version: String,
    index: Mutex<Index>,
    links: Mutex<Links>,
    /// Each add-on's shared runtime; its lock is held while one is made, so callers wait for it.
    runtimes: Mutex<HashMap<String, Slot>>,
    /// One install, update or removal of an add-on at a time.
    gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    remembered: Mutex<Remembered>,
    forms: Mutex<HashMap<String, Form>>,
    ffmpeg: Mutex<Option<PathBuf>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Packages {
    /// The add-ons under `root`, sealed with keys drawn from `master`; `app_version` is Stash's.
    pub fn open(
        root: PathBuf,
        master: [u8; 32],
        app_version: &str,
    ) -> Result<Arc<Packages>, String> {
        fs::create_dir_all(&root).map_err(|e| format!("Cannot open the add-ons folder: {e}"))?;
        // Nothing runs yet: what an earlier run left half-done is gone.
        for leftover in ["staging", "work"] {
            let _ = fs::remove_dir_all(root.join(leftover));
        }
        let mut index = fs::read(root.join("index.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map(|value| Index::from_json(&value))
            .unwrap_or_default();
        let mut changed = false;
        if !index.repositories.iter().any(|repo| repo.url == OFFICIAL) {
            let (url, id) = registry::normalize(OFFICIAL)?;
            index.repositories.insert(
                0,
                Repo {
                    id,
                    url,
                    name: OFFICIAL_NAME.into(),
                    refreshed_at: None,
                    entries: Vec::new(),
                    error: None,
                },
            );
            changed = true;
        }
        // An update cut short leaves the old files beside the new place: put them back.
        for key in index.addons.keys() {
            let dir = root.join("packages").join(key);
            let previous = root.join("packages").join(format!("{key}.previous"));
            if !dir.exists() && previous.exists() {
                let _ = fs::rename(&previous, &dir);
            }
        }
        let packages = Packages {
            store: Arc::new(Store::new(root.join("data"), master)),
            root,
            app_version: app_version.to_string(),
            index: Mutex::new(index),
            links: Mutex::default(),
            runtimes: Mutex::default(),
            gates: Mutex::default(),
            remembered: Mutex::default(),
            forms: Mutex::default(),
            ffmpeg: Mutex::new(None),
        };
        if changed {
            let index = lock(&packages.index);
            packages.save(&index)?;
        }
        Ok(Arc::new(packages))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// FFmpeg, when the computer has it.
    pub fn set_ffmpeg(&self, path: Option<PathBuf>) {
        *lock(&self.ffmpeg) = path;
    }

    // ---- repositories

    pub fn repositories(&self) -> Vec<RepositoryView> {
        lock(&self.index)
            .repositories
            .iter()
            .map(Repo::view)
            .collect()
    }

    /// Adds a repository and reads it.
    pub fn add_repository(&self, url: &str) -> Result<RepositoryView, String> {
        let (url, id) = registry::normalize(url)?;
        {
            let mut index = lock(&self.index);
            if !index.repositories.iter().any(|repo| repo.id == id) {
                let host = url::Url::parse(&url)
                    .ok()
                    .and_then(|parsed| parsed.host_str().map(str::to_string))
                    .unwrap_or_default();
                index.repositories.push(Repo {
                    id: id.clone(),
                    url: url.clone(),
                    name: host,
                    refreshed_at: None,
                    entries: Vec::new(),
                    error: None,
                });
                self.save(&index)?;
            }
        }
        let official = url == OFFICIAL;
        self.read_repositories(&[(id.clone(), url, official)]);
        // Kept with its error when it could not be read, as Lokal keeps it.
        let view = self
            .repositories()
            .into_iter()
            .find(|repo| repo.id == id)
            .ok_or_else(|| "Repository not found".to_string())?;
        match &view.error {
            Some(error) => Err(error.clone()),
            None => Ok(view),
        }
    }

    /// Not the official one (`The official repository cannot be removed`).
    pub fn remove_repository(&self, id: &str) -> Result<(), String> {
        let mut index = lock(&self.index);
        let at = index
            .repositories
            .iter()
            .position(|repo| repo.id == id)
            .ok_or_else(|| "Repository not found".to_string())?;
        if index.repositories[at].url == OFFICIAL {
            return Err("The official repository cannot be removed".into());
        }
        let removed = index.repositories.remove(at);
        if let Err(error) = self.save(&index) {
            index.repositories.insert(at, removed);
            return Err(error);
        }
        Ok(())
    }

    /// Reads one repository again, or all; what went wrong is kept on each.
    pub fn refresh(&self, id: Option<&str>) -> Vec<RepositoryView> {
        let targets: Vec<(String, String, bool)> = lock(&self.index)
            .repositories
            .iter()
            .filter(|repo| id.is_none_or(|id| repo.id == id))
            .map(|repo| (repo.id.clone(), repo.url.clone(), repo.url == OFFICIAL))
            .collect();
        self.read_repositories(&targets);
        self.repositories()
    }

    /// Reads the repositories not read for `max_age`.
    pub fn refresh_due(&self, max_age: Duration) {
        let now = crate::now_ms();
        let age = i64::try_from(max_age.as_millis()).unwrap_or(i64::MAX);
        let targets: Vec<(String, String, bool)> = lock(&self.index)
            .repositories
            .iter()
            .filter(|repo| {
                repo.refreshed_at
                    .is_none_or(|at| now.saturating_sub(at) > age)
            })
            .map(|repo| (repo.id.clone(), repo.url.clone(), repo.url == OFFICIAL))
            .collect();
        if !targets.is_empty() {
            self.read_repositories(&targets);
        }
    }

    pub fn catalogue(&self) -> Vec<CatalogueEntry> {
        let index = lock(&self.index);
        let mut catalogue = Vec::new();
        for repo in &index.repositories {
            for entry in &repo.entries {
                let installed = index
                    .addons
                    .get(&key_of(&entry.id))
                    .map(|addon| addon.manifest.version.clone());
                let update = installed.as_deref().is_some_and(|version| {
                    compare_versions(&entry.version, version) == Order::Greater
                });
                let wanted = entry.min_app_version.as_deref().unwrap_or("0");
                catalogue.push(CatalogueEntry {
                    repository: repo.id.clone(),
                    id: entry.id.clone(),
                    version: entry.version.clone(),
                    display_name: entry.display_name.clone(),
                    description: entry.description.clone(),
                    compatible: compare_versions(wanted, crate::COMPAT_VERSION) != Order::Greater,
                    installed,
                    update,
                });
            }
        }
        catalogue
    }

    /// Reads repositories side by side, outside the index's lock, then keeps what each gave (or why not).
    fn read_repositories(&self, targets: &[(String, String, bool)]) {
        let user_agent = self.user_agent();
        let results: Vec<(String, Result<(Option<String>, Vec<Entry>), String>)> =
            std::thread::scope(|scope| {
                let reads: Vec<_> = targets
                    .iter()
                    .map(|(id, url, official)| {
                        let user_agent = &user_agent;
                        (
                            id.clone(),
                            scope.spawn(move || read_repository(url, *official, user_agent)),
                        )
                    })
                    .collect();
                reads
                    .into_iter()
                    .map(|(id, read)| {
                        let result = read.join().unwrap_or_else(|_| {
                            Err("The repository could not be read".to_string())
                        });
                        (id, result)
                    })
                    .collect()
            });
        let mut index = lock(&self.index);
        for (id, result) in results {
            let Some(repo) = index.repositories.iter_mut().find(|repo| repo.id == id) else {
                continue;
            };
            match result {
                Ok((name, entries)) => {
                    repo.entries = entries;
                    repo.refreshed_at = Some(crate::now_ms());
                    repo.error = None;
                    if let Some(name) = name {
                        repo.name = name;
                    }
                }
                Err(error) => repo.error = Some(error),
            }
        }
        let _ = self.save(&index);
    }

    // ---- packages

    /// Installs (or updates to) the package `id` a repository lists.
    pub fn install(&self, repository: &str, id: &str) -> Result<InstalledView, String> {
        let entry = lock(&self.index)
            .repositories
            .iter()
            .find(|repo| repo.id == repository)
            .and_then(|repo| repo.entries.iter().find(|entry| entry.id == id).cloned())
            .ok_or_else(|| "Package not found in this repository".to_string())?;
        let path = url::Url::parse(&entry.download_url)
            .map(|url| url.path().to_string())
            .unwrap_or_default();
        if !is_package_name(&path) {
            return Err("Choose a .sflx, .spotiflac-ext or .morneye-ext package".into());
        }
        let bytes = registry::fetch(&entry.download_url, PACKAGE_LIMIT, &self.user_agent())?;
        // The registry's SHA-256 is the only thing that vouches for a package: there are no signatures.
        let digest = crate::hex(&crate::crypto::sha256(&bytes));
        if digest != entry.sha256.to_ascii_lowercase() {
            return Err("Package SHA-256 does not match its repository".into());
        }
        let package = Package::read(&bytes)?;
        if package.manifest.name != entry.id || package.manifest.version != entry.version {
            return Err("Package identity/version does not match its registry entry".into());
        }
        self.install_package(package, digest, Some(repository.to_string()))
    }

    /// Installs a package file the person picked.
    pub fn install_file(&self, bytes: &[u8]) -> Result<InstalledView, String> {
        let digest = crate::hex(&crate::crypto::sha256(bytes));
        let package = Package::read(bytes)?;
        self.install_package(package, digest, None)
    }

    pub fn remove(&self, key: &str) -> Result<(), String> {
        let gate = self.gate(key);
        let _held = lock(&gate);
        if self.addon(key).is_none() {
            return Err(NOT_FOUND.into());
        }
        self.retire(key);
        {
            let mut index = lock(&self.index);
            let removed = index.addons.remove(key);
            if let Err(error) = self.save(&index) {
                if let Some(removed) = removed {
                    index.addons.insert(key.to_string(), removed);
                }
                return Err(error);
            }
        }
        self.forget(key);
        let packages = self.root.join("packages");
        let _ = fs::remove_dir_all(packages.join(key));
        let _ = fs::remove_dir_all(packages.join(format!("{key}.previous")));
        self.store.remove(key);
        lock(&self.remembered).forget(&format!("{key}:"));
        lock(&self.forms).retain(|_, form| form.key != key);
        Ok(())
    }

    pub fn set_enabled(&self, key: &str, on: bool) -> Result<(), String> {
        let gate = self.gate(key);
        let _held = lock(&gate);
        if self.addon(key).is_none() {
            return Err(NOT_FOUND.into());
        }
        self.retire(key);
        let mut index = lock(&self.index);
        let before = index.addons.get(key).map(|addon| addon.enabled);
        if let Some(addon) = index.addons.get_mut(key) {
            addon.enabled = on;
        }
        if let Err(error) = self.save(&index) {
            if let (Some(addon), Some(before)) = (index.addons.get_mut(key), before) {
                addon.enabled = before;
            }
            return Err(error);
        }
        Ok(())
    }

    /// The installed add-ons, the newest first.
    pub fn installed(&self) -> Vec<InstalledView> {
        let mut addons: Vec<Addon> = lock(&self.index).addons.values().cloned().collect();
        addons.sort_by(|a, b| {
            b.installed_at
                .cmp(&a.installed_at)
                .then_with(|| a.key.cmp(&b.key))
        });
        addons.iter().map(|addon| self.view(addon)).collect()
    }

    pub fn get(&self, key: &str) -> Option<InstalledView> {
        self.addon(key).map(|addon| self.view(&addon))
    }

    /// The manifest of an installed add-on.
    pub fn manifest(&self, key: &str) -> Option<Arc<Manifest>> {
        self.addon(key).map(|addon| addon.manifest)
    }

    // ---- settings, logins, verification, actions

    /// The add-on's settings: its schema's defaults, its quality options' defaults (`qualitySettings`), then what
    /// was saved.
    pub fn settings(&self, key: &str) -> Result<Value, String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        Ok(self.settings_for(key, &addon.manifest))
    }

    /// Saves settings (checked against the schema; a secret left empty keeps its value); the shared runtime is made
    /// again. Answers the settings as they now are.
    pub fn set_settings(&self, key: &str, values: &Value) -> Result<Value, String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        let input = values
            .as_object()
            .ok_or_else(|| "Invalid settings".to_string())?;
        let manifest = &addon.manifest;
        let mut current = self.store.read(key, "settings");
        assign(&manifest.settings, Some(input), &mut current)?;
        if let Some(Value::Object(quality)) = input.get("qualitySettings") {
            let mut saved = current
                .get("qualitySettings")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for option in &manifest.quality_options {
                let id = text_of(option.get("id"));
                let mut target = saved
                    .get(&id)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let schema = option
                    .get("settings")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assign(
                    &schema,
                    quality.get(&id).and_then(Value::as_object),
                    &mut target,
                )?;
                saved.insert(id, Value::Object(target));
            }
            current.insert("qualitySettings".into(), Value::Object(saved));
        }
        if let Some(chosen) = input.get("downloadQuality").filter(|v| !v.is_null())
            && manifest
                .quality_options
                .iter()
                .any(|option| option.get("id") == Some(chosen))
        {
            current.insert("downloadQuality".into(), chosen.clone());
        }
        self.store.write(key, "settings", &current)?;
        self.retire(key);
        Ok(self.settings_for(key, manifest))
    }

    pub fn access(&self, key: &str) -> Result<Access, String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        Ok(self.access_of(&addon))
    }

    /// `{...session status, authenticated, open_auth_url}`.
    pub fn auth_status(&self, key: &str) -> Result<Value, String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        let access = self.access_of(&addon);
        let link = self.link(key, &addon.manifest);
        let mut status = link
            .session
            .as_ref()
            .map(|session| session.status())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let authenticated = match &link.session {
            Some(_) => Value::Bool(status.get("authenticated") == Some(&Value::Bool(true))),
            None => Value::Bool(access.verified),
        };
        let open = link
            .session
            .as_ref()
            .and_then(|session| session.pending_url())
            .or_else(|| crate::auth::pending_url(&link.network))
            .unwrap_or_default();
        status["authenticated"] = authenticated;
        status["open_auth_url"] = Value::String(open);
        Ok(status)
    }

    /// The page a person must pass (or sign in on), or nothing when the add-on is ready.
    pub fn verify(&self, key: &str) -> Result<Option<String>, String> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| OFF.to_string())?;
        let link = self.link(key, &addon.manifest);
        if let Some(session) = &link.session {
            session.bootstrap()?;
        }
        let status = self.auth_status(key)?;
        if status.get("authenticated") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        let url = text_of(status.get("open_auth_url"));
        Ok((!url.is_empty()).then_some(url))
    }

    /// Forgets the add-on's session, login and credentials.
    pub fn logout(&self, key: &str) -> Result<(), String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        // The add-on is asked to forget what it keeps itself, when it can run.
        let running = self.shared(key).ok();
        let link = self.link(key, &addon.manifest);
        if let Some(session) = &link.session {
            session.clear();
        }
        if let Some(running) = &running
            && running.engine.has("clearAuth")
        {
            let _ = invoke(running, None, "clearAuth", &[], CALL);
        }
        link.network.reset();
        let empty = Map::new();
        let _ = self.store.write(key, "connection", &empty);
        let _ = self.store.write(key, "credentials", &empty);
        Ok(())
    }

    /// Runs one of its declared actions (a settings button, a global action, `completeGrant`, a form's
    /// `submit_action` with its token): `{message?, success?, error?, open_auth_url?, action_form?, formToken?}`.
    pub fn invoke_action(
        &self,
        key: &str,
        action: &str,
        input: Option<Value>,
        token: Option<&str>,
    ) -> Result<Value, String> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| OFF.to_string())?;
        let running = self
            .shared(key)
            .map_err(|trouble| trouble.message().to_string())?;
        let manifest = &addon.manifest;
        // A form's token is good once.
        let form = token.and_then(|token| lock(&self.forms).remove(token));
        let steps = match &form {
            Some(form) => {
                if form.key != key
                    || form.action != action
                    || form.expires <= Instant::now()
                    || form.steps >= FORM_STEPS
                {
                    return Err("Invalid or expired addon form".into());
                }
                form.steps
            }
            None => {
                if !declared_actions(manifest)
                    .iter()
                    .any(|declared| declared == action)
                    && !(action == "completeGrant" && manifest.signed_session.is_some())
                {
                    return Err("This action is not declared by the addon".into());
                }
                0
            }
        };
        let seconds = manifest
            .capabilities
            .get("actionTimeoutSeconds")
            .and_then(Value::as_f64)
            .filter(|seconds| seconds.is_finite())
            .unwrap_or(30.0)
            .clamp(1.0, 120.0);
        let args: Vec<Value> = input.filter(|input| !input.is_null()).into_iter().collect();
        let session = running.host.setup().session.clone();
        let result = invoke(
            &running,
            session.as_deref(),
            action,
            &args,
            Duration::from_secs(seconds as u64),
        )
        .map_err(|trouble| trouble.message().to_string())?;
        let mut payload = match &result {
            Value::Object(map) => match map.get("result") {
                Some(Value::Object(inner)) => inner.clone(),
                _ => map.clone(),
            },
            _ => Map::new(),
        };
        let schema = payload
            .get("action_form")
            .filter(|form| !form.is_null())
            .or_else(|| payload.get("byoa_form"))
            .cloned();
        if let Some(Value::Object(schema)) = schema {
            let submit = schema
                .get("submit_action")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let fields = schema
                .get("fields")
                .and_then(Value::as_array)
                .is_some_and(|fields| fields.len() <= FORM_FIELDS);
            if schema.get("version").and_then(Value::as_f64) != Some(1.0)
                || !running.engine.has(&submit)
                || !fields
            {
                return Err("Invalid addon account form".into());
            }
            let token = crate::hex(&crate::crypto::sha256(&crate::random_bytes(16)));
            let mut forms = lock(&self.forms);
            let now = Instant::now();
            forms.retain(|_, form| form.expires > now);
            forms.insert(
                token.clone(),
                Form {
                    key: key.to_string(),
                    action: submit,
                    steps: steps + 1,
                    expires: now + FORM_LIFE,
                },
            );
            payload.insert("formToken".into(), Value::String(token));
        }
        Ok(Value::Object(payload))
    }

    /// Whether the verification or login window is about to open the add-on's callback.
    pub fn is_callback(&self, key: &str, url: &str) -> bool {
        let Some(addon) = self.addon(key) else {
            return false;
        };
        let link = self.link(key, &addon.manifest);
        match &link.session {
            Some(session) if session.pending_url().is_some() => session.is_callback(url),
            _ => crate::auth::is_callback(&link.network, url),
        }
    }

    /// The window reached the callback: the session's grant is exchanged, or the login's code kept.
    pub fn callback(&self, key: &str, url: &str) -> Result<Value, String> {
        let addon = self.addon(key).ok_or_else(|| NOT_FOUND.to_string())?;
        let link = self.link(key, &addon.manifest);
        match &link.session {
            Some(session) if session.pending_url().is_some() => session.callback(url),
            _ => crate::auth::callback(&link.network, url),
        }
    }

    // ---- songs

    /// Songs for `query` (a link goes to `handleUrl` when it handles links), at most `limit` (20 at most).
    pub fn search(&self, key: &str, query: &str, limit: usize) -> Result<Vec<MTrack>, Trouble> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| Trouble::Missing(OFF.into()))?;
        let limit = if limit == 0 {
            SEARCH_LIMIT
        } else {
            limit.min(SEARCH_LIMIT)
        };
        let running = self.shared(key)?;
        let setup = running.host.setup();
        let session = setup.session.as_deref();
        let manifest = &addon.manifest;
        let query = query.trim();
        let asked = if manifest.handles_links() && is_link(query) {
            invoke(&running, session, "handleUrl", &[json!(query)], CALL)
        } else if running.engine.has("searchTracks") {
            invoke(
                &running,
                session,
                "searchTracks",
                &[json!(query), json!(limit)],
                CALL,
            )
        } else {
            let filter = search_filter(manifest);
            invoke(
                &running,
                session,
                "customSearch",
                &[json!(query), json!({ "limit": limit, "filter": filter })],
                CALL,
            )
        };
        let sort = |trouble: Trouble| {
            sort_trouble(
                trouble,
                &manifest.display_name,
                &setup.network,
                session,
                false,
            )
        };
        let result = asked.map_err(sort)?;
        let mut tracks = rows(&result);
        if tracks.is_empty() && result.get("success") == Some(&Value::Bool(false)) {
            let said = if result.get("needsVerification") == Some(&Value::Bool(true)) {
                "VERIFY_REQUIRED".to_string()
            } else {
                failure_text(&result, "The add-on could not search")
            };
            return Err(sort(Trouble::Failed(said)));
        }
        if manifest.name.to_ascii_lowercase().starts_with("amazon") {
            tracks = collapse_repeats(tracks);
        }
        self.remember(key, &tracks);
        tracks.truncate(limit);
        Ok(tracks)
    }

    /// The song `id` as a search gave it, when it is remembered.
    pub fn track(&self, key: &str, id: &str) -> Option<MTrack> {
        lock(&self.remembered).get(&format!("{key}:{id}"))
    }

    /// Downloads the song `track_id` (spec §10): see [`crate::download`].
    pub fn download(
        &self,
        key: &str,
        track_id: &str,
        request: DownloadRequest,
    ) -> Result<Downloaded, Trouble> {
        crate::download::download(self, key, track_id, request)
    }

    // ---- for crate::download

    /// What a download needs of the add-on: its manifest, network, session, settings and package code.
    pub(crate) fn parts(&self, key: &str) -> Result<Parts, Trouble> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| Trouble::Missing("Addon is not available".into()))?;
        let link = self.link(key, &addon.manifest);
        Ok(Parts {
            settings: self.settings_for(key, &addon.manifest),
            manifest: addon.manifest,
            network: link.network,
            session: link.session,
        })
    }

    /// A fresh runtime for one download (256 MiB, `work` granted, `hooks` wired); not `initialize`d yet.
    pub(crate) fn download_runtime(
        &self,
        key: &str,
        work: &Path,
        hooks: Hooks,
    ) -> Result<Running, Trouble> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| Trouble::Missing("Addon is not available".into()))?;
        let code = self.code(key).map_err(Trouble::Failed)?;
        let link = self.link(key, &addon.manifest);
        self.start(
            key,
            &addon.manifest,
            &code,
            self.files_root(key),
            vec![work.to_path_buf()],
            hooks,
            DOWNLOAD_HEAP_MB,
            &link,
        )
        .map_err(Trouble::Failed)
    }

    /// Remembers songs a search gave (and the download's own lookups).
    pub(crate) fn remember(&self, key: &str, tracks: &[MTrack]) {
        let mut remembered = lock(&self.remembered);
        for track in tracks {
            remembered.put(format!("{key}:{}", track.id), track.clone());
        }
    }

    /// A new work folder under `work/`.
    pub(crate) fn work_folder(&self) -> Result<PathBuf, Trouble> {
        let work = self.root.join("work").join(unique());
        fs::create_dir_all(&work)
            .map_err(|e| Trouble::Failed(format!("Cannot make a download folder: {e}")))?;
        Ok(work)
    }

    pub(crate) fn ffmpeg(&self) -> Option<PathBuf> {
        lock(&self.ffmpeg).clone()
    }

    /// The add-on's own files folder, where its relative paths lead.
    pub(crate) fn files_root(&self, key: &str) -> PathBuf {
        self.root.join("data").join(key).join("files")
    }

    // ---- inside

    fn user_agent(&self) -> String {
        format!("Stash/{}", self.app_version)
    }

    fn addon(&self, key: &str) -> Option<Addon> {
        lock(&self.index).addons.get(key).cloned()
    }

    fn save(&self, index: &Index) -> Result<(), String> {
        atomic_write(
            &self.root.join("index.json"),
            index.to_json().to_string().as_bytes(),
        )
    }

    fn gate(&self, key: &str) -> Arc<Mutex<()>> {
        Arc::clone(lock(&self.gates).entry(key.to_string()).or_default())
    }

    fn code(&self, key: &str) -> Result<String, String> {
        let path = self.root.join("packages").join(key).join("index.js");
        fs::read(&path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|_| "The add-on's files are missing. Install it again.".to_string())
    }

    fn settings_for(&self, key: &str, manifest: &Manifest) -> Value {
        settings_of(manifest, &self.store.read(key, "settings"))
    }

    /// The add-on's network and session, made (with its saved connection) the first time.
    fn link(&self, key: &str, manifest: &Arc<Manifest>) -> Link {
        let mut links = lock(&self.links);
        if let Some(link) = links.live.get(key) {
            return link.clone();
        }
        let alive = Arc::clone(
            links
                .alive
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(AtomicBool::new(true))),
        );
        let keep: Arc<dyn Fn(Value) + Send + Sync> = {
            let store = Arc::clone(&self.store);
            let key = key.to_string();
            Arc::new(move |record: Value| {
                if alive.load(Ordering::SeqCst)
                    && let Value::Object(record) = record
                {
                    let _ = store.write(&key, "connection", &record);
                }
            })
        };
        let network = Arc::new(Network::new(NetworkSetup {
            permissions: manifest.permissions.clone(),
            user_agent: self.user_agent(),
            timeout: request_timeout(manifest),
            saved: Value::Object(self.store.read(key, "connection")),
            keep,
        }));
        let session = manifest
            .signed_session
            .as_ref()
            .and_then(SessionConfig::of)
            .map(|config| {
                Arc::new(Session::new(
                    config,
                    key,
                    Arc::clone(&network),
                    Arc::clone(&self.store),
                ))
            });
        let link = Link { network, session };
        links.live.insert(key.to_string(), link.clone());
        link
    }

    /// Stops the add-on's shared runtime, and lets its network and session go (kept first): the next use makes
    /// them again, with the settings and manifest as they are then.
    fn retire(&self, key: &str) {
        let slot = lock(&self.runtimes).get(key).cloned();
        if let Some(slot) = slot
            && let Some(running) = lock(&slot).take()
        {
            close(&running);
        }
        let link = lock(&self.links).live.remove(key);
        if let Some(link) = link {
            link.network.keep();
        }
    }

    /// The add-on is gone: nothing it still runs may write its connection back.
    fn forget(&self, key: &str) {
        let mut links = lock(&self.links);
        links.live.remove(key);
        if let Some(alive) = links.alive.remove(key) {
            alive.store(false, Ordering::SeqCst);
        }
    }

    /// The add-on's shared runtime, made and `initialize`d when there is none (or it closed).
    fn shared(&self, key: &str) -> Result<Arc<Running>, Trouble> {
        let addon = self
            .addon(key)
            .filter(|addon| addon.enabled)
            .ok_or_else(|| Trouble::Missing(OFF.into()))?;
        let slot = Arc::clone(lock(&self.runtimes).entry(key.to_string()).or_default());
        let mut held = lock(&slot);
        if let Some(running) = held.as_ref()
            && !running.engine.is_closed()
        {
            return Ok(Arc::clone(running));
        }
        *held = None;
        let code = self.code(key).map_err(Trouble::Failed)?;
        let link = self.link(key, &addon.manifest);
        let running = Arc::new(
            self.start(
                key,
                &addon.manifest,
                &code,
                self.files_root(key),
                Vec::new(),
                Hooks::default(),
                SHARED_HEAP_MB,
                &link,
            )
            .map_err(Trouble::Failed)?,
        );
        let settings = self.settings_for(key, &addon.manifest);
        if let Err(trouble) = invoke(
            &running,
            link.session.as_deref(),
            "initialize",
            &[settings],
            CALL,
        ) {
            close(&running);
            return Err(sort_trouble(
                trouble,
                &addon.manifest.display_name,
                &link.network,
                link.session.as_deref(),
                false,
            ));
        }
        *held = Some(Arc::clone(&running));
        Ok(running)
    }

    /// A runtime of the add-on: its host (files in `files`, `grants` too), then its engine with the config the
    /// prelude reads (pkgs.dart:1521-1530).
    #[allow(clippy::too_many_arguments)]
    fn start(
        &self,
        key: &str,
        manifest: &Arc<Manifest>,
        code: &str,
        files: PathBuf,
        grants: Vec<PathBuf>,
        hooks: Hooks,
        heap_mb: u64,
        link: &Link,
    ) -> Result<Running, String> {
        let config = json!({
            "permissions": manifest.permissions.to_json(),
            "signedSession": manifest.signed_session.clone().unwrap_or(Value::Null),
            "settings": self.settings_for(key, manifest),
            "rawFfmpeg": manifest.capabilities.get("rawFfmpeg").cloned().unwrap_or(Value::Null),
            "heapMb": heap_mb,
        });
        let host = Host::new(HostSetup {
            key: key.to_string(),
            manifest: Arc::clone(manifest),
            root: files,
            grants,
            network: Arc::clone(&link.network),
            store: Arc::clone(&self.store),
            session: link.session.clone(),
            hooks,
            ffmpeg: self.ffmpeg(),
            app_version: self.app_version.clone(),
        });
        let engine = Arc::new(Engine::start(code, &config, host.bridge())?);
        host.bind(&engine);
        Ok(Running { engine, host })
    }

    fn view(&self, addon: &Addon) -> InstalledView {
        let manifest = &addon.manifest;
        let has = |name: &str| addon.methods.iter().any(|method| method == name);
        InstalledView {
            key: addon.key.clone(),
            name: addon.name.clone(),
            display_name: manifest.display_name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            enabled: addon.enabled,
            playable: manifest.downloads() && (has("searchTracks") || has("customSearch")),
            methods: addon.methods.clone(),
            repository: addon.repository.clone(),
            installed_at: addon.installed_at,
            settings: Value::Array(manifest.settings.clone()),
            quality_options: Value::Array(manifest.quality_options.clone()),
            global_actions: Value::Array(manifest.global_actions.clone()),
            access: self.access_of(addon),
        }
    }

    fn access_of(&self, addon: &Addon) -> Access {
        let manifest = &addon.manifest;
        let values = self.settings_for(&addon.key, manifest);
        let missing: Vec<String> = manifest
            .settings
            .iter()
            .filter(|field| {
                field.get("required") == Some(&Value::Bool(true))
                    && field.get("type").and_then(Value::as_str) != Some("button")
                    && values
                        .get(text_of(field.get("key")))
                        .is_none_or(|value| value.is_null() || value.as_str() == Some(""))
            })
            .map(|field| {
                text_of(
                    field
                        .get("label")
                        .filter(|l| !l.is_null())
                        .or_else(|| field.get("key")),
                )
            })
            .collect();
        let link = self.link(&addon.key, manifest);
        let (kind, verified) = if let Some(session) = &link.session {
            (
                "verification",
                session.status().get("authenticated") == Some(&Value::Bool(true)),
            )
        } else if manifest.global_actions.iter().any(|action| {
            signs_in(&format!(
                "{} {}",
                text_of(action.get("action")),
                text_of(action.get("label"))
            ))
        }) {
            let account = !self.store.read(&addon.key, "credentials").is_empty();
            ("login", crate::auth::has_login(&link.network) || account)
        } else if missing.is_empty() {
            ("none", true)
        } else {
            ("settings", true)
        };
        Access {
            kind: kind.to_string(),
            ready: verified && missing.is_empty(),
            verified,
            missing,
        }
    }

    fn install_package(
        &self,
        package: Package,
        digest: String,
        repository: Option<String>,
    ) -> Result<InstalledView, String> {
        let code = package.code();
        let Package { manifest, files } = package;
        let manifest = Arc::new(manifest);
        let key = key_of(&manifest.name);
        let gate = self.gate(&key);
        let _held = lock(&gate);
        let existing = self.addon(&key);
        if let Some(existing) = &existing {
            if existing.name != manifest.name {
                return Err("Addon identity collision".into());
            }
            if compare_versions(&manifest.version, &existing.manifest.version) == Order::Less {
                return Err("Cannot downgrade an installed addon".into());
            }
        }
        let staged = self.root.join("staging").join(unique());
        let placed = self.place(Placing {
            key: &key,
            manifest: &manifest,
            code: &code,
            files: &files,
            staged: &staged,
            digest,
            repository,
            existing,
        });
        let _ = fs::remove_dir_all(&staged);
        placed
    }

    /// Writes the files to `staged`, checks the package runs and registers what its kind needs, then moves it to
    /// `packages/<key>` (the old one kept as `.previous` until the index is saved) and records it.
    fn place(&self, placing: Placing<'_>) -> Result<InstalledView, String> {
        let Placing {
            key,
            manifest,
            code,
            files,
            staged,
            digest,
            repository,
            existing,
        } = placing;
        let io = |error: std::io::Error| error.to_string();
        fs::create_dir_all(staged).map_err(io)?;
        for (name, bytes) in files {
            let target = safe_join(staged, name)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::write(&target, bytes).map_err(io)?;
        }
        let validation = staged.join("validation-data");
        let methods = self.validate(key, manifest, code, &validation)?;
        let _ = fs::remove_dir_all(&validation);

        self.retire(key);
        let packages = self.root.join("packages");
        fs::create_dir_all(&packages).map_err(io)?;
        let target = packages.join(key);
        let backup = packages.join(format!("{key}.previous"));
        if backup.exists() {
            fs::remove_dir_all(&backup).map_err(io)?;
        }
        if target.exists() {
            fs::rename(&target, &backup).map_err(io)?;
        }
        let restore = || {
            let _ = fs::remove_dir_all(&target);
            if backup.exists() {
                let _ = fs::rename(&backup, &target);
            }
        };
        if let Err(error) = fs::rename(staged, &target) {
            restore();
            return Err(io(error));
        }
        let addon = Addon {
            key: key.to_string(),
            name: manifest.name.clone(),
            manifest: Arc::clone(manifest),
            digest,
            installed_at: existing
                .as_ref()
                .map(|existing| existing.installed_at)
                .unwrap_or_else(crate::now_ms),
            methods,
            repository,
            enabled: existing.as_ref().is_none_or(|existing| existing.enabled),
        };
        let saved = {
            let mut index = lock(&self.index);
            let before = index.addons.insert(key.to_string(), addon.clone());
            let saved = self.save(&index);
            if saved.is_err() {
                match before {
                    Some(before) => index.addons.insert(key.to_string(), before),
                    None => index.addons.remove(key),
                };
            }
            saved
        };
        if let Err(error) = saved {
            restore();
            return Err(error);
        }
        let _ = fs::remove_dir_all(&backup);
        Ok(self.view(&addon))
    }

    /// Runs the package once, where it can only write in `data`, to read the methods it registers
    /// (pkgs.dart:1338-1358): a download provider needs `download`, a link reader `handleUrl`.
    fn validate(
        &self,
        key: &str,
        manifest: &Arc<Manifest>,
        code: &str,
        data: &Path,
    ) -> Result<Vec<String>, String> {
        let link = Link {
            network: Arc::new(Network::new(NetworkSetup {
                permissions: manifest.permissions.clone(),
                user_agent: self.user_agent(),
                timeout: request_timeout(manifest),
                saved: json!({}),
                keep: Arc::new(|_: Value| {}),
            })),
            session: None,
        };
        let running = self.start(
            key,
            manifest,
            code,
            data.to_path_buf(),
            Vec::new(),
            Hooks::default(),
            SHARED_HEAP_MB,
            &link,
        )?;
        let methods = running.engine.methods().to_vec();
        close(&running);
        let has = |name: &str| methods.iter().any(|method| method == name);
        if manifest.downloads() && !has("download") {
            return Err("Download provider has no registered download() method".into());
        }
        if (manifest.handles_links() || !manifest.downloads()) && !has("handleUrl") {
            return Err("Playlist-link addon has no registered handleUrl() method".into());
        }
        Ok(methods)
    }
}

/// What [`Packages::place`] needs.
struct Placing<'a> {
    key: &'a str,
    manifest: &'a Arc<Manifest>,
    code: &'a str,
    files: &'a [(String, Vec<u8>)],
    staged: &'a Path,
    digest: String,
    repository: Option<String>,
    existing: Option<Addon>,
}

/// What a download needs of an add-on.
pub(crate) struct Parts {
    pub(crate) manifest: Arc<Manifest>,
    pub(crate) network: Arc<Network>,
    pub(crate) session: Option<Arc<Session>>,
    pub(crate) settings: Value,
}

/// Calls `method` of a runtime as Lokal's runtime does (rt.dart:207-255): at most `timeout`; a timeout or a cancel
/// closes it; a result `{success: false}` or a failure, while the add-on's gateway cooldown lasts, becomes
/// [`Trouble::RateLimited`].
pub(crate) fn invoke(
    running: &Running,
    session: Option<&Session>,
    method: &str,
    args: &[Value],
    timeout: Duration,
) -> Result<Value, Trouble> {
    match running.engine.invoke(method, args, timeout) {
        Ok(value) => {
            if value.get("success") == Some(&Value::Bool(false))
                && let Some(trouble) = session.and_then(rate_limited)
            {
                return Err(trouble);
            }
            Ok(value)
        }
        Err(error) => {
            if let Some(trouble) = session.and_then(rate_limited) {
                return Err(trouble);
            }
            let message = error.lines().next().unwrap_or_default().to_string();
            let lower = message.to_lowercase();
            if lower.contains("timed out")
                || lower.contains("cancelled")
                || lower.contains("closed")
            {
                close(running);
            }
            Err(Trouble::Failed(message))
        }
    }
}

/// Stops a runtime: its host's waits and transfers, then its engine.
pub(crate) fn close(running: &Running) {
    running.host.abort();
    running.engine.close();
}

fn rate_limited(session: &Session) -> Option<Trouble> {
    session
        .rate_limited_for()
        .map(|seconds| Trouble::RateLimited {
            seconds,
            message: rate_limit_message(seconds),
        })
}

/// Sorts a failure into what the core acts on: the gateway's cooldown, a verification a person must pass, a song
/// the add-on cannot give (or `unavailable` when nothing else fits), or else words to show.
pub(crate) fn sort_trouble(
    trouble: Trouble,
    display_name: &str,
    network: &Network,
    session: Option<&Session>,
    unavailable: bool,
) -> Trouble {
    let (message, was_unavailable) = match trouble {
        Trouble::Failed(message) => (message, false),
        Trouble::Unavailable(message) => (message, true),
        other => return other,
    };
    if let Some(trouble) = session.and_then(rate_limited) {
        return trouble;
    }
    let lower = message.to_lowercase();
    if lower.starts_with("the add-on service is rate limiting") {
        let seconds = lower
            .split("try again in ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|seconds| seconds.parse().ok())
            .unwrap_or(60);
        return Trouble::RateLimited { seconds, message };
    }
    if needs_verification(&lower) {
        let url = session
            .and_then(Session::pending_url)
            .or_else(|| crate::auth::pending_url(network));
        return Trouble::Verify {
            url,
            message: format!(
                "{display_name} needs verification. Complete verification and retry this song."
            ),
        };
    }
    if unavailable || was_unavailable || cannot_give(&lower) {
        return Trouble::Unavailable(message);
    }
    Trouble::Failed(message)
}

/// `verify_required|verification_required|needs.?verification|verify.*(?:required|access)` (lowercase text).
fn needs_verification(text: &str) -> bool {
    if text.contains("verify_required") || text.contains("verification_required") {
        return true;
    }
    let needs = text.match_indices("needs").any(|(at, word)| {
        let rest = &text[at + word.len()..];
        rest.starts_with("verification")
            || rest
                .chars()
                .next()
                .is_some_and(|c| rest[c.len_utf8()..].starts_with("verification"))
    });
    needs
        || text.match_indices("verify").any(|(at, word)| {
            let rest = &text[at + word.len()..];
            rest.contains("required") || rest.contains("access")
        })
}

/// `not_found|no .*track match|cannot download this track|unavailable|not available` (lowercase text).
fn cannot_give(text: &str) -> bool {
    [
        "not_found",
        "cannot download this track",
        "unavailable",
        "not available",
    ]
    .iter()
    .any(|words| text.contains(words))
        || text
            .match_indices("no ")
            .any(|(at, word)| text[at + word.len()..].contains("track match"))
}

/// `log\s*in|sign\s*in|connect|auth|account`, any case: an action that signs in.
fn signs_in(text: &str) -> bool {
    let text = text.to_lowercase();
    let then_in = |word: &str| {
        text.match_indices(word)
            .any(|(at, found)| text[at + found.len()..].trim_start().starts_with("in"))
    };
    text.contains("connect")
        || text.contains("auth")
        || text.contains("account")
        || then_in("log")
        || then_in("sign")
}

/// `^(https?:|[a-z]+:)`, any case: a link rather than words to search.
fn is_link(query: &str) -> bool {
    let letters = query.bytes().take_while(u8::is_ascii_alphabetic).count();
    letters > 0 && query.as_bytes().get(letters) == Some(&b':')
}

/// The first `searchBehavior.filters[].id` that means songs, else `tracks` (pkgs.dart:1635-1649).
fn search_filter(manifest: &Manifest) -> Value {
    manifest
        .raw
        .pointer("/searchBehavior/filters")
        .and_then(Value::as_array)
        .and_then(|filters| {
            filters.iter().find_map(|filter| {
                let id = filter.as_object()?.get("id")?;
                matches!(
                    text_of(Some(id)).as_str(),
                    "song" | "songs" | "track" | "tracks"
                )
                .then(|| id.clone())
            })
        })
        .unwrap_or_else(|| json!("tracks"))
}

/// The actions settings buttons and global actions declare.
fn declared_actions(manifest: &Manifest) -> Vec<String> {
    let buttons = manifest
        .settings
        .iter()
        .filter(|field| field.get("type").and_then(Value::as_str) == Some("button"))
        .map(|field| text_of(field.get("action")));
    let global = manifest
        .global_actions
        .iter()
        .map(|action| text_of(action.get("action")));
    buttons.chain(global).collect()
}

/// An add-on's own words for a failure: `error_message`, `error`, else `fallback`.
pub(crate) fn failure_text(result: &Value, fallback: &str) -> String {
    let said = text_of(
        result
            .get("error_message")
            .filter(|v| !v.is_null())
            .or_else(|| result.get("error")),
    );
    if said.trim().is_empty() {
        fallback.to_string()
    } else {
        said
    }
}

/// Amazon lists one recording in several places: the first stays, and takes an Atmos mode only a later one names.
fn collapse_repeats(tracks: Vec<MTrack>) -> Vec<MTrack> {
    let mut kept: Vec<MTrack> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for track in tracks {
        if let Some(&at) = seen.get(&track.id) {
            if track.atmos && !kept[at].atmos {
                let mode = atmos_mode(&track.raw);
                let first = &mut kept[at];
                first.atmos = true;
                if let (Value::Object(raw), Some(mode)) = (&mut first.raw, mode) {
                    raw.insert("audio_modes".into(), Value::String(mode.into()));
                }
            }
            continue;
        }
        seen.insert(track.id.clone(), kept.len());
        kept.push(track);
    }
    kept
}

/// The settings an add-on runs with: its schema's defaults, `qualitySettings` (each quality option's defaults),
/// then what was saved (pkgs.dart:935-960).
fn settings_of(manifest: &Manifest, saved: &Map<String, Value>) -> Value {
    let defaults = |fields: &[Value]| -> Map<String, Value> {
        fields
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|field| Some((text_of(field.get("key")), field.get("default")?.clone())))
            .collect()
    };
    let mut settings = defaults(&manifest.settings);
    let quality: Map<String, Value> = manifest
        .quality_options
        .iter()
        .map(|option| {
            let fields = option
                .get("settings")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            (text_of(option.get("id")), Value::Object(defaults(&fields)))
        })
        .collect();
    settings.insert("qualitySettings".into(), Value::Object(quality));
    for (key, value) in saved {
        settings.insert(key.clone(), value.clone());
    }
    Value::Object(settings)
}

/// Saves the `input` values `schema` allows into `target` (pkgs.dart:967-996): buttons skipped, a secret given as
/// `""` keeps its value, booleans must be booleans, numbers numbers, a select one of its options; text is cut at
/// 8000 characters. `Invalid setting: <key>`.
fn assign(
    schema: &[Value],
    input: Option<&Map<String, Value>>,
    target: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(input) = input else {
        return Ok(());
    };
    for field in schema.iter().filter_map(Value::as_object) {
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "button" {
            continue;
        }
        let name = text_of(field.get("key"));
        let Some(value) = input.get(&name) else {
            continue;
        };
        if field.get("secret") == Some(&Value::Bool(true)) && value.as_str() == Some("") {
            continue;
        }
        let invalid = || format!("Invalid setting: {name}");
        let text = text_of(Some(value));
        let stored = match kind {
            "boolean" if value.is_boolean() => value.clone(),
            "boolean" => return Err(invalid()),
            "number" => number_of(&text).ok_or_else(invalid)?,
            _ => {
                if kind == "select" {
                    let options: Vec<Value> = field
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|options| {
                            options
                                .iter()
                                .map(|option| match option {
                                    Value::Object(option) => {
                                        option.get("value").cloned().unwrap_or(Value::Null)
                                    }
                                    other => other.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !options.contains(value) {
                        return Err(invalid());
                    }
                }
                Value::String(text.chars().take(SETTING_TEXT).collect())
            }
        };
        target.insert(name, stored);
    }
    Ok(())
}

/// A number written as text (`"5"`, `" 2.5 "`), as a JSON number.
fn number_of(text: &str) -> Option<Value> {
    let text = text.trim();
    if let Ok(whole) = text.parse::<i64>() {
        return Some(Value::from(whole));
    }
    let real = text.parse::<f64>().ok().filter(|real| real.is_finite())?;
    serde_json::Number::from_f64(real).map(Value::Number)
}

fn request_timeout(manifest: &Manifest) -> Duration {
    if manifest.name.to_ascii_lowercase().contains("tidal") {
        TIDAL_REQUEST_TIMEOUT
    } else {
        REQUEST_TIMEOUT
    }
}

fn read_repository(
    url: &str,
    official: bool,
    user_agent: &str,
) -> Result<(Option<String>, Vec<Entry>), String> {
    let bytes = registry::fetch(url, REGISTRY_LIMIT, user_agent)?;
    let text = String::from_utf8(bytes).map_err(|_| "Invalid Morneye registry".to_string())?;
    registry::parse(&text, official)
}

/// A name for a new folder: the time, then random hex.
fn unique() -> String {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("{micros}-{}", crate::hex(&crate::random_bytes(4)))
}

/// Writes a whole file at once: a temporary file beside it, then renamed over it.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("index.json");
    let temp = path.with_file_name(format!("{name}.{}.tmp", unique()));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

/// `base` joined with an archive path, refusing what Windows would read as something else: a drive or stream
/// (`:`), a device name (`CON`, `NUL`, …), characters it does not allow, or a name ending in a dot or a space.
fn safe_join(base: &Path, name: &str) -> Result<PathBuf, String> {
    const DEVICES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let mut path = base.to_path_buf();
    for part in name.split('/').filter(|part| !part.is_empty()) {
        let device = part
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_ascii_lowercase();
        let odd = part
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\\'));
        if odd
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || part.ends_with(' ')
            || DEVICES.contains(&device.as_str())
        {
            return Err(UNSAFE_PATH.into());
        }
        path.push(part);
    }
    let inside = path.starts_with(base)
        && path
            .components()
            .skip(base.components().count())
            .all(|component| matches!(component, Component::Normal(_)));
    if inside && path != base {
        Ok(path)
    } else {
        Err(UNSAFE_PATH.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: Value) -> Manifest {
        let mut raw = json!({
            "name": "demo-addon",
            "displayName": "Demo",
            "version": "1.0.0",
            "type": ["download_provider"],
            "permissions": { "network": ["api.example.com"], "storage": true, "file": true }
        });
        if let (Value::Object(raw), Value::Object(extra)) = (&mut raw, extra) {
            raw.extend(extra);
        }
        Manifest::read(raw).unwrap()
    }

    #[test]
    fn settings_merge_defaults_quality_and_saved() {
        let manifest = manifest(json!({
            "settings": [
                { "key": "country", "type": "string", "default": "US" },
                { "key": "token", "type": "string" },
                { "key": "go", "type": "button", "action": "go" }
            ],
            "qualityOptions": [
                { "id": "LOSSLESS", "settings": [{ "key": "fallback", "type": "boolean", "default": true }] },
                { "id": "HIGH" }
            ]
        }));
        let mut saved = Map::new();
        saved.insert("country".into(), json!("GB"));
        let settings = settings_of(&manifest, &saved);
        assert_eq!(settings["country"], "GB");
        assert_eq!(settings.get("token"), None);
        assert_eq!(
            settings["qualitySettings"],
            json!({ "LOSSLESS": { "fallback": true }, "HIGH": {} })
        );
        assert_eq!(settings_of(&manifest, &Map::new())["country"], "US");
    }

    #[test]
    fn settings_are_checked_against_their_schema() {
        let schema = vec![
            json!({ "key": "flag", "type": "boolean" }),
            json!({ "key": "count", "type": "number" }),
            json!({ "key": "pick", "type": "select", "options": ["a", { "value": "b", "label": "B" }] }),
            json!({ "key": "secret", "type": "string", "secret": true }),
            json!({ "key": "name", "type": "string" }),
            json!({ "key": "go", "type": "button", "action": "go" }),
        ];
        let mut target = Map::new();
        target.insert("secret".into(), json!("kept"));
        let input = json!({ "flag": true, "count": " 2.5 ", "pick": "b", "secret": "", "name": 7, "go": "x",
                            "other": "ignored" });
        assign(&schema, input.as_object(), &mut target).unwrap();
        assert_eq!(target["flag"], true);
        assert_eq!(target["count"], 2.5);
        assert_eq!(target["pick"], "b");
        assert_eq!(target["secret"], "kept");
        assert_eq!(target["name"], "7");
        assert!(!target.contains_key("go") && !target.contains_key("other"));

        let whole = json!({ "count": "12" });
        assign(&schema, whole.as_object(), &mut target).unwrap();
        assert_eq!(target["count"], 12);
        let long = json!({ "name": "x".repeat(9000) });
        assign(&schema, long.as_object(), &mut target).unwrap();
        assert_eq!(target["name"].as_str().unwrap().len(), SETTING_TEXT);

        for bad in [
            json!({ "flag": "yes" }),
            json!({ "count": "many" }),
            json!({ "pick": "c" }),
        ] {
            let error = assign(&schema, bad.as_object(), &mut target).unwrap_err();
            assert!(error.starts_with("Invalid setting: "), "{error}");
        }
    }

    #[test]
    fn sign_in_actions_are_recognized() {
        assert!(signs_in("login Log in"));
        assert!(signs_in("startLogIn"));
        assert!(signs_in("x Sign  in with your account"));
        assert!(signs_in("connectAccount"));
        assert!(signs_in("oauth"));
        assert!(!signs_in("forceCleanup Clear Cache"));
        assert!(!signs_in("blog catalogue"));
        // Lokal's pattern takes any `connect`, so a connection test counts too.
        assert!(signs_in("testConnection Test"));
    }

    #[test]
    fn failures_are_sorted() {
        assert!(needs_verification("verify_required"));
        assert!(needs_verification("this needs verification"));
        assert!(needs_verification("needs-verification"));
        assert!(needs_verification("please verify your access"));
        assert!(!needs_verification("verified"));
        assert!(cannot_give("track not_found"));
        assert!(cannot_give("the addon cannot download this track"));
        assert!(cannot_give("no exact track match"));
        assert!(cannot_give("song unavailable in your region"));
        assert!(!cannot_give("http 500"));
    }

    #[test]
    fn links_are_told_from_words() {
        assert!(is_link("https://example.com/track/1"));
        assert!(is_link("spotify:track:1"));
        assert!(is_link("Qobuz:album:1"));
        assert!(!is_link("daft punk"));
        assert!(!is_link(":x"));
        assert!(!is_link("ac/dc: live"));
    }

    #[test]
    fn the_search_filter_is_the_songs_one() {
        let songs = manifest(
            json!({ "searchBehavior": { "filters": [{ "id": "albums" }, { "id": "songs" }] } }),
        );
        assert_eq!(search_filter(&songs), json!("songs"));
        let none = manifest(json!({ "searchBehavior": { "filters": [{ "id": "albums" }] } }));
        assert_eq!(search_filter(&none), json!("tracks"));
    }

    fn track(id: &str, extra: Value) -> MTrack {
        let mut raw = json!({ "id": id, "name": "s" });
        if let (Value::Object(raw), Value::Object(extra)) = (&mut raw, extra) {
            raw.extend(extra);
        }
        MTrack::of(&raw, true).unwrap()
    }

    #[test]
    fn repeats_collapse_and_keep_a_later_atmos_mode() {
        let tracks = vec![
            track("A1", json!({ "artists": "x" })),
            track("B2", json!({})),
            track("A1", json!({ "audioQualities": ["ATMOS"] })),
        ];
        let kept = collapse_repeats(tracks);
        assert_eq!(kept.len(), 2);
        assert!(kept[0].atmos);
        assert_eq!(kept[0].raw["audio_modes"], "DOLBY_ATMOS");
        assert_eq!(kept[0].artists, "x");
    }

    #[test]
    fn remembered_songs_are_bounded() {
        let mut remembered = Remembered::default();
        for i in 0..(REMEMBERED + 5) {
            remembered.put(format!("k:{i}"), track(&i.to_string(), json!({})));
        }
        assert!(remembered.get("k:0").is_none());
        assert!(remembered.get("k:5").is_some());
        assert_eq!(remembered.tracks.len(), REMEMBERED);
        // Used again, it stays.
        remembered.put("k:new".into(), track("new", json!({})));
        assert!(remembered.get("k:5").is_some());
        remembered.forget("k:");
        assert!(remembered.tracks.is_empty() && remembered.order.is_empty());
    }

    #[test]
    fn archive_paths_are_joined_safely() {
        let base = Path::new("staging").join("1");
        assert_eq!(
            safe_join(&base, "lib/a.js").unwrap(),
            base.join("lib").join("a.js")
        );
        for bad in ["a:b.js", "CON.js", "nul", "a/..", "x.", "x ", "a?b", ""] {
            assert!(safe_join(&base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn failure_words() {
        assert_eq!(failure_text(&json!({ "error": "nope" }), "x"), "nope");
        assert_eq!(
            failure_text(&json!({ "error_message": "first", "error": "second" }), "x"),
            "first"
        );
        assert_eq!(
            failure_text(&json!({ "success": false }), "fallback"),
            "fallback"
        );
    }

    fn folder() -> PathBuf {
        std::env::temp_dir().join(format!(
            "stash-mornye-packages-{}",
            crate::hex(&crate::random_bytes(6))
        ))
    }

    #[test]
    fn an_index_is_written_whole_and_read_back() {
        let root = folder();
        let path = root.join("index.json");
        let mut index = Index::default();
        index.repositories.push(Repo {
            id: "abc".into(),
            url: "https://example.org/registry.json".into(),
            name: "Example".into(),
            refreshed_at: Some(5),
            entries: vec![Entry {
                id: "demo".into(),
                version: "1.0.0".into(),
                sha256: "0".repeat(64),
                download_url: "https://example.org/demo.sflx".into(),
                display_name: "Demo".into(),
                description: String::new(),
                min_app_version: None,
            }],
            error: Some("Repository returned HTTP 404".into()),
        });
        atomic_write(&path, index.to_json().to_string().as_bytes()).unwrap();
        let read: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(Index::from_json(&read).repositories, index.repositories);
        // Nothing temporary is left beside it.
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_folder_has_the_official_repository_only() {
        let root = folder();
        let packages = Packages::open(root.clone(), [7u8; 32], "1.0.0").unwrap();
        let repositories = packages.repositories();
        assert_eq!(repositories.len(), 1);
        assert!(repositories[0].official);
        assert_eq!(repositories[0].url, OFFICIAL);
        assert_eq!(
            packages.remove_repository(&repositories[0].id).unwrap_err(),
            "The official repository cannot be removed"
        );
        assert!(packages.installed().is_empty());
        assert!(packages.catalogue().is_empty());
        assert_eq!(packages.remove("0123456789").unwrap_err(), NOT_FOUND);
        assert!(matches!(
            packages.search("0123456789", "x", 5),
            Err(Trouble::Missing(_))
        ));
        assert!(!packages.is_callback("0123456789", "https://example.org/cb"));
        drop(packages);
        // Opened again, the same.
        let again = Packages::open(root.clone(), [7u8; 32], "1.0.0").unwrap();
        assert_eq!(again.repositories()[0].id, repositories[0].id);
        drop(again);
        let _ = fs::remove_dir_all(root);
    }
}
