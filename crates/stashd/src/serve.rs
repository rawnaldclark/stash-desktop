//! The core's local API: what every shell (the desktop window, later a phone remote) talks to.
//!
//! It listens on this computer only. A request must name this server as its Host (so a web page cannot reach it by
//! pointing a domain name at 127.0.0.1) and, when it comes from a page, that page must be the UI this server serves
//! or the desktop window.
//!
//! The API reads the library (counts, a page of tracks or albums at any position, search), drives the player, and
//! keeps the listener's add-ons: adding one from a link, their order, searching them, and playing what they find.
//! It takes in a library from another Stash (`POST /api/import`) and lists what came with it: liked songs and
//! playlists.
//! Several requests are answered at once, so an add-on that is slow to answer holds up its own answer and nothing
//! else.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use stash_core::addons::{Addon, AddonError, Code, Delivery, StreamRequest, Track};
use stash_core::audio::{Output, find_library};
use stash_core::blocks::BlockKind;
use stash_core::import::ImportError;
use stash_core::import::Song;
use stash_core::player::{
    AddonTrack, Failure, Located, OnPlay, Player, QueueItem, Repeat, Resolver, Settings,
};
use stash_core::scan::{ScanReport, scan_folder};
use stash_core::{Library, StoredAddon, matching};
use tiny_http::{Header, Method, Request, Response, Server};
use url::Url;

/// The largest page a list may ask for.
const PAGE_CAP: usize = 500;
/// The most songs one change acts on at once (as many as can be picked in a list).
pub(crate) const PICKS_CAP: usize = 5000;
/// The most credits one ask splits, and the longest credit split (a longer one is one name).
const CREDITS_CAP: usize = 400;
const CREDIT_LEN: usize = 400;
/// The largest body the API reads.
const BODY_CAP: u64 = 4 * 1024 * 1024;
/// The most tracks one queue holds.
const QUEUE_CAP: usize = 5_000;
/// The largest file an import takes. (The shared library file has its own, smaller limit; a phone's database or
/// backup can be bigger.)
const IMPORT_CAP: u64 = 512 * 1024 * 1024;
/// How many requests are answered at once.
const WORKERS: usize = 8;
/// The longest text taken from an add-on's answer, or from the window about an add-on's track.
const TEXT_CAP: usize = 300;
/// How long an add-on that did not have a song is not asked for it again.
const MISS_MEMORY: Duration = Duration::from_secs(6 * 3600);
/// How long after it last found a song an add-on's "I don't have it" is believed (see [`Lacks`]).
const FINDING: Duration = Duration::from_secs(30 * 60);
/// The most tracks of an add-on one search shows, and one play request queues.
const ADDON_TRACKS_CAP: usize = 200;

pub struct Options {
    pub db: PathBuf,
    /// A folder holding the built UI (`index.html` and its files), served at `/`.
    pub ui: Option<PathBuf>,
    pub port: u16,
    /// Where a measurement the UI posts to `/api/bench-result` is written; the server then stops.
    pub bench_out: Option<PathBuf>,
    /// Where the player sends its sound; `None` runs the core without a player.
    pub audio: Option<Output>,
    /// The libmpv file for the player; without it, the one `STASH_MPV_LIBRARY` names or one beside the program.
    pub mpv: Option<PathBuf>,
    /// When a play counts, in seconds (see the player's settings); `None` is the rule the other Stashes use.
    pub play_counts_between_seconds: Option<(f64, f64)>,
}

/// What the program around the core can be asked to show: the full window, or the mini player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Full,
    Mini,
}

/// The program around the core that owns its windows (the desktop shell), when there is one. The page asks it for a
/// view through the core (`POST /api/window`), so the page talks to nothing but the core; it says whether it did.
pub type WindowShell = Arc<dyn Fn(View) -> bool + Send + Sync>;

/// The program around the core that holds keys system-wide (the desktop shell), for the shortcuts that work in any app.
/// Handed the keys to hold, by action (`("next", "Ctrl+Alt+PageDown")`), it lets go of any it held before and answers
/// those it could not hold, with why. A key pressed comes back to the core as `POST /api/shortcut {"action"}`.
pub type KeysShell = Arc<dyn Fn(&[(String, String)]) -> Vec<(String, String)> + Send + Sync>;

pub(crate) type Reply = Response<std::io::Cursor<Vec<u8>>>;

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn reply(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
    Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
}

/// A picture the window may keep for a while: unlike every other answer, which is as things are this moment.
fn reply_picture(content_type: &str, body: Vec<u8>) -> Reply {
    Response::from_data(body)
        .with_status_code(200)
        .with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "private, max-age=3600"))
        // The window asks for a picture twice: plainly, to draw it, and as another origin, to read its colours.
        // The kept answer to the first must not be handed to the second: it does not say the window may read it.
        .with_header(header("Vary", "Origin"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
}

pub(crate) fn reply_json(status: u16, value: Value) -> Reply {
    reply(status, "application/json", value.to_string().into_bytes())
}

pub(crate) fn refuse(status: u16, why: &str) -> Reply {
    reply_json(status, json!({ "error": { "message": why } }))
}

fn find<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// The origins a page may call the API from: this server's own, and the desktop window's.
fn allowed_origin(origin: &str, port: u16) -> bool {
    origin == format!("http://127.0.0.1:{port}")
        || origin == format!("http://localhost:{port}")
        || window_origin(origin)
}

/// The desktop window's origins. A browser gives the first two to any page at tauri.localhost too, which another
/// program on this computer may serve: from them, a request also needs the window's key (see [`Core::with_key`]).
fn window_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://tauri.localhost" | "https://tauri.localhost" | "tauri://localhost"
    )
}

/// The key a request carries (`?k=`).
fn key_of(url: &str) -> Option<String> {
    Url::parse(&format!("http://local{url}"))
        .ok()?
        .query_pairs()
        .find(|(name, _)| name == "k")
        .map(|(_, key)| key.into_owned())
}

/// The origin of an address (its scheme, host and port), as a browser names it; `tauri://localhost` too, which the
/// URL standard leaves without one.
fn origin_of(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => String::new(),
    }
}

fn capped(text: &str) -> String {
    text.chars().take(TEXT_CAP).collect()
}

/// An add-on the listener added, as the core holds it while it runs.
pub(crate) struct Loaded {
    pub(crate) stored: StoredAddon,
    /// Ready to be asked, or why it cannot be (what was kept no longer passes the checks).
    addon: std::result::Result<Addon, String>,
}

impl Loaded {
    pub(crate) fn usable(&self) -> Option<&Addon> {
        self.addon.as_ref().ok().filter(|_| self.stored.enabled)
    }

    /// The add-on as the window lists it. Its link is not part of it: a link can carry the listener's own key.
    fn listed(&self) -> Value {
        let kept: Value = serde_json::from_str(&self.stored.manifest).unwrap_or(Value::Null);
        let text = |name: &str| kept.get(name).and_then(Value::as_str).map(capped);
        let host = Url::parse(&self.stored.link)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        json!({
            "id": self.stored.id,
            "name": text("name").unwrap_or_else(|| self.stored.id.clone()),
            "version": text("version"),
            "description": text("description"),
            "host": host,
            "resources": kept.get("resources").cloned().unwrap_or_else(|| json!([])),
            "enabled": self.stored.enabled,
            "problem": self.addon.as_ref().err(),
        })
    }
}

/// The listener's add-ons, in the listener's order.
pub(crate) type Registry = Arc<RwLock<Vec<Arc<Loaded>>>>;

fn load_addons(library: &Library) -> Result<Vec<Arc<Loaded>>> {
    Ok(library
        .addons()?
        .into_iter()
        .map(|stored| {
            let addon =
                Addon::from_stored(&stored.link, &stored.manifest).map_err(|e| e.in_words());
            Arc::new(Loaded { stored, addon })
        })
        .collect())
}

/// Where one add-on's track plays from.
fn stream_from(addon: &Addon, id: &str, track: &str) -> std::result::Result<Located, AddonError> {
    addon
        .stream(track, &StreamRequest::default())
        .map(|info| Located {
            locator: info.url,
            tier: Some(info.tier.as_str().to_string()),
            addon: Some(id.to_string()),
        })
}

/// What each add-on was found not to have, and when: (the song, the add-on). It is not asked again for a while, so a
/// queue of songs it lacks does not spend the listener's allowance there on the same answers. And when each add-on
/// last found a song it was asked for: its "I don't have it" is believed only while it is finding others (an add-on
/// whose allowance is spent, or whose lookup is broken, may answer "nothing" to everything, and still play the names
/// it gave before).
#[derive(Default)]
pub(crate) struct Lacks {
    /// With whether that "no" was believed when it was said.
    missed: Mutex<HashMap<(i64, String), (Instant, bool)>>,
    /// Songs found by asking, not names given before.
    found: Mutex<HashMap<String, Instant>>,
    /// For the window: when each add-on last gave a song, and its last trouble (when, in words).
    found_at: Mutex<HashMap<String, i64>>,
    troubles: Mutex<HashMap<String, (i64, String)>>,
}

impl Lacks {
    /// Forgets what the add-ons were found not to have of the song `id` (the listener fixed it).
    pub(crate) fn forget(&self, id: i64) {
        self.missed
            .lock()
            .unwrap()
            .retain(|(song, _), _| *song != id);
    }

    /// Forgets the "no"s that were not believed for the songs `ids` (every song, when none are named): the listener
    /// asks to try them again.
    pub(crate) fn forget_doubted(&self, ids: &[i64]) {
        self.missed
            .lock()
            .unwrap()
            .retain(|(song, _), (_, believed)| {
                *believed || !(ids.is_empty() || ids.contains(song))
            });
    }

    /// Forgets what the add-on was found not to have (it was added again, or switched back on).
    pub(crate) fn forget_addon(&self, addon: &str) {
        self.missed.lock().unwrap().retain(|(_, a), _| a != addon);
    }

    /// The add-on gave a song (for the window).
    fn gave(&self, addon: &str) {
        self.found_at
            .lock()
            .unwrap()
            .insert(addon.to_string(), now_ms());
    }

    /// The add-on found a song it was asked for: its "no"s are believed for a while.
    fn found_when_asked(&self, addon: &str) {
        self.found
            .lock()
            .unwrap()
            .insert(addon.to_string(), Instant::now());
        self.gave(addon);
    }

    /// The add-on said it does not have the song; `believed`: it said so while finding others.
    fn missed(&self, id: i64, addon: &str, believed: bool) {
        self.missed
            .lock()
            .unwrap()
            .insert((id, addon.to_string()), (Instant::now(), believed));
    }

    /// A "no" from a little while ago, and whether it was believed.
    fn remembered(&self, id: i64, addon: &str) -> Option<bool> {
        self.missed
            .lock()
            .unwrap()
            .get(&(id, addon.to_string()))
            .filter(|(at, _)| at.elapsed() < MISS_MEMORY)
            .map(|(_, believed)| *believed)
    }

    /// Notes the add-on's trouble (it could not be reached, its allowance is spent…), for the window.
    fn troubled(&self, addon: &str, e: &AddonError) {
        self.troubles
            .lock()
            .unwrap()
            .insert(addon.to_string(), (now_ms(), e.in_words()));
    }

    /// When the add-on last gave a song, and its last trouble.
    fn health(&self, addon: &str) -> (Option<i64>, Option<(i64, String)>) {
        (
            self.found_at.lock().unwrap().get(addon).copied(),
            self.troubles.lock().unwrap().get(addon).cloned(),
        )
    }

    fn finding(&self, addon: &str) -> bool {
        self.found
            .lock()
            .unwrap()
            .get(addon)
            .is_some_and(|at| at.elapsed() < FINDING)
    }
}

/// Asks the listener's add-ons that can (`can`) for the library's song `id`, in the listener's order, and does `act`
/// with the first that has it: the add-on's own name for the song is tried first, then the add-on is asked whether
/// it has the song (or searched, when it can only be searched; a result is taken only when it is the same song by
/// the contract's rule). Answers what `act` gave and which add-on gave it. `none` says why when no add-on can.
///
/// When every add-on that could be asked said it does not have the song, and each of them is finding other songs,
/// the song goes on the list of songs no add-on could find (for the listener to fix); found, it leaves that list.
/// Trouble with an add-on (it could not be reached, its allowance is spent) never puts a song there.
pub(crate) fn through_addons<T>(
    registry: &Registry,
    library: &Mutex<Library>,
    lacks: &Lacks,
    id: i64,
    none: &str,
    can: impl Fn(&Addon) -> bool,
    act: impl Fn(&Addon, &str, &str) -> std::result::Result<T, AddonError>,
) -> std::result::Result<(T, String), Failure> {
    let gone = |why: &str| Failure {
        message: why.to_string(),
        miss: true,
        ..Default::default()
    };
    let described = library
        .lock()
        .unwrap()
        .describe(id)
        .map_err(|e| Failure {
            message: e.to_string(),
            ..Default::default()
        })?
        .ok_or_else(|| gone("it is no longer in the library"))?;
    let addons: Vec<Arc<Loaded>> = registry
        .read()
        .unwrap()
        .iter()
        .filter(|a| a.usable().is_some_and(&can))
        .cloned()
        .collect();
    // No add-on to ask (none added, or switched off): not the song's own trouble.
    if addons.is_empty() {
        return Err(Failure {
            nothing_to_ask: true,
            ..gone(none)
        });
    }
    // Trouble with an add-on is not "it does not have the song": it is said, and asking again may help.
    let mut trouble: Option<AddonError> = None;
    // The add-ons that said they do not have it (now, or a little while ago), and whether one had it but could not
    // give it.
    let mut said_no: Vec<&str> = Vec::new();
    let mut had_it = false;
    // Whether an add-on was asked anything this time (not only "no"s remembered from before).
    let mut any_asked = false;
    // A name the add-on gave before says nothing of how it answers when asked; a song it was asked for does.
    let found = |addon_id: &str, asked: bool| {
        if asked {
            lacks.found_when_asked(addon_id);
        } else {
            lacks.gave(addon_id);
        }
        let _ = library.lock().unwrap().found_again(id);
    };
    for loaded in &addons {
        let Some(addon) = loaded.usable() else {
            continue;
        };
        let addon_id = loaded.stored.id.as_str();
        let known = described
            .refs
            .iter()
            .find(|(source, _)| source == addon_id)
            .map(|(_, theirs)| theirs.clone());
        if let Some(track) = &known {
            any_asked = true;
            match act(addon, addon_id, track) {
                Ok(done) => {
                    found(addon_id, false);
                    return Ok((done, addon_id.to_string()));
                }
                // The add-on no longer knows that name: it is forgotten, and the add-on is asked for the song.
                Err(e) if e.code == Code::NotFound => {
                    let _ = library.lock().unwrap().drop_ref(id, addon_id);
                }
                Err(e) if e.is_miss() => {
                    had_it = true;
                    continue;
                }
                Err(e) => {
                    lacks.troubled(addon_id, &e);
                    trouble = Some(e);
                    continue;
                }
            }
        }
        // A "no" from a little while ago counts only if it was believed when it was said. One that was not is asked
        // again once the add-on is finding other songs: its answer counts then.
        if let Some(believed) = lacks.remembered(id, addon_id) {
            if believed {
                said_no.push(addon_id);
                continue;
            }
            if !lacks.finding(addon_id) {
                continue;
            }
        }
        let resolving = addon.offers_resource("resolve");
        let asked = if resolving {
            // As the song is read, then each other way it may be read (a row from YouTube), until the add-on has it.
            let mut answer = Ok(None);
            for reading in std::iter::once(&described.wanted).chain(&described.also) {
                answer = addon.resolve(reading, None);
                if !matches!(&answer, Ok(None)) {
                    break;
                }
            }
            answer
        } else if addon.offers_resource("search") {
            addon
                .search(&matching::query(&described.wanted), 20)
                .map(|found| described.pick(&found).cloned())
        } else {
            continue;
        };
        any_asked = true;
        let track = match asked {
            Ok(Some(track)) => track,
            // It does not have the song, or what it answered cannot be this song.
            Ok(None) => {
                lacks.missed(id, addon_id, lacks.finding(addon_id));
                said_no.push(addon_id);
                continue;
            }
            // A wrong answer to "do you have this song" (the host refused it as another song) is a no; a broken
            // search answer is not.
            Err(e) if e.is_miss() || e.code == Code::InvalidResponse => {
                let no = e.is_miss() || resolving;
                lacks.missed(id, addon_id, no && lacks.finding(addon_id));
                if no {
                    said_no.push(addon_id);
                }
                continue;
            }
            Err(e) => {
                lacks.troubled(addon_id, &e);
                trouble = Some(e);
                continue;
            }
        };
        match act(addon, addon_id, &track.id) {
            Ok(done) => {
                let _ = library.lock().unwrap().set_ref(id, addon_id, &track.id);
                found(addon_id, true);
                return Ok((done, addon_id.to_string()));
            }
            Err(e) if e.is_miss() => {
                had_it = true;
                continue;
            }
            Err(e) => {
                lacks.troubled(addon_id, &e);
                trouble = Some(e);
                continue;
            }
        }
    }
    Err(match trouble {
        Some(e) => Failure {
            message: e.in_words(),
            code: Some(e.code),
            retry_after: e.retry_after,
            ..Default::default()
        },
        None => {
            // Noted only when every add-on that can play songs and be asked for one said no (on the way to saving, an
            // add-on that only streams was not asked), and each of them is finding others.
            let askable: Vec<String> = registry
                .read()
                .unwrap()
                .iter()
                .filter(|a| {
                    a.usable().is_some_and(|addon| {
                        addon.offers_resource("stream")
                            && (addon.offers_resource("resolve") || addon.offers_resource("search"))
                    })
                })
                .map(|a| a.stored.id.clone())
                .collect();
            if !had_it
                && !said_no.is_empty()
                && askable.iter().all(|a| said_no.contains(&a.as_str()))
                && said_no.iter().all(|a| lacks.finding(a))
            {
                let _ = library.lock().unwrap().note_unfound(id, now_ms());
            }
            // Believed only when an add-on had it but could not give it, or the ones that said no are finding others.
            Failure {
                doubtful: !had_it
                    && (said_no.is_empty() || !said_no.iter().all(|a| lacks.finding(a))),
                asked: any_asked,
                ..gone("none of your add-ons has it")
            }
        }
    })
}

/// How the player finds where a track plays from.
///
/// An add-on's own track is asked of that add-on. A song the library knows by its description is asked of the
/// listener's add-ons in their order: the name an add-on gave the song before is tried first, else the add-on is
/// asked whether it has the song (the contract's `resolve`: which of its tracks is the same song is the add-on's to
/// say), and an add-on that can only search is searched for it ([`matching`]). The first add-on that gives a stream
/// wins, and the name it gave is kept for next time. A song played before may have a copy kept from then: it plays
/// from that, and no add-on is asked. A plain stream (not DASH or HLS, not encrypted) is relayed by the core, which
/// keeps a copy of it as it plays ([`crate::relay`]).
fn resolver(
    registry: Registry,
    library: Arc<Mutex<Library>>,
    lacks: Arc<Lacks>,
    relays: Arc<crate::relay::Relays>,
    offline: Arc<AtomicBool>,
    port: u16,
) -> Resolver {
    Arc::new(move |item: &QueueItem| {
        let gone = |why: &str| Failure {
            message: why.to_string(),
            miss: true,
            ..Default::default()
        };
        // Offline, no add-on is asked: a song plays only from a copy kept here. What cannot play is passed over.
        let offline = offline.load(Ordering::Relaxed);
        if let Some(wanted) = &item.addon {
            if offline {
                return Err(gone("Stash is offline"));
            }
            let loaded = registry
                .read()
                .unwrap()
                .iter()
                .find(|a| a.stored.id == wanted.addon)
                .cloned()
                .ok_or_else(|| gone("its add-on was removed"))?;
            if !loaded.stored.enabled {
                return Err(gone("its add-on is switched off"));
            }
            let addon = loaded.addon.as_ref().map_err(|why| gone(why))?;
            let addon_id = &loaded.stored.id;
            return match stream_from(addon, addon_id, &wanted.track) {
                Ok(located) => {
                    lacks.gave(addon_id);
                    let _ = library.lock().unwrap().note_addon_use(
                        addon_id,
                        stash_core::usage::Given::Stream,
                        now_ms(),
                    );
                    Ok(located)
                }
                Err(e) => {
                    if !e.is_miss() {
                        lacks.troubled(addon_id, &e);
                    }
                    Err(Failure {
                        message: e.in_words(),
                        miss: e.is_miss(),
                        code: Some(e.code),
                        retry_after: e.retry_after,
                        ..Default::default()
                    })
                }
            };
        }
        let Some(id) = item.id else {
            return Err(gone("it has nowhere to play from"));
        };
        if let Some(located) = relays.copy_of(&library, id) {
            return Ok(located);
        }
        if offline {
            return Err(gone("it is not on this computer, and Stash is offline"));
        }
        relays.not_handed(id);
        let keeping = library.lock().unwrap().played_cap_bytes().unwrap_or(0) > 0;
        through_addons(
            &registry,
            &library,
            &lacks,
            id,
            "it is not a file here, and there is no add-on to play it from",
            |addon| addon.offers_resource("stream"),
            |addon, addon_id, track| {
                addon.stream(track, &StreamRequest::default()).map(|info| {
                    let plain =
                        matches!(info.manifest, Delivery::None) && info.encrypted != Some(true);
                    let located = Located {
                        locator: info.url,
                        tier: Some(info.tier.as_str().to_string()),
                        addon: Some(addon_id.to_string()),
                    };
                    (plain, located)
                })
            },
        )
        .map(|((plain, located), addon_id)| {
            let _ = library.lock().unwrap().note_addon_use(
                &addon_id,
                stash_core::usage::Given::Stream,
                now_ms(),
            );
            if plain && keeping {
                relays.link(port, id, located)
            } else {
                located
            }
        })
    })
}

/// A play that counted is kept with its song, when the song is the library's.
fn keep_plays(library: Arc<Mutex<Library>>) -> OnPlay {
    Arc::new(move |item: &QueueItem, started_at_ms: i64| {
        if let Some(id) = item.id {
            let _ = library.lock().unwrap().record_play(id, started_at_ms);
        }
    })
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn addon_error(e: &AddonError) -> Value {
    json!({ "code": e.code.as_str(), "message": e.in_words(), "retry_after": e.retry_after })
}

/// A track an add-on found, as the window shows it. What an add-on sends is cut to size; a track without a usable
/// id is left out.
fn found_track(track: &Track) -> Option<Value> {
    if track.id.is_empty() || track.id.len() > 512 {
        return None;
    }
    Some(json!({
        "id": track.id,
        "title": capped(&track.title),
        "artist": capped(&track.artist),
        "album": track.album.as_deref().map(capped),
        "duration_ms": track.duration_ms,
        "max_tier": track.max_tier.map(|t| t.as_str()),
        "explicit": track.explicit,
        "art": track.art(),
    }))
}

/// The scan of the listener's music folders: running or not, and what it has done.
#[derive(Default, Clone, serde::Serialize)]
struct Scanning {
    running: bool,
    /// The folder being read.
    folder: Option<String>,
    /// What the scan did, so far or in the end, over all its folders.
    report: Option<ScanReport>,
    error: Option<String>,
    /// How many scans have ended since the core started: the window reads the library again when it goes up.
    finished: u64,
}

/// One report added to another: a scan of several folders says what it did in all.
fn in_all(done: &ScanReport, now: &ScanReport) -> ScanReport {
    let mut examples = done.unreadable_examples.clone();
    examples.extend(now.unreadable_examples.iter().cloned());
    examples.truncate(5);
    ScanReport {
        files: done.files + now.files,
        added: done.added + now.added,
        changed: done.changed + now.changed,
        unchanged: done.unchanged + now.unchanged,
        unreadable: done.unreadable + now.unreadable,
        unreadable_examples: examples,
        missing: done.missing + now.missing,
        seconds: done.seconds + now.seconds,
    }
}

/// What everything that answers requests shares.
pub(crate) struct State {
    pub(crate) library: Arc<Mutex<Library>>,
    options: Options,
    port: u16,
    /// The player, or the reason there is none (no audio engine found, or it would not start).
    pub(crate) player: std::result::Result<Player, String>,
    pub(crate) addons: Registry,
    scanning: Arc<Mutex<Scanning>>,
    /// What each add-on was found not to have (shared by playing and saving).
    pub(crate) lacks: Arc<Lacks>,
    /// Songs being saved as files.
    pub(crate) saving: crate::saving::Saving,
    /// Add-on streams the engine reads through the core, and the copies kept of them.
    pub(crate) relays: Arc<crate::relay::Relays>,
    /// Plays on their way to the listener's scrobbling services.
    pub(crate) scrobbling: Arc<crate::scrobbling::Scrobbling>,
    /// Stash is offline (see [`offline`]).
    pub(crate) offline: Arc<AtomicBool>,
    /// The windows' owner, when the core runs inside one.
    pub(crate) window: Option<WindowShell>,
    /// The key the window was given for this run, when there is a window (see [`Core::with_key`]).
    key: Option<String>,
    /// The shortcuts that work in any app.
    pub(crate) shortcuts: crate::shortcuts::Shortcuts,
    /// Goes up with each change to the likes and the playlists, so every window (the mini player too) can tell when
    /// its lists are behind: a change may come from another window, or from a shortcut.
    pub(crate) edits: AtomicU64,
}

impl State {
    /// Notes that the likes or the playlists changed. A list kept on this computer may have new songs to save.
    pub(crate) fn edited(&self) {
        self.edits.fetch_add(1, Ordering::Relaxed);
        self.saving.ring();
    }

    /// Starts reading `folders` into the library on a thread of its own, through a connection of its own, so the
    /// window goes on answering. False when a scan is running already.
    fn scan(&self, folders: Vec<String>) -> bool {
        {
            let mut scanning = self.scanning.lock().unwrap();
            if scanning.running {
                return false;
            }
            *scanning = Scanning {
                running: true,
                folder: folders.first().cloned(),
                finished: scanning.finished,
                ..Scanning::default()
            };
        }
        let (db, scanning) = (self.options.db.clone(), Arc::clone(&self.scanning));
        std::thread::spawn(move || {
            let outcome = (|| -> stash_core::Result<ScanReport> {
                let mut library = Library::open(&db)?;
                let mut done = ScanReport::default();
                for folder in &folders {
                    scanning.lock().unwrap().folder = Some(folder.clone());
                    let before = done.clone();
                    let report = scan_folder(&mut library, Path::new(folder), |so_far| {
                        scanning.lock().unwrap().report = Some(in_all(&before, so_far));
                    })?;
                    library.remember_folder(folder, now_ms())?;
                    done = in_all(&before, &report);
                }
                Ok(done)
            })();
            let mut scanning = scanning.lock().unwrap();
            scanning.running = false;
            scanning.finished += 1;
            match outcome {
                Ok(report) => scanning.report = Some(report),
                Err(e) => scanning.error = Some(e.to_string()),
            }
        });
        true
    }

    fn reload_addons(&self) -> Result<()> {
        let loaded = load_addons(&self.library.lock().unwrap())?;
        *self.addons.write().unwrap() = loaded;
        Ok(())
    }

    fn addons_reply(&self) -> Reply {
        let listed: Vec<Value> = self
            .addons
            .read()
            .unwrap()
            .iter()
            .map(|a| a.listed())
            .collect();
        reply_json(200, json!({ "addons": listed }))
    }
}

/// The core, listening but not yet answering: the port is known, so a window can be pointed at it.
pub struct Core {
    server: Server,
    state: State,
}

impl Core {
    /// Opens the library and takes the port (`0` asks the system for a free one).
    pub fn bind(options: Options) -> Result<Self> {
        let library = Library::open(&options.db)
            .with_context(|| format!("opening {}", options.db.display()))?;
        let server = Server::http(("127.0.0.1", options.port))
            .map_err(|e| anyhow!("cannot listen on 127.0.0.1:{}: {e}", options.port))?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .unwrap_or(options.port);
        let addons: Registry = Arc::new(RwLock::new(load_addons(&library)?));
        // Copies of songs as they were played sit next to the library, in a folder of their own.
        let relays = Arc::new(crate::relay::Relays::new(
            options
                .db
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("Played"),
            &library,
        ));
        let library = Arc::new(Mutex::new(library));
        let lacks = Arc::new(Lacks::default());
        let scrobbling = Arc::new(crate::scrobbling::Scrobbling::default());
        // Offline holds from one run to the next.
        let offline = Arc::new(AtomicBool::new(
            library.lock().unwrap().setting(OFFLINE_SETTING)?.as_deref() == Some("1"),
        ));
        let player = match &options.audio {
            None => Err("this core runs without a player".to_string()),
            Some(output) => match options.mpv.clone().or_else(find_library) {
                None => Err("the audio engine (libmpv) was not found".to_string()),
                Some(engine) => {
                    let settings = Settings {
                        resolver: Some(resolver(
                            Arc::clone(&addons),
                            Arc::clone(&library),
                            Arc::clone(&lacks),
                            Arc::clone(&relays),
                            Arc::clone(&offline),
                            port,
                        )),
                        // A play that counts is kept, and waits for the listener's scrobbling services.
                        on_play: Some({
                            let keep = keep_plays(Arc::clone(&library));
                            let send = crate::scrobbling::on_play(
                                Arc::clone(&library),
                                Arc::clone(&scrobbling),
                            );
                            Arc::new(move |item: &QueueItem, started_at_ms: i64| {
                                keep(item, started_at_ms);
                                send(item, started_at_ms);
                            })
                        }),
                        on_start: Some(crate::scrobbling::on_start(
                            Arc::clone(&library),
                            Arc::clone(&scrobbling),
                        )),
                        // A copy the engine cannot play is let go; the song comes from an add-on again.
                        on_failed: Some({
                            let (relays, library) = (Arc::clone(&relays), Arc::clone(&library));
                            Arc::new(move |item: &QueueItem| {
                                if let Some(id) = item.id {
                                    relays.failed(&library, id);
                                }
                            })
                        }),
                        counts_between_seconds: options
                            .play_counts_between_seconds
                            .unwrap_or(Settings::default().counts_between_seconds),
                        ..Settings::default()
                    };
                    Player::start_with(&engine, output, settings).map_err(|e| e.to_string())
                }
            },
        };
        // What the listener set last time holds again: the volume, and whether Stash takes the sound card for itself.
        if let Ok(playing) = &player {
            let kept = |key: &str| library.lock().unwrap().setting(key).ok().flatten();
            if let Some(volume) = kept("volume").and_then(|v| v.parse::<f64>().ok()) {
                let _ = playing.set_volume(volume);
            }
            if kept("exclusive").as_deref() == Some("1") {
                let _ = playing.set_exclusive(true);
            }
            if let Some(mode) = kept("replaygain") {
                let _ = playing.set_replaygain(&mode);
            }
            // The output chosen last time, when it is connected; else the system's own choice, and the choice
            // holds for when it is back.
            if let Some(chosen) = kept("device").and_then(|text| chosen_device(&text))
                && playing.devices().iter().any(|d| d.name == chosen.0)
            {
                let _ = playing.set_device(&chosen.0);
            }
            // And the queue as it was left: nothing plays until the listener says so.
            crate::kept::restore(&library, playing);
        }
        Ok(Self {
            server,
            state: State {
                library,
                options,
                port,
                player,
                addons,
                scanning: Arc::new(Mutex::new(Scanning::default())),
                lacks,
                saving: crate::saving::Saving::default(),
                relays,
                scrobbling,
                offline,
                window: None,
                key: None,
                shortcuts: crate::shortcuts::Shortcuts::default(),
                edits: AtomicU64::new(0),
            },
        })
    }

    pub fn port(&self) -> u16 {
        self.state.port
    }

    /// Lets the page ask `shell` for the full window or the mini player.
    pub fn with_window(mut self, shell: WindowShell) -> Self {
        self.state.window = Some(shell);
        self
    }

    /// Answers the desktop window's origins only with `key`, which the shell gives the window for this run. A browser
    /// names a page at tauri.localhost the way it names the window, and any program on this computer may serve one
    /// there; without the key such a page is refused, as any other site's.
    pub fn with_key(mut self, key: String) -> Self {
        self.state.key = Some(key);
        self
    }

    /// Lets `shell` hold the listener's shortcuts that work in any app.
    pub fn with_shortcuts(mut self, shell: KeysShell) -> Self {
        self.state.shortcuts.shell = Some(shell);
        self
    }

    /// Answers requests until a measurement arrives (when one is being collected), else for good.
    pub fn run(self) -> Result<()> {
        let Core { server, state } = self;
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            // The queue is written down as it changes, so the next run opens on it (the program may be closed, or
            // lose power, at any moment: there is no "on the way out" to rely on).
            if let Ok(player) = &state.player {
                let (library, stop, addons, offline) =
                    (&state.library, &stop, &state.addons, &state.offline);
                scope.spawn(move || {
                    let mut keeper = crate::kept::Keeper::default();
                    let mut autoplay = crate::autoplay::Autoplay::default();
                    let mut looks = 0u32;
                    while !stop.load(Ordering::Relaxed) {
                        keeper.look(library, player);
                        // The last song of the queue started: songs like the last ones are added after it.
                        let can_ask = !offline.load(Ordering::Relaxed)
                            && addons.read().unwrap().iter().any(|a| {
                                a.usable()
                                    .is_some_and(|addon| addon.offers_resource("stream"))
                            });
                        autoplay.look(library, player, can_ask);
                        // Every few seconds: the output chosen and unplugged is taken back once it is connected.
                        if looks.is_multiple_of(5) {
                            follow_chosen_device(library, player);
                        }
                        looks = looks.wrapping_add(1);
                        std::thread::sleep(Duration::from_millis(1000));
                    }
                });
            }
            // Songs to save as files, one at a time.
            {
                let (state, stop) = (&state, &stop);
                scope.spawn(move || crate::saving::run(state, stop));
            }
            // Plays on their way to the listener's scrobbling services.
            {
                let (state, stop) = (&state, &stop);
                scope.spawn(move || crate::scrobbling::run(state, stop));
            }
            // The shortcuts as the listener left them (the shell holds them once its window is up).
            if state.shortcuts.shell.is_some() {
                let state = &state;
                scope.spawn(move || {
                    if let Err(e) = crate::shortcuts::apply(state) {
                        eprintln!("the shortcuts were not held: {e:#}");
                    }
                });
            }
            for _ in 0..WORKERS {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        match server.recv_timeout(Duration::from_millis(250)) {
                            Ok(Some(request)) => {
                                if answer(&state, request) {
                                    stop.store(true, Ordering::Relaxed);
                                }
                            }
                            Ok(None) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
        });
        Ok(())
    }
}

/// Binds and runs in one go, saying where it listens.
pub fn serve(options: Options) -> Result<()> {
    let core = Core::bind(options)?;
    println!("listening on http://127.0.0.1:{}", core.port());
    core.run()
}

/// Answers one request. True when the server is done (a measurement arrived).
fn answer(state: &State, mut request: Request) -> bool {
    let port = state.port;
    let host_ok = find(&request, "Host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    let origin = find(&request, "Origin").map(str::to_string);
    // A browser says where a request comes from (Sec-Fetch-Site). From another site it must come from the window: by
    // its Origin, or, for a picture (an image names no Origin), by the page it was asked from (Referer). A web page's
    // image or no-cors request names neither, and is not answered: it could otherwise spend an add-on's allowance.
    // Programs that are not browsers (the engine, a script) say nothing of where they come from.
    let elsewhere =
        find(&request, "Sec-Fetch-Site").is_some_and(|s| !matches!(s, "same-origin" | "none"));
    let referer = || {
        find(&request, "Referer")
            .and_then(|r| Url::parse(r).ok())
            .map(|r| origin_of(&r))
    };
    let origin_ok = match origin.as_deref() {
        Some(o) => allowed_origin(o, port),
        None if elsewhere => referer().is_some_and(|r| allowed_origin(&r, port)),
        None => true,
    };
    // From the window's origin, the window's key too.
    let from_window = match origin.as_deref() {
        Some(o) => window_origin(o),
        None => elsewhere && referer().is_some_and(|r| window_origin(&r)),
    };
    let keyed = match &state.key {
        Some(key) if from_window => key_of(request.url()).as_deref() == Some(key.as_str()),
        _ => true,
    };

    // The engine reading a stream the core relays: answered on a thread of its own, as long as the song plays.
    if host_ok && request.url().starts_with("/relay/") {
        state.relays.answer(Arc::clone(&state.library), request);
        return false;
    }
    let mut done = false;
    let mut response = if !host_ok {
        refuse(403, "this server answers only as 127.0.0.1 or localhost")
    } else if !origin_ok || !keyed {
        refuse(403, "this page may not use the Stash core")
    } else if *request.method() == Method::Options {
        reply(204, "text/plain", Vec::new())
    } else {
        match route(state, &mut request) {
            Ok((response, finished)) => {
                done = finished;
                response
            }
            Err(e) => match e.downcast_ref::<stash_core::Error>() {
                Some(stash_core::Error::Refused(why)) => refuse(400, why),
                _ => refuse(500, &format!("{e:#}")),
            },
        }
    };
    // The desktop window is another origin than this server: it is told it may read the answer.
    if let Some(origin) = origin.filter(|o| allowed_origin(o, port)) {
        response = response
            .with_header(header("Access-Control-Allow-Origin", &origin))
            .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
            .with_header(header("Vary", "Origin"));
    }
    let _ = request.respond(response);
    done
}

/// The body of a request as JSON (an empty body is `{}`).
pub(crate) fn body_json(request: &mut Request) -> Result<Value> {
    let mut body = String::new();
    request
        .as_reader()
        .take(BODY_CAP)
        .read_to_string(&mut body)?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).context("the request's body is not JSON")
}

/// The player's state as the window reads it. Without a player: `available: false` and the reason.
fn player_reply(state: &State) -> Reply {
    match &state.player {
        Ok(player) => {
            let mut answer = json!(player.state());
            answer["available"] = json!(true);
            answer["edits"] = json!(state.edits.load(Ordering::Relaxed));
            reply_json(200, answer)
        }
        Err(why) => reply_json(
            200,
            json!({ "available": false, "reason": why, "status": "idle" }),
        ),
    }
}

/// The queue for tracks an add-on found: the window hands back what the search gave it.
fn addon_queue(addon: &str, tracks: Option<&Value>) -> Vec<QueueItem> {
    tracks
        .and_then(Value::as_array)
        .map(|tracks| {
            tracks
                .iter()
                .take(ADDON_TRACKS_CAP)
                .filter_map(|track| addon_item(addon, track))
                .collect()
        })
        .unwrap_or_default()
}

/// A cover's address handed in from outside, when it is one a window may load: https, of a sane length.
fn cover(value: Option<&Value>) -> Option<String> {
    let address = value?.as_str()?;
    let plain = address.len() <= 2_048
        && !address
            .chars()
            .any(|c| c.is_control() || c == ' ' || c == '\\');
    (plain && Url::parse(address).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some()))
        .then(|| address.to_string())
}

/// One track an add-on found, as a queue item. `None` without a usable id.
fn addon_item(addon: &str, track: &Value) -> Option<QueueItem> {
    let text = |name: &str| track.get(name).and_then(Value::as_str).map(capped);
    let id = track.get("id").and_then(Value::as_str)?;
    if addon.is_empty() || id.is_empty() || id.len() > 512 {
        return None;
    }
    Some(QueueItem {
        id: None,
        title: text("title").unwrap_or_default(),
        artist: text("artist").unwrap_or_default(),
        album: text("album"),
        addon: Some(AddonTrack {
            addon: addon.to_string(),
            track: id.to_string(),
        }),
        locator: String::new(),
        art: cover(track.get("art")),
        duration_ms: track
            .get("duration_ms")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0),
        qid: 0,
    })
}

/// A list of tracks handed in as `[{id} | {addon, track}]`, as queue items, with where place `start` of the list
/// ended up (a track the library no longer has, or whose file is gone, is left out).
fn listed_items(
    library: &Library,
    list: &[Value],
    start: usize,
) -> Result<(Vec<QueueItem>, usize)> {
    let mut items = Vec::with_capacity(list.len().min(QUEUE_CAP));
    let mut begins = 0;
    for (place, entry) in list.iter().take(QUEUE_CAP).enumerate() {
        if place == start {
            begins = items.len();
        }
        let item = match entry.get("id").and_then(Value::as_i64) {
            Some(id) => library.queue_items(&[id])?.pop(),
            None => entry
                .get("addon")
                .and_then(Value::as_str)
                .zip(entry.get("track"))
                .and_then(|(addon, track)| addon_item(addon, track)),
        };
        items.extend(item);
    }
    Ok((items, begins))
}

/// Offline, a queue keeps only the songs that play with no connection, and begins at the song at `start` or, when that
/// one does not play here, the next that does. `None`: none from `start` on does.
fn offline_queue(
    state: &State,
    items: Vec<QueueItem>,
    start: usize,
) -> Result<Option<(Vec<QueueItem>, usize)>> {
    if !offline(state) {
        return Ok(Some((items, start)));
    }
    let ids: Vec<i64> = items.iter().filter_map(|i| i.id).collect();
    let here = state.library.lock().unwrap().here_of(&ids)?;
    let plays = |item: &QueueItem| plays_here(item, &here);
    let Some(first) = items
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, item)| plays(item))
        .map(|(place, _)| place)
    else {
        return Ok(None);
    };
    let mut kept = Vec::with_capacity(items.len());
    let mut begins = 0;
    for (place, item) in items.into_iter().enumerate() {
        if place == first {
            begins = kept.len();
        }
        if plays(&item) {
            kept.push(item);
        }
    }
    Ok(Some((kept, begins)))
}

/// Whether a queued song plays with no connection, given which of the library's songs do (`Library::here_of`): a
/// song of the library that is a file here or has a copy kept. An add-on's own track never does.
fn plays_here(item: &QueueItem, here: &std::collections::HashSet<i64>) -> bool {
    item.addon.is_none() && item.id.is_some_and(|id| here.contains(&id))
}

/// /api/offline: whether Stash is offline; POST {offline} says so, and it holds next time.
fn offline_route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if path != "/api/offline" {
        return Ok(None);
    }
    if *request.method() == Method::Post {
        let Some(on) = body_json(request)?.get("offline").and_then(Value::as_bool) else {
            return Ok(Some(refuse(400, "offline is true or false")));
        };
        state
            .library
            .lock()
            .unwrap()
            .set_setting(OFFLINE_SETTING, if on { "1" } else { "0" })?;
        let was = state.offline.swap(on, Ordering::Relaxed);
        // Offline: what the engine was handed after the song playing is asked for again, so a song streamed from an
        // add-on that was about to start does not. The song playing plays to its end.
        if on && let Ok(player) = &state.player {
            player.ask_again_ahead();
        }
        // Back online: what waited goes.
        if !on {
            state.scrobbling.ring(None);
            state.saving.ring();
        }
        // Every window reads its lists again: which songs are on this computer may have changed since it last did
        // (songs saved in the meantime), and offline that decides what plays.
        if was != on {
            state.edited();
        }
    } else if *request.method() != Method::Get {
        return Ok(None);
    }
    Ok(Some(reply_json(200, json!({ "offline": offline(state) }))))
}

/// One of the player's routes. `None`: the path is not one of them.
fn player_route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let player = &state.player;
    if *request.method() == Method::Get && path == "/api/player" {
        return Ok(Some(player_reply(state)));
    }
    // The sound outputs, the one in use and the one chosen.
    if *request.method() == Method::Get && path == "/api/player/devices" {
        return Ok(Some(match player {
            Ok(playing) => reply_json(200, devices_reply(state, playing)),
            Err(_) => player_reply(state),
        }));
    }
    // The queue from `offset` on, each track with its place.
    if *request.method() == Method::Get && path == "/api/player/queue" {
        let url =
            Url::parse(&format!("http://local{}", request.url())).context("the request's path")?;
        let number = |name: &str, default: usize| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(default)
        };
        let (offset, limit) = (
            number("offset", 0),
            number("limit", PAGE_CAP).clamp(1, PAGE_CAP),
        );
        let rows: Vec<Value> = match player {
            Ok(playing) => {
                let items = playing.queue_at(offset, limit);
                let ids: Vec<i64> = items.iter().filter_map(|item| item.id).collect();
                let here = state.library.lock().unwrap().here_of(&ids)?;
                items
                    .iter()
                    .enumerate()
                    .map(|(n, item)| {
                        let mut row = json!(item);
                        row["place"] = json!(offset + n);
                        // It plays with no connection: offline, the others are passed over.
                        row["here"] = json!(plays_here(item, &here));
                        row
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        return Ok(Some(reply_json(200, json!({ "rows": rows }))));
    }
    if *request.method() != Method::Post || !path.starts_with("/api/player/") {
        return Ok(None);
    }
    let Ok(playing) = player else {
        return Ok(Some(player_reply(state)));
    };
    let body = body_json(request)?;
    match path {
        // What to play: `{"ids": [...]}` (library tracks, in that order), `{"offset": n}` (the library's rows from
        // there on; with `"playlist": id` that playlist's, with `"liked": true` the liked songs'), or
        // `{"addon": id, "tracks": [...]}` (tracks an add-on found, in that order).
        // What to play, one of: `{"items": [{id} | {addon, track}], "start": n}` (these tracks, from place n);
        // `{"offset": n}` (the library from that row on), with `"liked": true`, `"playlist": id`, `"album": id` or
        // `"artist": id` that list from row n on; `{"ids": [...]}`; `{"addon": id, "tracks": [...]}`.
        // `"from"` says what it is in the listener's words, for "Playing from ...".
        "/api/player/play" => {
            let from = body
                .get("from")
                .and_then(Value::as_str)
                .map(capped)
                .filter(|f| !f.is_empty());
            let (items, start) = if let Some(list) = body.get("items").and_then(Value::as_array) {
                let start = body.get("start").and_then(Value::as_u64).unwrap_or(0) as usize;
                listed_items(&state.library.lock().unwrap(), list, start)?
            } else if let Some(addon) = body.get("addon").and_then(Value::as_str) {
                (addon_queue(addon, body.get("tracks")), 0)
            } else {
                let library = state.library.lock().unwrap();
                let offset = body.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let count = body.get("count").and_then(Value::as_u64).unwrap_or(1_000) as usize;
                let count = count.clamp(1, QUEUE_CAP);
                let list = |name: &str| body.get(name).and_then(Value::as_i64);
                let ids: Vec<i64> = if let Some(ids) = body.get("ids").and_then(Value::as_array) {
                    ids.iter()
                        .filter_map(Value::as_i64)
                        .take(QUEUE_CAP)
                        .collect()
                } else if let Some(playlist) = list("playlist") {
                    library.playlist_track_ids(playlist, offset, count)?
                } else if let Some(album) = list("album") {
                    library.album_track_ids(album, offset, count)?
                } else if let Some(artist) = list("artist") {
                    library.artist_track_ids(artist, offset, count)?
                } else if body.get("liked").and_then(Value::as_bool) == Some(true) {
                    library.liked_ids_at(offset, count)?
                } else {
                    let here = body.get("here").and_then(Value::as_bool) == Some(true);
                    let at = |offset: usize, count: usize| {
                        if here {
                            library.here_track_ids_at(offset, count)
                        } else {
                            library.track_ids_at(offset, count)
                        }
                    };
                    let mut ids = at(offset, count)?;
                    // The song clicked (`id`), when the list moved since the window read it (a song saved or added in
                    // between): it is looked for near where it was, and the list plays from it.
                    if let Some(want) = body.get("id").and_then(Value::as_i64)
                        && ids.first() != Some(&want)
                    {
                        let from = offset.saturating_sub(50);
                        if let Some(place) = at(from, 101)?.iter().position(|i| *i == want) {
                            ids = at(from + place, count)?;
                        }
                    }
                    ids
                };
                (library.queue_items(&ids)?, 0)
            };
            // Songs the listener chose not to play are left out (one song asked for by itself still plays).
            let Some((items, start)) =
                crate::blocking::leave_out(&state.library.lock().unwrap(), items, start)?
            else {
                return Ok(Some(refuse(409, crate::blocking::ALL_BLOCKED)));
            };
            let Some((items, start)) = offline_queue(state, items, start)? else {
                return Ok(Some(refuse(409, NOT_HERE_OFFLINE)));
            };
            playing.play_from(items, start, from)?;
        }
        // Adds tracks (as in `items` above) right after the playing one (`"next": true`) or at the end.
        "/api/player/enqueue" => {
            let list = body
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let (items, _) = listed_items(&state.library.lock().unwrap(), &list, 0)?;
            let Some((items, _)) =
                crate::blocking::leave_out(&state.library.lock().unwrap(), items, 0)?
            else {
                return Ok(Some(refuse(409, crate::blocking::ALL_BLOCKED)));
            };
            let Some((items, _)) = offline_queue(state, items, 0)? else {
                return Ok(Some(refuse(409, NOT_HERE_OFFLINE)));
            };
            playing.enqueue(
                items,
                body.get("next").and_then(Value::as_bool).unwrap_or(false),
            );
        }
        "/api/player/remove" => {
            let place = body
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            playing.remove_at(usize::try_from(place).unwrap_or(usize::MAX));
        }
        "/api/player/move" => {
            let place = |name: &str| {
                body.get(name)
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(usize::MAX)
            };
            playing.move_item(place("from"), place("to"));
        }
        "/api/player/shuffle" => {
            playing.set_shuffle(body.get("on").and_then(Value::as_bool).unwrap_or(false))
        }
        "/api/player/repeat" => playing.set_repeat(Repeat::from_word(
            body.get("mode").and_then(Value::as_str).unwrap_or("off"),
        )),
        "/api/player/pause" => {
            playing.set_paused(body.get("paused").and_then(Value::as_bool).unwrap_or(true))?
        }
        "/api/player/next" => playing.next()?,
        "/api/player/previous" => playing.previous()?,
        "/api/player/seek" => {
            playing.seek_to(body.get("seconds").and_then(Value::as_f64).unwrap_or(0.0))?
        }
        "/api/player/stop" => playing.stop()?,
        "/api/player/jump" => {
            let place = body
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX);
            playing.jump(usize::try_from(place).unwrap_or(usize::MAX));
        }
        "/api/player/volume" => {
            let percent = body
                .get("percent")
                .and_then(Value::as_f64)
                .unwrap_or(100.0)
                .clamp(0.0, 100.0);
            playing.set_volume(percent)?;
            state
                .library
                .lock()
                .unwrap()
                .set_setting("volume", &format!("{percent:.0}"))?;
        }
        // Sends the sound to another output (`{"name": "auto"}` for the system's own choice); it holds until changed.
        // Answers the outputs, as GET /api/player/devices does.
        "/api/player/device" => {
            let name = body.get("name").and_then(Value::as_str).unwrap_or("auto");
            let Some(device) = playing.devices().into_iter().find(|d| d.name == name) else {
                return Ok(Some(refuse(400, "that sound output is not connected")));
            };
            playing.set_device(&device.name)?;
            state
                .library
                .lock()
                .unwrap()
                .set_setting("device", &json!(device).to_string())?;
            return Ok(Some(reply_json(200, devices_reply(state, playing))));
        }
        // Takes the sound card for Stash alone, or gives it back; it holds until it is changed again.
        "/api/player/exclusive" => {
            let exclusive = body
                .get("exclusive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            playing.set_exclusive(exclusive)?;
            state
                .library
                .lock()
                .unwrap()
                .set_setting("exclusive", if exclusive { "1" } else { "0" })?;
        }
        // Evens out loudness between songs: {"mode": "off" | "track" | "album"}; it holds until changed.
        "/api/player/replaygain" => {
            let mode = body.get("mode").and_then(Value::as_str).unwrap_or("off");
            if !["off", "track", "album"].contains(&mode) {
                return Ok(Some(refuse(400, "mode is off, track or album")));
            }
            playing.set_replaygain(mode)?;
            state
                .library
                .lock()
                .unwrap()
                .set_setting("replaygain", mode)?;
        }
        _ => return Ok(None),
    }
    Ok(Some(player_reply(state)))
}

/// Where lyrics are looked up when no add-on has them.
const LRCLIB: &str = "https://lrclib.net";
const LRCLIB_SETTING: &str = "lyrics.lrclib";
/// The setting that keeps Stash offline ("1").
const OFFLINE_SETTING: &str = "offline";
/// Songs asked for while offline, none of which plays with no connection.
const NOT_HERE_OFFLINE: &str = "Stash is offline, and none of that is on this computer";

/// Stash is offline: nothing is asked of the network for music (no add-on, no LRCLIB, no scrobbling service).
pub(crate) fn offline(state: &State) -> bool {
    state.offline.load(Ordering::Relaxed)
}
/// How long lyrics found on the network are kept, and how long "none there" is believed.
const LYRICS_KEPT_SECONDS: i64 = 30 * 24 * 3600;
const NO_LYRICS_KEPT_SECONDS: i64 = 24 * 3600;

/// A song's lyrics: `{"lyrics": …|null, "via": "…"}`. The song is a library row (`id`), or an add-on's track
/// (`addon`, `track`) or any song (`title`, `artist`, `album`, `duration_ms`) by what it is.
fn lyrics_for(state: &State, param: &dyn Fn(&str) -> Option<String>) -> Result<Value> {
    let library = || state.library.lock().unwrap();
    let row = param("id").and_then(|id| id.parse::<i64>().ok());
    let (wanted, also, mut refs, file) = match row {
        Some(id) => {
            let Some(described) = library().describe(id)? else {
                return Ok(json!({ "lyrics": null, "via": null }));
            };
            (
                described.wanted,
                described.also,
                described.refs,
                library().file_of(id)?,
            )
        }
        None => {
            let text = |name: &str| param(name).map(|v| capped(&v)).filter(|v| !v.is_empty());
            let wanted = stash_core::addons::Wanted {
                title: text("title").unwrap_or_default(),
                artist: text("artist").unwrap_or_default(),
                album: text("album"),
                duration_ms: param("duration_ms").and_then(|v| v.parse::<u64>().ok()),
                isrc: param("isrc").and_then(|v| stash_core::identity::isrc(&v)),
                explicit: None,
            };
            (wanted, Vec::new(), Vec::new(), None)
        }
    };
    if let (Some(addon), Some(track)) = (param("addon"), param("track")) {
        refs.insert(0, (addon, track));
    }

    // A file's own lyrics: read each time (the listener may have just put a .lrc beside it).
    if let Some(file) = &file
        && let Some((found, via)) = stash_core::lyrics::of_file(Path::new(file))
    {
        return Ok(json!({ "lyrics": found, "via": via }));
    }
    if wanted.title.is_empty() || wanted.artist.is_empty() {
        return Ok(json!({ "lyrics": null, "via": null }));
    }
    let key = match &wanted.isrc {
        Some(isrc) => format!("isrc:{isrc}"),
        None => stash_core::identity::words_key(&wanted.title, &wanted.artist),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Some(stash_core::lists::KeptLyrics {
        lyrics: found,
        via,
        at,
    }) = library().cached_lyrics(&key)?
    {
        let keep = if found.is_some() {
            LYRICS_KEPT_SECONDS
        } else {
            NO_LYRICS_KEPT_SECONDS
        };
        if now - at < keep || (offline(state) && found.is_some()) {
            return Ok(json!({ "lyrics": found, "via": via }));
        }
    }
    if offline(state) {
        return Ok(json!({ "lyrics": null, "via": null }));
    }

    // The listener's add-ons that offer lyrics, in the listener's order; the one that serves the song is asked by
    // its own id for it.
    let mut unreachable = 0;
    let addons: Vec<Arc<Loaded>> = state.addons.read().unwrap().clone();
    let mut ordered: Vec<&Arc<Loaded>> = addons.iter().collect();
    ordered.sort_by_key(|loaded| !refs.iter().any(|(id, _)| *id == loaded.stored.id));
    for loaded in ordered {
        let Some(addon) = loaded.usable().filter(|a| a.offers_resource("lyrics")) else {
            continue;
        };
        let own = refs
            .iter()
            .find(|(id, _)| *id == loaded.stored.id)
            .map(|(_, track)| track.as_str());
        match addon.lyrics(own, &wanted) {
            Ok(Some(found)) => {
                let name = loaded.listed()["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                library().keep_lyrics(&key, Some(&found), Some(&name), now)?;
                return Ok(json!({ "lyrics": found, "via": name }));
            }
            Ok(None) => {}
            Err(_) => unreachable += 1,
        }
    }
    if library().setting(LRCLIB_SETTING)?.as_deref() != Some("0") {
        // As the song is read, then each other way it may be read (a row from YouTube).
        for reading in std::iter::once(&wanted).chain(&also) {
            match stash_core::lyrics::from_lrclib(
                LRCLIB,
                &reading.title,
                &reading.artist,
                reading.album.as_deref(),
                reading.duration_ms,
            ) {
                Ok(Some(found)) => {
                    library().keep_lyrics(&key, Some(&found), Some("LRCLIB"), now)?;
                    return Ok(json!({ "lyrics": found, "via": "LRCLIB" }));
                }
                Ok(None) => {}
                Err(_) => {
                    unreachable += 1;
                    break;
                }
            }
        }
    }
    // "None there" is kept only when every source answered: one that could not be asked is asked again next time.
    if unreachable == 0 {
        library().keep_lyrics(&key, None, None, now)?;
    }
    Ok(json!({ "lyrics": null, "via": null }))
}

/// The output kept in the settings: its name and what it was called.
fn chosen_device(text: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let name = value["name"].as_str()?.to_string();
    let description = value["description"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Some((name, description))
}

/// Sound goes to the output the listener chose whenever it is connected: one that was unplugged (the system's own
/// choice played meanwhile) is taken back when it is there again. A choice of "auto" is left alone.
fn follow_chosen_device(library: &Mutex<Library>, player: &Player) {
    let chosen = library
        .lock()
        .unwrap()
        .setting("device")
        .ok()
        .flatten()
        .and_then(|text| chosen_device(&text));
    let Some((name, _)) = chosen else { return };
    if name == "auto" || player.device() != "auto" {
        return;
    }
    if player.devices().iter().any(|d| d.name == name) {
        let _ = player.set_device(&name);
    }
}

/// The outputs, the one in use, and the one chosen when it is not connected now (so the window can say so).
fn devices_reply(state: &State, playing: &Player) -> Value {
    let devices = playing.devices();
    let chosen = state
        .library
        .lock()
        .unwrap()
        .setting("device")
        .ok()
        .flatten()
        .and_then(|text| chosen_device(&text));
    let missing = chosen
        .filter(|(name, _)| name != "auto" && !devices.iter().any(|d| d.name == *name))
        .map(|(name, description)| json!({ "name": name, "description": description }));
    json!({ "devices": devices, "device": playing.device(), "missing": missing })
}

/// Asks `asked` for tracks matching `query`, all at once; one group of results (or one reason) per add-on, in the
/// order given.
pub(crate) fn search_addons(asked: &[Arc<Loaded>], query: &str, limit: usize) -> Vec<Value> {
    let one = |loaded: &Loaded| {
        let who = json!({ "id": loaded.stored.id, "name": loaded.listed()["name"] });
        let Some(addon) = loaded.usable() else {
            return json!({ "addon": who, "tracks": [], "error": null });
        };
        match addon.search(query, limit as u32) {
            Ok(tracks) => {
                let tracks: Vec<Value> =
                    tracks.iter().filter_map(found_track).take(limit).collect();
                json!({ "addon": who, "tracks": tracks, "error": null })
            }
            Err(e) => json!({ "addon": who, "tracks": [], "error": addon_error(&e) }),
        }
    };
    std::thread::scope(|scope| {
        let asking: Vec<_> = asked
            .iter()
            .map(|loaded| scope.spawn(|| one(loaded)))
            .collect();
        asking
            .into_iter()
            .zip(asked)
            .map(|(asking, loaded)| {
                asking.join().unwrap_or_else(|_| {
                    json!({
                        "addon": { "id": loaded.stored.id, "name": loaded.stored.id },
                        "tracks": [],
                        "error": { "code": "unavailable", "message": "the search failed", "retry_after": null },
                    })
                })
            })
            .collect()
    })
}

/// One of the add-on routes. `None`: the path is not one of them.
fn addons_route(
    state: &State,
    request: &mut Request,
    path: &str,
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Reply>> {
    if !path.starts_with("/api/addons") {
        return Ok(None);
    }
    if *request.method() == Method::Get {
        return Ok(match path {
            "/api/addons" => Some(state.addons_reply()),
            "/api/addons/use" => {
                let since = param("since")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or_else(|| now_ms() - 24 * 3600 * 1000);
                let used = state.library.lock().unwrap().addon_use_since(since)?;
                let ids: Vec<String> = state
                    .addons
                    .read()
                    .unwrap()
                    .iter()
                    .map(|a| a.stored.id.clone())
                    .collect();
                let mut out = serde_json::Map::new();
                for id in ids {
                    let given = used.get(&id).copied().unwrap_or_default();
                    let (found_at, trouble) = state.lacks.health(&id);
                    out.insert(
                        id,
                        json!({
                            "streams": given.streams,
                            "downloads": given.downloads,
                            "found_at_ms": found_at,
                            "trouble": trouble.map(|(at, message)| json!({ "at_ms": at, "message": message })),
                        }),
                    );
                }
                Some(reply_json(200, json!({ "addons": out })))
            }
            // `q`, and optionally `addon` (ask only that one) and `limit`.
            "/api/addons/search" => {
                if offline(state) {
                    return Ok(Some(refuse(
                        409,
                        "Stash is offline: only your library is searched",
                    )));
                }
                let query = param("q").unwrap_or_default();
                let only = param("addon");
                let limit = param("limit")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(25)
                    .clamp(1, ADDON_TRACKS_CAP);
                let asked: Vec<Arc<Loaded>> = state
                    .addons
                    .read()
                    .unwrap()
                    .iter()
                    .filter(|a| {
                        a.usable()
                            .is_some_and(|addon| addon.offers_resource("search"))
                    })
                    .filter(|a| only.as_deref().is_none_or(|id| id == a.stored.id))
                    .cloned()
                    .collect();
                let groups = if query.trim().is_empty() {
                    Vec::new()
                } else {
                    search_addons(&asked, query.trim(), limit)
                };
                Some(reply_json(200, json!({ "groups": groups })))
            }
            _ => None,
        });
    }
    if *request.method() != Method::Post {
        return Ok(None);
    }
    let body = body_json(request)?;
    let id = body.get("id").and_then(Value::as_str).unwrap_or_default();
    match path {
        // Adds the add-on at `link` (the address of its manifest); one that is there already is read again.
        "/api/addons" => {
            if offline(state) {
                return Ok(Some(refuse(
                    409,
                    "Stash is offline: an add-on is read from its link when it is added",
                )));
            }
            let link = body
                .get("link")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            // The add-on is asked for its manifest here, with nothing else waiting on it.
            let addon = match Addon::from_link(link) {
                Ok(addon) => addon,
                Err(e) => {
                    let status = match e.code {
                        Code::BadRequest | Code::InvalidResponse | Code::Refused => 400,
                        _ => 502,
                    };
                    let mut error = addon_error(&e);
                    if e.code == Code::NotFound {
                        error["message"] = json!("there is no add-on at that address");
                    }
                    return Ok(Some(reply_json(status, json!({ "error": error }))));
                }
            };
            state.library.lock().unwrap().save_addon(
                &addon.manifest.id,
                addon.link(),
                &addon.manifest_json(),
            )?;
            state.lacks.forget_addon(&addon.manifest.id);
            // Songs waiting to be saved may be found now.
            state.saving.release();
            state.saving.ring();
        }
        "/api/addons/remove" => {
            state.library.lock().unwrap().remove_addon(id)?;
        }
        "/api/addons/enabled" => {
            let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            state
                .library
                .lock()
                .unwrap()
                .set_addon_enabled(id, enabled)?;
            if enabled {
                state.lacks.forget_addon(id);
                state.saving.release();
                state.saving.ring();
            }
        }
        "/api/addons/order" => {
            let ids: Vec<String> = body
                .get("ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            state.library.lock().unwrap().order_addons(&ids)?;
        }
        _ => return Ok(None),
    }
    state.reload_addons()?;
    Ok(Some(state.addons_reply()))
}

/// The listener's music folders and the scan of them. `None`: the path is not one of these.
fn scan_route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let told = |state: &State| -> Result<Reply> {
        let scanning = state.scanning.lock().unwrap().clone();
        let folders = state.library.lock().unwrap().folders()?;
        Ok(reply_json(
            200,
            json!({ "scan": scanning, "folders": folders }),
        ))
    };
    match (request.method(), path) {
        (Method::Get, "/api/scan") => Ok(Some(told(state)?)),
        // Asks the listener for a folder with the system's own dialog, on the computer the core runs on.
        (Method::Post, "/api/scan/pick") => {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a music folder")
                .pick_folder()
                .map(|folder| folder.to_string_lossy().into_owned());
            Ok(Some(reply_json(200, json!({ "folder": picked }))))
        }
        // Scans `folder` and remembers it; without one, scans the folders that are remembered.
        (Method::Post, "/api/scan") => {
            let body = body_json(request)?;
            let folders = match body.get("folder").and_then(Value::as_str).map(str::trim) {
                Some(folder) if !folder.is_empty() => {
                    if !Path::new(folder).is_dir() {
                        return Ok(Some(refuse(400, "That folder was not found")));
                    }
                    vec![folder.to_string()]
                }
                _ => state.library.lock().unwrap().folders()?,
            };
            if folders.is_empty() {
                return Ok(Some(refuse(400, "There is no music folder to scan yet")));
            }
            if !state.scan(folders) {
                return Ok(Some(refuse(409, "A scan is running already")));
            }
            Ok(Some(told(state)?))
        }
        _ => Ok(None),
    }
}

/// The listener's own doing: liking, playlists, and the library written out. `None`: the path is not one of these.
fn lists_route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    // The library as the file the Android app and the web player read.
    if *request.method() == Method::Get && path == "/api/export" {
        let file = state.library.lock().unwrap().export(
            now_ms(),
            concat!("Stash desktop ", env!("CARGO_PKG_VERSION")),
        )?;
        let day = file["exportedAt"]
            .as_str()
            .unwrap_or_default()
            .get(..10)
            .unwrap_or("backup")
            .to_string();
        let named = format!("attachment; filename=\"stash-library-{day}.json\"");
        return Ok(Some(
            reply(200, "application/json", file.to_string().into_bytes())
                .with_header(header("Content-Disposition", &named)),
        ));
    }
    if *request.method() != Method::Post {
        return Ok(None);
    }
    let playlist_verb = path
        .strip_prefix("/api/playlists/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(id, verb)| Some((id.parse::<i64>().ok()?, verb)));
    if !matches!(
        path,
        "/api/tracks/like" | "/api/tracks/keep" | "/api/playlists" | "/api/playlists/preview"
    ) && playlist_verb.is_none()
    {
        return Ok(None);
    }
    let body = body_json(request)?;
    let rules_in =
        |body: &Value| stash_core::smart::Rules::read(body.get("rules").unwrap_or(&Value::Null));
    // What rules would pick, for the editor: how many, and the first few. Nothing changes.
    if path == "/api/playlists/preview" {
        let rules = match rules_in(&body) {
            Ok(rules) => rules,
            Err(why) => return Ok(Some(refuse(400, &why))),
        };
        let library = state.library.lock().unwrap();
        let now = now_ms();
        let count = library.smart_count(&rules, now)?;
        let rows = library.smart_tracks_at(&rules, 0, 5, now)?;
        return Ok(Some(reply_json(
            200,
            json!({ "count": count, "rows": rows }),
        )));
    }
    let number = |name: &str| body.get(name).and_then(Value::as_u64).map(|n| n as usize);
    let mut library = state.library.lock().unwrap();
    let now = now_ms();
    let mut answer = json!({});
    match (path, playlist_verb) {
        ("/api/tracks/like", _) => {
            let id = body.get("id").and_then(Value::as_i64).unwrap_or(0);
            let liked = body.get("liked").and_then(Value::as_bool).unwrap_or(true);
            if !library.set_liked(id, liked, now)? {
                return Ok(Some(refuse(404, "no such song")));
            }
            answer = json!({ "id": id, "liked": liked });
        }
        // A track an add-on found becomes the library's own: the row it has for that song, or a new one.
        ("/api/tracks/keep", _) => {
            let addon = body
                .get("addon")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(item) =
                addon_queue(addon, body.get("track").map(|t| json!([t])).as_ref()).pop()
            else {
                return Ok(Some(refuse(400, "not a track of an add-on")));
            };
            let theirs = item.addon.expect("made from an add-on's track");
            let duration_ms = body["track"]
                .get("duration_ms")
                .and_then(Value::as_u64)
                .filter(|ms| *ms > 0);
            let id = library.adopt(&Song {
                title: item.title,
                artist: item.artist,
                album: item.album,
                duration_ms,
                refs: vec![(theirs.addon, theirs.track)],
                art_url: item.art,
                added_at_ms: Some(now),
                ..Song::default()
            })?;
            answer = json!({ "id": id });
        }
        // A playlist; with rules, a smart one, which fills itself.
        ("/api/playlists", _) => {
            let name = body.get("name").and_then(Value::as_str).unwrap_or_default();
            let id = match body.get("rules").filter(|r| !r.is_null()) {
                Some(_) => match rules_in(&body) {
                    Ok(rules) => library.create_smart_playlist(name, &rules, now)?,
                    Err(why) => return Ok(Some(refuse(400, &why))),
                },
                None => library.create_playlist(name, now)?,
            };
            answer = json!({ "id": id });
        }
        (_, Some((id, verb))) => {
            if matches!(verb, "add" | "remove" | "move") && library.smart_rules(id)?.is_some() {
                return Ok(Some(refuse(
                    409,
                    "a smart playlist fills itself: its songs are its rules'",
                )));
            }
            let done = match verb {
                "rules" => match rules_in(&body) {
                    Ok(rules) => library.set_smart_rules(id, &rules, now)?,
                    Err(why) => return Ok(Some(refuse(400, &why))),
                },
                "rename" => library.rename_playlist(
                    id,
                    body.get("name").and_then(Value::as_str).unwrap_or_default(),
                    now,
                )?,
                "delete" => library.delete_playlist(id)?,
                // Into a folder, or (`folder` null) out of any.
                "folder" => {
                    library.set_playlist_folder(id, body.get("folder").and_then(Value::as_i64))?
                }
                "add" => {
                    let ids: Vec<i64> = body
                        .get("ids")
                        .and_then(Value::as_array)
                        .map(|ids| {
                            ids.iter()
                                .filter_map(Value::as_i64)
                                .take(PAGE_CAP)
                                .collect()
                        })
                        .unwrap_or_default();
                    answer = json!({ "added": library.add_to_playlist(id, &ids, now)? });
                    true
                }
                // Several at once (`places`: each place, and the song there), or one by its place.
                "remove" => match body.get("places").and_then(Value::as_array) {
                    Some(places) => {
                        let places: Vec<(usize, i64)> = places
                            .iter()
                            .filter_map(|p| {
                                Some((p.get("index")?.as_u64()? as usize, p.get("id")?.as_i64()?))
                            })
                            .take(PICKS_CAP)
                            .collect();
                        library.remove_many_from_playlist(id, &places, now)?
                    }
                    None => library.remove_from_playlist(
                        id,
                        number("index").unwrap_or(usize::MAX),
                        now,
                    )?,
                },
                "move" => library.move_in_playlist(
                    id,
                    number("from").unwrap_or(usize::MAX),
                    number("to").unwrap_or(usize::MAX),
                    now,
                )?,
                _ => return Ok(None),
            };
            answer["done"] = json!(done);
        }
        _ => return Ok(None),
    }
    // Every answer carries the lists as they now are, so the window need not ask again; and the count of changes, so it
    // can tell this change from one made elsewhere.
    answer["playlists"] = json!(library.playlists()?);
    answer["folders"] = json!(library.playlist_folders()?);
    answer["counts"] = json!(library.counts()?);
    state.edited();
    answer["edits"] = json!(state.edits.load(Ordering::Relaxed));
    Ok(Some(reply_json(200, answer)))
}

/// Takes in a library from another Stash: the request's body is the file (the shared library file, the Android
/// app's backup, or its database). The file is read and checked with nothing
/// else waiting, then merged in one go.
fn import_route(state: &State, request: &mut Request) -> Result<Reply> {
    let mut bytes = Vec::new();
    request
        .as_reader()
        .take(IMPORT_CAP + 1)
        .read_to_end(&mut bytes)?;
    let refused = |e: &ImportError| refuse(400, &e.to_string());
    if bytes.len() as u64 > IMPORT_CAP {
        return Ok(refused(&ImportError::TooBig));
    }
    let incoming = match stash_core::import::read_bytes(&bytes) {
        Ok(incoming) => incoming,
        Err(e) => return Ok(refused(&e)),
    };
    let mut library = state.library.lock().unwrap();
    let keys = |library: &Library| -> stash_core::Result<HashSet<(&'static str, String)>> {
        Ok(library
            .blocks()?
            .into_iter()
            .map(|b| (b.kind, b.key))
            .collect())
    };
    let before = keys(&library)?;
    let summary = library.import(&incoming)?;
    // Songs the listener chose not to play on the phone leave the queue here too.
    let new: Vec<(BlockKind, String)> = if summary.blocks_added > 0 {
        keys(&library)?
            .into_iter()
            .filter(|b| !before.contains(b))
            .filter_map(|(kind, key)| BlockKind::from_word(kind).map(|kind| (kind, key)))
            .collect()
    } else {
        Vec::new()
    };
    let counts = library.counts()?;
    drop(library);
    crate::blocking::clear_queue(state, &new);
    state.edited();
    Ok(reply_json(
        200,
        json!({
            "source": incoming.source,
            "generator": incoming.generator,
            "summary": summary,
            "counts": counts,
        }),
    ))
}

/// The answer to one request, and whether the server is done (a measurement arrived).
fn route(state: &State, request: &mut Request) -> Result<(Reply, bool)> {
    let url =
        Url::parse(&format!("http://local{}", request.url())).context("the request's path")?;
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };
    let number = |name: &str, default: usize| {
        param(name)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    };
    let limit = |default: usize| number("limit", default).clamp(1, PAGE_CAP);

    let path = url.path().to_string();
    if let Some(response) = player_route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = addons_route(state, request, &path, &param)? {
        return Ok((response, false));
    }
    let library = || state.library.lock().unwrap();
    if *request.method() == Method::Post && path == "/api/import" {
        return Ok((import_route(state, request)?, false));
    }
    // Before the lists' own routes, which read a request's body before passing on a path they do not answer.
    if let Some(response) = crate::saving::kept_route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = lists_route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = scan_route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::saving::route(state, request, &path, &param)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::unfound::route(state, request, &path, &param)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::relay::route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::scrobbling::route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::shortcuts::route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::blocking::route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = crate::folders::route(state, request, &path)? {
        return Ok((response, false));
    }
    if let Some(response) = offline_route(state, request, &path)? {
        return Ok((response, false));
    }
    // One album or one artist: /api/albums/{id}, /api/artists/{id}; the picture of a file: /api/art/track/{id}
    if *request.method() == Method::Get {
        let one = |prefix: &str| {
            path.strip_prefix(prefix)
                .and_then(|id| id.parse::<i64>().ok())
        };
        if let Some(id) = one(stash_core::covers::ADDRESS) {
            // The file is read without the library held: a large picture on a slow disk holds nothing else up.
            let Some(file) = library().file_of(id)? else {
                return Ok((refuse(404, "no such file in the library"), false));
            };
            let file = Path::new(&file);
            let cover = stash_core::covers::of_file(file);
            // "No picture" is kept only for a file that is there to be read: a drive that is unplugged for now
            // says nothing about its files.
            if cover.is_some() || file.is_file() {
                library().set_cover(id, cover.is_some())?;
            }
            return Ok((
                match cover {
                    // The picture of a file changes only when the file does; the window may keep it for the hour.
                    Some(cover) => reply_picture(cover.content_type, cover.bytes),
                    None => refuse(404, "this file has no picture"),
                },
                false,
            ));
        }
        if let Some(id) = one("/api/albums/") {
            return Ok((
                match library().album(id)? {
                    Some((album, rows)) => reply_json(200, json!({ "album": album, "rows": rows })),
                    None => refuse(404, "no such album"),
                },
                false,
            ));
        }
        if let Some(id) = one("/api/artists/") {
            let library = library();
            return Ok((
                match library.artist(id, 50)? {
                    // `with`: their songs with others ("A & B", "B feat. A"), credited apart.
                    Some((artist, rows, albums)) => reply_json(
                        200,
                        json!({
                            "artist": artist,
                            "rows": rows,
                            "albums": albums,
                            "with": library.artist_with_others(id, 50)?,
                        }),
                    ),
                    None => refuse(404, "no such artist"),
                },
                false,
            ));
        }
    }
    // A playlist's tracks: /api/playlists/{id}/tracks
    if *request.method() == Method::Get
        && let Some(id) = path
            .strip_prefix("/api/playlists/")
            .and_then(|rest| rest.strip_suffix("/tracks"))
            .and_then(|id| id.parse::<i64>().ok())
    {
        let rows = library().playlist_tracks_at(id, number("offset", 0), limit(200))?;
        return Ok((reply_json(200, json!({ "rows": rows })), false));
    }
    let response = match (request.method(), url.path()) {
        (Method::Get, "/api/counts") => {
            let library = library();
            let mut counts = json!(library.counts()?);
            // Whether the liked songs are kept on this computer (a playlist's row says so of itself).
            counts["liked_kept"] = json!(
                library
                    .setting(stash_core::lists::DOWNLOAD_LIKED)?
                    .as_deref()
                    == Some("1")
            );
            reply_json(200, counts)
        }
        // `here=1`: only the songs that play with no connection (Library › Songs, "On this computer").
        (Method::Get, "/api/tracks") if param("here").as_deref() == Some("1") => reply_json(
            200,
            json!({ "rows": library().tracks_here_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/tracks") => reply_json(
            200,
            json!({ "rows": library().tracks_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/tracks/here") => {
            reply_json(200, json!({ "count": library().here_count()? }))
        }
        (Method::Get, "/api/liked") => reply_json(
            200,
            json!({ "rows": library().liked_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/playlists") => {
            reply_json(200, json!({ "rows": library().playlists()? }))
        }
        (Method::Get, "/api/history") => reply_json(
            200,
            json!({ "rows": library().history_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/artists") => reply_json(
            200,
            json!({ "rows": library().artists_at(number("offset", 0), limit(200))? }),
        ),
        (Method::Get, "/api/albums") => reply_json(
            200,
            json!({ "rows": library().albums_at(number("offset", 0), limit(120))? }),
        ),
        (Method::Get, "/api/lyrics") => reply_json(200, lyrics_for(state, &param)?),
        // Whether lyrics are looked up on LRCLIB (on unless switched off).
        // Whether songs like the last ones are added when the queue ends (on unless switched off).
        (Method::Get, "/api/settings/autoplay") => {
            reply_json(200, json!({ "on": crate::autoplay::on(&library()) }))
        }
        (Method::Post, "/api/settings/autoplay") => {
            let on = body_json(request)?
                .get("on")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            library().set_setting(crate::autoplay::SETTING, if on { "1" } else { "0" })?;
            reply_json(200, json!({ "on": on }))
        }
        // Whether closing the window keeps Stash playing in the tray (off unless switched on).
        (Method::Get, "/api/settings/window") => reply_json(
            200,
            json!({ "tray": library().setting("window.tray")?.as_deref() == Some("1") }),
        ),
        (Method::Post, "/api/settings/window") => {
            let on = body_json(request)?
                .get("tray")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            library().set_setting("window.tray", if on { "1" } else { "0" })?;
            reply_json(200, json!({ "tray": on }))
        }
        // Whether there is a mini player to switch to (only in a program that owns windows), and the switch.
        (Method::Get, "/api/window") => reply_json(200, json!({ "mini": state.window.is_some() })),
        (Method::Post, "/api/window") => {
            let view = match body_json(request)?.get("view").and_then(Value::as_str) {
                Some("mini") => View::Mini,
                Some("full") => View::Full,
                _ => return Ok((refuse(400, "the view is \"mini\" or \"full\""), false)),
            };
            match &state.window {
                None => refuse(404, "there is no window here to change"),
                Some(shell) if shell(view) => reply_json(
                    200,
                    json!({ "view": if view == View::Mini { "mini" } else { "full" } }),
                ),
                Some(_) => refuse(503, "the window could not change"),
            }
        }
        (Method::Get, "/api/settings/lyrics") => reply_json(
            200,
            json!({ "lrclib": library().setting(LRCLIB_SETTING)?.as_deref() != Some("0") }),
        ),
        // The artists each credit names (two or more), each with their artist here when the library knows them by
        // themselves: the window links each name to its own page. One credit names one artist: no parts.
        (Method::Post, "/api/credits") => {
            let body = body_json(request)?;
            let library = library();
            let mut known = std::collections::HashMap::new();
            let mut answer = serde_json::Map::new();
            let asked = body.get("credits").and_then(Value::as_array);
            for credit in asked
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(CREDITS_CAP)
            {
                if answer.contains_key(credit) {
                    continue;
                }
                let parts = if credit.len() > CREDIT_LEN {
                    Vec::new()
                } else {
                    library.credit_parts_with(credit, &mut known)?
                };
                answer.insert(credit.to_string(), json!(parts));
            }
            reply_json(200, json!({ "credits": answer }))
        }
        (Method::Post, "/api/settings/lyrics") => {
            let on = body_json(request)?
                .get("lrclib")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            library().set_setting(LRCLIB_SETTING, if on { "1" } else { "0" })?;
            reply_json(200, json!({ "lrclib": on }))
        }
        (Method::Get, "/api/search") => {
            let (query, library) = (param("q").unwrap_or_default(), library());
            // `here=1`: among the songs that play with no connection only (Library › Songs, "On this computer").
            let rows = if param("here").as_deref() == Some("1") {
                library.search_here(&query, limit(50))?
            } else {
                library.search(&query, limit(50))?
            };
            reply_json(
                200,
                json!({
                    "rows": rows,
                    "albums": library.search_albums(&query, 12)?,
                    "artists": library.search_artists(&query, 12)?,
                }),
            )
        }
        (Method::Post, "/api/bench-result") => {
            let mut body = String::new();
            request
                .as_reader()
                .take(BODY_CAP)
                .read_to_string(&mut body)?;
            let value: Value =
                serde_json::from_str(&body).context("the measurement is not JSON")?;
            let Some(out) = &state.options.bench_out else {
                return Ok((
                    refuse(404, "this server is not collecting a measurement"),
                    false,
                ));
            };
            fs::write(out, serde_json::to_string_pretty(&value)?)
                .with_context(|| format!("writing {}", out.display()))?;
            println!("measurement written to {}", out.display());
            return Ok((reply_json(200, json!({ "ok": true })), true));
        }
        (Method::Get, path) if !path.starts_with("/api/") => match &state.options.ui {
            Some(root) => ui_file(root, path),
            None => refuse(404, "no UI is served here"),
        },
        _ => refuse(404, "no such route"),
    };
    Ok((response, false))
}

/// A file of the built UI. Only plain names below the UI folder: no `..`, no drive letters, nothing absolute.
fn ui_file(root: &Path, path: &str) -> Reply {
    let relative = path.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let plain = relative
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains([':', '\\']));
    if !plain {
        return refuse(400, "not a file of the UI");
    }
    let file = root.join(relative);
    let content_type = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("woff2") => "font/woff2",
        Some("ico") => "image/x-icon",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    match fs::read(&file) {
        Ok(bytes) => reply(200, content_type, bytes),
        // The window is one page that draws its own screens: an address like `/library` or `/album/lib/7` is a
        // screen of it, not a file, so a reload there gets the page again. A missing file with an extension
        // stays missing.
        Err(_) if file.extension().is_none() => match fs::read(root.join("index.html")) {
            Ok(bytes) => reply(200, "text/html; charset=utf-8", bytes),
            Err(_) => refuse(404, "no such file"),
        },
        Err(_) => refuse(404, "no such file"),
    }
}
