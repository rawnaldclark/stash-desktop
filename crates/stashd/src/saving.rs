//! Saving songs as files: the list of songs to save, worked through one at a time on a thread of its own, and the
//! routes the window uses for it.
//!
//! A song of the library is found through the listener's add-ons as it is for playing (its known add-on first, then
//! each add-on in the listener's order), and the add-on is asked for a file to keep (contract section 4.5). The
//! file goes under the downloads folder; its row in the library becomes that file, keeping its likes, playlists and
//! plays. Taking it off the list again deletes the file and the song goes back to playing through the add-ons.

use std::cell::Cell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::{Code, StreamRequest};
use stash_core::downloads::{self, Song, Trouble};
use stash_core::lists::SavedDownload;
use stash_core::player::Failure;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json, through_addons};

/// The setting that names the downloads folder.
const FOLDER: &str = "downloads.folder";
/// How often the one coming in checks it is still wanted.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// How often, with nothing to save, the lists kept on this computer are looked at for songs that came onto them (a
/// smart playlist's rules pick new songs as they are played). A change to the likes or the playlists looks at once.
const LISTS_EVERY: Duration = Duration::from_secs(30);

/// The song coming in, how far, and a bell to ring when there is something new on the list.
#[derive(Default)]
pub(crate) struct Saving {
    now: Mutex<Option<Progress>>,
    bell: (Mutex<bool>, Condvar),
    /// The lists kept on this computer are to be looked at again.
    lists: AtomicBool,
    /// Saving waits (see [`Held`]).
    held: Mutex<Option<Held>>,
    /// Trouble that may be a song's own (see [`Saving::unsure`]).
    tries: Mutex<Tries>,
}

/// Trouble that may be the song's own or everyone's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unsure {
    /// A server (the add-on's, or the file's) could not be reached, was busy, or stopped sending.
    Busy,
    /// "No" from add-ons that are not finding other songs either.
    Doubt,
    /// The add-on refused the request, or answered it with what the contract does not allow.
    Refused(Code),
}

/// For each song, how many of its tries had trouble that may be its own; and the trouble of the song tried last, while
/// nothing else came between (a song saved, a song failed for its own reasons, trouble that is everyone's).
#[derive(Default)]
struct Tries {
    counts: HashMap<i64, u32>,
    last: Option<(i64, Unsure)>,
}

/// What saving does next with a song that had trouble that may be its own.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// To the back of the line; the next song is tried at once.
    Next,
    /// To the back of the line, and saving waits before it asks again.
    Wait,
    /// Given up on.
    GiveUp,
}

/// A song is given up on at its third try with trouble that may be its own.
const TRIES: u32 = 3;

/// Saving waits: after trouble that is everyone's (the add-on's allowance is spent or its key refused, too many asks,
/// no add-on can be asked, the downloads folder cannot be written), when two songs in a row had the same trouble, or
/// when the one song left had trouble again. Nothing is asked until `until`; then the song first in line is tried (the
/// one held went to the back). Each time it is held again it waits twice as long, up to an hour, or as long as the
/// add-on asked; a song saved ends it, and so do the listener's "Try again", an add-on added or switched on, and a new
/// downloads folder.
struct Held {
    why: String,
    until: Instant,
    until_ms: i64,
    wait: Duration,
}

/// How long saving first waits after trouble with an add-on, and the longest it waits.
const HELD_FIRST: Duration = Duration::from_secs(60);
const HELD_MOST: Duration = Duration::from_secs(3600);
/// The shortest wait an add-on's own "ask again in" is taken for.
const HELD_LEAST: Duration = Duration::from_secs(5);
/// Why saving waits after "no"s that were not believed.
const NOT_FINDING: &str = "your add-ons are not finding songs just now";

#[derive(Clone, Copy, serde::Serialize)]
struct Progress {
    id: i64,
    bytes: u64,
    of: Option<u64>,
}

impl Saving {
    /// Something was put on the list, or the lists kept on this computer may hold new songs: the worker looks at once.
    pub fn ring(&self) {
        self.lists.store(true, Ordering::Relaxed);
        *self.bell.0.lock().unwrap() = true;
        self.bell.1.notify_all();
    }

    /// Saving waits after trouble that is not the song's own: as long as the add-on asked (`after`, in seconds), else
    /// a minute, and held again, twice as long.
    fn hold(&self, why: String, after: Option<u32>) {
        let mut held = self.held.lock().unwrap();
        let step = held
            .as_ref()
            .map_or(HELD_FIRST, |h| (h.wait * 2).min(HELD_MOST));
        let wait = after
            .map(|s| Duration::from_secs(u64::from(s)).clamp(HELD_LEAST, HELD_MOST))
            .unwrap_or(step);
        *held = Some(Held {
            why,
            until: Instant::now() + wait,
            until_ms: now_ms() + wait.as_millis() as i64,
            wait,
        });
    }

    /// The trouble is over (a song was saved, the listener asked to try again, an add-on was added or switched on,
    /// the downloads folder changed).
    pub(crate) fn release(&self) {
        *self.held.lock().unwrap() = None;
    }

    /// The song was saved.
    fn saved_one(&self, id: i64) {
        let mut tries = self.tries.lock().unwrap();
        tries.counts.remove(&id);
        tries.last = None;
    }

    /// Something else came of a song than trouble that may be its own (it failed for its own reasons, or the trouble
    /// was everyone's): a run of such trouble ends.
    fn settled(&self) {
        self.tries.lock().unwrap().last = None;
    }

    /// Trouble that may be the song's own (`kind`), for the song `id`. Each try that asked something (`asked`) counts
    /// against the song: it is given up on at its third, or at once for a refusal the song before did not have. Trouble
    /// that is everyone's never comes here (see [`Trouble::Held`]), so a spent allowance gives no song up.
    ///
    /// When the song before had the same trouble, it is likely everyone's (the add-on or the line is down, its lookup
    /// is broken, it refuses every request), and saving waits instead of asking for the next song at once; it waits too
    /// when the same song comes again (it is the only one left), so nothing is asked over and over.
    fn unsure(&self, id: i64, kind: Unsure, asked: bool) -> Step {
        let mut tries = self.tries.lock().unwrap();
        let Tries { counts, last } = &mut *tries;
        let run = matches!(*last, Some((other, k)) if other != id && k == kind);
        let again = matches!(*last, Some((other, _)) if other == id);
        *last = Some((id, kind));
        if asked {
            let n = counts.entry(id).or_insert(0);
            *n += 1;
            let most = match kind {
                Unsure::Refused(_) if !run => 1,
                _ => TRIES,
            };
            if *n >= most {
                counts.remove(&id);
                return Step::GiveUp;
            }
        }
        if run || again { Step::Wait } else { Step::Next }
    }

    /// The listener asks to try again: every song starts with a clean slate.
    fn forgive(&self) {
        *self.tries.lock().unwrap() = Tries::default();
        self.release();
    }

    /// How long saving still waits, if it does.
    fn held_for(&self) -> Option<Duration> {
        self.held
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|h| h.until.checked_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    /// For the list: why saving waits, and until when (null when it does not).
    fn held_json(&self) -> Value {
        match &*self.held.lock().unwrap() {
            Some(h) if h.until > Instant::now() => json!({ "why": h.why, "until_ms": h.until_ms }),
            _ => Value::Null,
        }
    }

    fn wait(&self, at_most: Duration) {
        let rung = self.bell.0.lock().unwrap();
        let (mut rung, _) = self
            .bell
            .1
            .wait_timeout_while(rung, at_most, |rung| !*rung)
            .unwrap();
        *rung = false;
    }
}

/// Where songs are saved: the folder the listener chose, else "Stash" in the system's music folder.
pub(crate) fn folder(library: &Library) -> PathBuf {
    library
        .setting(FOLDER)
        .ok()
        .flatten()
        .filter(|f| !f.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::audio_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
                .unwrap_or_else(std::env::temp_dir)
                .join("Stash")
        })
}

/// Works through the list until `stop`: the first asked for first, one at a time (an add-on's allowance is spent
/// one song at a time, and a line is not shared between two big files).
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let _ = state.library.lock().unwrap().requeue_downloads();
    let mut looked: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        // Offline, nothing is fetched: what was asked for waits.
        if crate::serve::offline(state) {
            state.saving.wait(Duration::from_secs(2));
            continue;
        }
        // After trouble with an add-on, nothing is asked until it is time.
        if let Some(left) = state.saving.held_for() {
            state.saving.wait(left.min(Duration::from_secs(2)));
            continue;
        }
        // The downloads are brought into line with the lists kept on this computer: first of all, after any change to
        // the likes or the playlists (before the next song is fetched, so a song that left them is not), and now and
        // then while a smart playlist is kept (its songs change as songs are played).
        let changed = state.saving.lists.swap(false, Ordering::Relaxed);
        let due = looked.is_none_or(|at| at.elapsed() >= LISTS_EVERY);
        if changed || due {
            let mut library = state.library.lock().unwrap();
            if (changed || looked.is_none() || library.kept_smart_lists().unwrap_or(false))
                && let Err(e) = library.reconcile_kept(now_ms())
            {
                eprintln!("the lists kept on this computer were not read: {e:#}");
            }
            looked = Some(Instant::now());
        }
        let next = state.library.lock().unwrap().next_download().ok().flatten();
        match next {
            Some(id) => save(state, id, stop),
            None => state.saving.wait(Duration::from_secs(2)),
        }
    }
}

fn save(state: &State, id: i64, stop: &AtomicBool) {
    let outcome = fetch_and_keep(state, id, stop);
    *state.saving.now.lock().unwrap() = None;
    let failed = |why: &str| {
        let _ = state.library.lock().unwrap().download_failed(id, why);
    };
    let back = || {
        let _ = state.library.lock().unwrap().download_held(id, now_ms());
    };
    // Trouble that may be the song's own: tried again later, given up on at last (see `Saving::unsure`). `waits` says
    // why saving waits, when it does.
    let unsure =
        |kind: Unsure, why: String, waits: String, after: Option<u32>, asked: bool| match state
            .saving
            .unsure(id, kind, asked)
        {
            Step::GiveUp => failed(&why),
            Step::Next => back(),
            Step::Wait => {
                back();
                state.saving.hold(waits, after);
            }
        };
    match outcome {
        Ok(()) => {
            state.saving.release();
            state.saving.saved_one(id);
        }
        // The song's own: given up on.
        Err(Trouble::Failed(why)) => {
            state.saving.settled();
            failed(&why);
        }
        // Everyone's: the song waits its turn again at the back of the line, and saving waits before it asks again (an
        // add-on whose allowance is spent is not asked for every song on the list). It never gives a song up.
        Err(Trouble::Held(why, after)) => {
            state.saving.settled();
            back();
            state.saving.hold(why, after);
        }
        Err(Trouble::Busy(why, after)) => unsure(Unsure::Busy, why.clone(), why, after, true),
        Err(Trouble::Later(why, asked)) => {
            unsure(Unsure::Doubt, why, NOT_FINDING.into(), None, asked)
        }
        Err(Trouble::Refused(why, code)) => {
            unsure(Unsure::Refused(code), why.clone(), why, None, true)
        }
        // Stopped because Stash went offline: it waits for the connection, first on the list as it was.
        Err(Trouble::Cancelled) if crate::serve::offline(state) => {
            let _ = state.library.lock().unwrap().requeue_downloads();
        }
        Err(Trouble::Cancelled) => {}
    }
}

fn fetch_and_keep(state: &State, id: i64, stop: &AtomicBool) -> Result<(), Trouble> {
    let (song, art, root) = {
        let library = state.library.lock().unwrap();
        let described = library
            .describe(id)
            .map_err(|e| Trouble::Failed(e.to_string()))?
            .ok_or_else(|| Trouble::Failed("it is no longer in the library".into()))?;
        let art = library.art_url_of(id).ok().flatten();
        // Tagged with what the library knows the song by, not with how it was asked for.
        let kept = described.kept;
        let song = Song {
            title: kept.title,
            artist: kept.artist,
            album: kept.album,
            track_no: library.track_number(id).ok().flatten(),
            isrc: kept.isrc,
        };
        (song, art, folder(&library))
    };
    // It comes in under a name of its own at the top of the folder, and is moved where it belongs once it is whole
    // and its kind is known.
    std::fs::create_dir_all(&root)?;
    let part = root.join(format!(".stash-{id}.part"));
    // A copy kept from playing the song is the song already: it is taken from there, and no add-on is asked. Only when
    // the add-on it came from lets its songs be saved, and its download would not be better: an Eclipse add-on's
    // download is its stream; another's may be a better file than the stream, unless the copy is lossless already.
    let allowed = |copy: &stash_core::played::PlayedCopy| {
        let Some(from) = copy.addon.as_deref() else {
            return false;
        };
        let lossless = matches!(copy.tier.as_deref(), Some("lossless" | "hires"));
        state.addons.read().unwrap().iter().any(|loaded| {
            loaded.stored.id == from
                && loaded.usable().is_some_and(|addon| {
                    addon.offers_download()
                        && (addon.manifest.dialect == stash_core::addons::Dialect::Eclipse
                            || lossless)
                })
        })
    };
    let copy = state
        .relays
        .copy(&state.library, id)
        .filter(|(_, copy)| allowed(copy));
    let (head, addon, tier) = if let Some((kept, copy)) = copy {
        let copied = std::fs::copy(&kept, &part).and_then(|_| {
            let mut head = vec![0u8; 64];
            let n = std::fs::File::open(&part)?.read(&mut head)?;
            head.truncate(n);
            Ok(head)
        });
        match copied {
            Ok(head) if downloads::kind_of(&head).is_some() => {
                (head, copy.addon.unwrap_or_default(), copy.tier)
            }
            Ok(_) => {
                let _ = std::fs::remove_file(&part);
                return Err(Trouble::Failed(
                    "the copy kept from playing it is not audio".into(),
                ));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e.into());
            }
        }
    } else {
        let (info, addon) = through_addons(
            &state.addons,
            &state.library,
            &state.lacks,
            id,
            "there is no add-on to save it from",
            |addon| addon.offers_download(),
            |addon, _, track| addon.download(track, &StreamRequest::default()),
        )
        .map_err(trouble_of)?;
        let _ = state.library.lock().unwrap().note_addon_use(
            &addon,
            stash_core::usage::Given::Download,
            now_ms(),
        );
        let (looked, wanted) = (Cell::new(Instant::now()), Cell::new(true));
        let fetched = downloads::fetch(
            &info.url,
            &part,
            &|| !wanted.get() || stop.load(Ordering::Relaxed) || crate::serve::offline(state),
            &mut |bytes, of| {
                *state.saving.now.lock().unwrap() = Some(Progress { id, bytes, of });
                if looked.get().elapsed() >= LOOK_EVERY {
                    looked.set(Instant::now());
                    wanted.set(
                        state
                            .library
                            .lock()
                            .unwrap()
                            .download_wanted(id)
                            .unwrap_or(false),
                    );
                }
            },
        );
        match fetched {
            Ok((head, _)) => (head, addon, Some(info.tier.as_str().to_string())),
            Err(trouble) => {
                let _ = std::fs::remove_file(&part);
                return Err(trouble);
            }
        }
    };
    let kind = downloads::kind_of(&head).unwrap_or("audio");
    let file = downloads::path_for(&root, &song, kind);
    let placed = file
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::rename(&part, &file));
    if let Err(e) = placed {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    let kept = (|| {
        let cover = art.as_deref().and_then(downloads::fetch_cover);
        let cover_inside = downloads::tag(&file, &song, cover.as_deref())?;
        let read = downloads::read(&file)?;
        let saved = SavedDownload {
            addon,
            tier,
            at_ms: now_ms(),
            cover_inside,
        };
        state
            .library
            .lock()
            .unwrap()
            .download_saved(id, &read, &saved)
            .map_err(|e| Trouble::Failed(e.to_string()))
    })();
    match kept {
        // A file now: a copy kept from playing it is not needed any more.
        Ok(true) => {
            let copy = state.library.lock().unwrap().drop_played_copy(id);
            if let Ok(Some(file)) = copy {
                state.relays.delete(vec![file]);
            }
            Ok(())
        }
        // Taken off the list while its tags were written: nothing is kept.
        Ok(false) => {
            remove_saved(&file, &root);
            Err(Trouble::Cancelled)
        }
        Err(trouble) => {
            remove_saved(&file, &root);
            Err(trouble)
        }
    }
}

/// What saving makes of the add-ons not giving the song (see `save`). Everyone's: an allowance spent, too many asks, a
/// key refused, no add-on to ask, the library not read. Maybe the song's own: an add-on down, out of reach or too slow;
/// "no"s from add-ons that are not finding other songs either; a request the add-on refused, or answered with what the
/// contract does not allow. The song's own: no add-on has it (when that is believed).
fn trouble_of(failure: Failure) -> Trouble {
    match failure.code {
        Some(Code::QuotaExceeded | Code::RateLimited | Code::Unauthorized) => {
            Trouble::Held(failure.message, failure.retry_after)
        }
        Some(Code::Unavailable | Code::Timeout | Code::Network) => {
            Trouble::Busy(failure.message, failure.retry_after)
        }
        Some(code) => Trouble::Refused(failure.message, code),
        None if failure.nothing_to_ask || !failure.miss => Trouble::Held(failure.message, None),
        None if failure.doubtful => Trouble::Later(failure.message, failure.asked),
        None => Trouble::Failed(failure.message),
    }
}

/// Deletes a saved file, and its album and artist folders when they are left empty (never `root` itself).
fn remove_saved(file: &Path, root: &Path) {
    let _ = std::fs::remove_file(file);
    let mut folder = file.parent();
    while let Some(dir) = folder {
        if dir == root || !dir.starts_with(root) || std::fs::remove_dir(dir).is_err() {
            break;
        }
        folder = dir.parent();
    }
}

/// The downloads routes. `None`: the path is not one of them.
/// A list kept on this computer (`/api/liked/download`, `/api/playlists/{id}/download`). GET: whether it is, and how
/// far (its songs, how many play with no connection, how many wait to be saved, how many could not be). POST `{on}`:
/// keeps it or stops; kept, its songs are put on the list at once, and songs that come onto it later are saved too.
/// Stopping saves no more of it: its songs still waiting are taken off the list (unless another list kept on this
/// computer holds them), and songs saved stay on this computer (Library › Downloads takes them off).
pub(crate) fn kept_route(
    state: &State,
    request: &mut Request,
    path: &str,
) -> anyhow::Result<Option<Reply>> {
    let playlist = match path {
        "/api/liked/download" => None,
        _ => match path
            .strip_prefix("/api/playlists/")
            .and_then(|rest| rest.strip_suffix("/download"))
        {
            Some(id) => match id.parse::<i64>() {
                Ok(id) => Some(id),
                Err(_) => return Ok(None),
            },
            None => return Ok(None),
        },
    };
    let method = request.method().clone();
    if !matches!(method, Method::Get | Method::Post) {
        return Ok(None);
    }
    let on = if method == Method::Post {
        let Some(on) = body_json(request)?.get("on").and_then(Value::as_bool) else {
            return Ok(Some(refuse(400, "on is true or false")));
        };
        Some(on)
    } else {
        None
    };
    let mut library = state.library.lock().unwrap();
    let kept = match playlist {
        None => {
            library
                .setting(stash_core::lists::DOWNLOAD_LIKED)?
                .as_deref()
                == Some("1")
        }
        Some(id) => match library.playlist_download(id)? {
            Some(kept) => kept,
            None => return Ok(Some(refuse(404, "no such playlist"))),
        },
    };
    let ids = |library: &Library| -> stash_core::Result<Vec<i64>> {
        match playlist {
            None => library.liked_ids_at(0, usize::MAX >> 1),
            Some(id) => library.playlist_track_ids(id, 0, usize::MAX >> 1),
        }
    };
    let changed = match on {
        Some(on) if on != kept => {
            match playlist {
                None => library.set_setting(
                    stash_core::lists::DOWNLOAD_LIKED,
                    if on { "1" } else { "0" },
                )?,
                Some(id) => {
                    library.set_playlist_download(id, on)?;
                }
            }
            // Kept: its songs are put on the list at once. Let go: its songs still waiting leave (unless another kept
            // list holds them, or they were asked for by hand).
            library.reconcile_kept(now_ms())?;
            Some(on)
        }
        _ => None,
    };
    let progress = library.download_progress(&ids(&library)?)?;
    drop(library);
    let mut answer = json!(progress);
    answer["on"] = json!(changed.unwrap_or(kept));
    if changed.is_some() {
        // The playlists' rows say which are kept: other windows read them again; this one has its answer.
        state.edited();
        answer["edits"] = json!(state.edits.load(std::sync::atomic::Ordering::Relaxed));
    }
    Ok(Some(reply_json(200, answer)))
}

pub(crate) fn route(
    state: &State,
    request: &mut Request,
    path: &str,
    param: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<Reply>> {
    if !path.starts_with("/api/downloads") {
        return Ok(None);
    }
    let listed = |state: &State, offset: usize, limit: usize| -> anyhow::Result<Value> {
        let library = state.library.lock().unwrap();
        Ok(json!({
            "rows": library.downloads_at(offset, limit)?,
            "counts": library.download_counts()?,
            "now": *state.saving.now.lock().unwrap(),
            "held": state.saving.held_json(),
            "folder": folder(&library).to_string_lossy(),
        }))
    };
    let number = |name: &str, default: usize| {
        param(name)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    };
    match (request.method(), path) {
        // The list: the one coming in, those waiting, those that failed, those saved.
        (Method::Get, "/api/downloads") => Ok(Some(reply_json(
            200,
            listed(
                state,
                number("offset", 0),
                number("limit", 200).clamp(1, 500),
            )?,
        ))),
        // Puts songs on the list: `ids`, or `liked: true`, or a `playlist`.
        (Method::Post, "/api/downloads") => {
            let body = body_json(request)?;
            let ids = {
                let library = state.library.lock().unwrap();
                if body.get("liked").and_then(Value::as_bool) == Some(true) {
                    library.liked_ids_at(0, usize::MAX >> 1)?
                } else if let Some(playlist) = body.get("playlist").and_then(Value::as_i64) {
                    library.playlist_track_ids(playlist, 0, usize::MAX >> 1)?
                } else {
                    ids_of(&body)
                }
            };
            let queued = state
                .library
                .lock()
                .unwrap()
                .ask_downloads(&ids, now_ms())?;
            state.saving.ring();
            let mut answer = listed(state, 0, 0)?;
            answer["queued"] = json!(queued);
            Ok(Some(reply_json(200, answer)))
        }
        // Takes songs off the list; a saved one's file is deleted and the song plays through the add-ons again.
        (Method::Post, "/api/downloads/remove") => {
            let ids = ids_of(&body_json(request)?);
            let files = state.library.lock().unwrap().forget_downloads(&ids)?;
            let root = folder(&state.library.lock().unwrap());
            for file in &files {
                let file = Path::new(file);
                remove_saved(
                    file,
                    file.parent()
                        .filter(|_| !file.starts_with(&root))
                        .unwrap_or(&root),
                );
            }
            Ok(Some(reply_json(200, listed(state, 0, 0)?)))
        }
        // Failed songs back on the list (all of them without `ids`).
        (Method::Post, "/api/downloads/retry") => {
            let ids = ids_of(&body_json(request)?);
            state
                .library
                .lock()
                .unwrap()
                .retry_downloads(&ids, now_ms())?;
            // The listener asks to try again: saving does not wait out trouble it had with an add-on, and the add-ons
            // are asked again for those songs where their "no" was not believed.
            state.lacks.forget_doubted(&ids);
            state.saving.forgive();
            state.saving.ring();
            Ok(Some(reply_json(200, listed(state, 0, 0)?)))
        }
        // Where new songs are saved: `{"folder": path}`, or the system's dialog with `{"pick": true}`. Songs saved
        // before stay where they are.
        (Method::Post, "/api/downloads/folder") => {
            let body = body_json(request)?;
            let chosen = if body.get("pick").and_then(Value::as_bool) == Some(true) {
                rfd::FileDialog::new()
                    .set_title("Choose where songs are saved")
                    .pick_folder()
                    .map(|f| f.to_string_lossy().into_owned())
            } else {
                body.get("folder")
                    .and_then(Value::as_str)
                    .map(|f| f.trim().to_string())
            };
            if let Some(chosen) = chosen {
                if chosen.is_empty() || !Path::new(&chosen).is_absolute() {
                    return Ok(Some(refuse(400, "That is not a folder")));
                }
                if std::fs::create_dir_all(&chosen).is_err() {
                    return Ok(Some(refuse(
                        400,
                        "That folder cannot be made or written to",
                    )));
                }
                state.library.lock().unwrap().set_setting(FOLDER, &chosen)?;
                // A folder that can be written: saving does not wait out trouble it had with the last one.
                state.saving.release();
                state.saving.ring();
            }
            Ok(Some(reply_json(200, listed(state, 0, 0)?)))
        }
        _ => Ok(None),
    }
}

fn ids_of(body: &Value) -> Vec<i64> {
    body.get("ids")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_i64).take(100_000).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(code: Option<Code>, miss: bool, doubtful: bool) -> Failure {
        Failure {
            message: "why".into(),
            miss,
            code,
            retry_after: Some(30),
            doubtful,
            asked: true,
            ..Default::default()
        }
    }

    #[test]
    fn trouble_is_everyones_the_songs_own_or_either() {
        // Everyone's: waited out, with the add-on's "ask again in".
        for code in [Code::QuotaExceeded, Code::RateLimited, Code::Unauthorized] {
            assert!(
                matches!(
                    trouble_of(failure(Some(code), false, false)),
                    Trouble::Held(_, Some(30))
                ),
                "{code:?}"
            );
        }
        // Either: an add-on down, out of reach or too slow...
        for code in [Code::Unavailable, Code::Timeout, Code::Network] {
            assert!(
                matches!(
                    trouble_of(failure(Some(code), false, false)),
                    Trouble::Busy(_, Some(30))
                ),
                "{code:?}"
            );
        }
        // ...or a request it refused, or answered with nonsense.
        for code in [Code::InvalidResponse, Code::BadRequest, Code::Refused] {
            assert!(
                matches!(trouble_of(failure(Some(code), false, false)), Trouble::Refused(_, c) if c == code),
                "{code:?}"
            );
        }
        // No add-on has it: believed, the song's own; doubted, either (and whether anything was asked); no add-on to
        // ask, everyone's.
        assert!(matches!(
            trouble_of(failure(None, true, false)),
            Trouble::Failed(_)
        ));
        assert!(matches!(
            trouble_of(failure(None, true, true)),
            Trouble::Later(_, true)
        ));
        assert!(matches!(
            trouble_of(Failure {
                asked: false,
                ..failure(None, true, true)
            }),
            Trouble::Later(_, false)
        ));
        assert!(matches!(
            trouble_of(Failure {
                nothing_to_ask: true,
                ..failure(None, true, false)
            }),
            Trouble::Held(_, None)
        ));
    }

    #[test]
    fn trouble_that_may_be_the_songs_own_is_tried_three_times() {
        let saving = Saving::default();
        // One song alone, its file's server down: tried again at once, then after a wait, then given up on.
        assert_eq!(saving.unsure(1, Unsure::Busy, true), Step::Next);
        assert_eq!(saving.unsure(1, Unsure::Busy, true), Step::Wait);
        assert_eq!(saving.unsure(1, Unsure::Busy, true), Step::GiveUp);
        // Two songs in a row with the same trouble: likely everyone's, and saving waits. Each try still counts, so two
        // songs that both have trouble of their own are not tried for ever.
        saving.settled();
        assert_eq!(saving.unsure(2, Unsure::Busy, true), Step::Next);
        assert_eq!(saving.unsure(3, Unsure::Busy, true), Step::Wait);
        assert_eq!(saving.unsure(2, Unsure::Busy, true), Step::Wait);
        assert_eq!(saving.unsure(3, Unsure::Busy, true), Step::Wait);
        assert_eq!(saving.unsure(2, Unsure::Busy, true), Step::GiveUp);
        assert_eq!(saving.unsure(3, Unsure::Busy, true), Step::GiveUp);
        // A try that asked nothing (only "no"s remembered from before) does not count, and waits when it comes again.
        saving.settled();
        assert_eq!(saving.unsure(4, Unsure::Doubt, true), Step::Next);
        for _ in 0..5 {
            assert_eq!(saving.unsure(4, Unsure::Doubt, false), Step::Wait);
        }
        assert_eq!(saving.unsure(4, Unsure::Doubt, true), Step::Wait);
        assert_eq!(saving.unsure(4, Unsure::Doubt, true), Step::GiveUp);
        // Other trouble is not the same run. A song saved ends a run, not another song's count.
        assert_eq!(saving.unsure(5, Unsure::Busy, true), Step::Next);
        saving.saved_one(6);
        assert_eq!(saving.unsure(5, Unsure::Busy, true), Step::Next);
        assert_eq!(saving.unsure(5, Unsure::Busy, true), Step::GiveUp);
        // "Try again" starts every song over.
        saving.forgive();
        assert_eq!(saving.unsure(7, Unsure::Busy, true), Step::Next);
        assert_eq!(saving.unsure(7, Unsure::Busy, true), Step::Wait);
        saving.forgive();
        assert_eq!(saving.unsure(7, Unsure::Busy, true), Step::Next);
        assert_eq!(saving.unsure(7, Unsure::Busy, true), Step::Wait);
        assert_eq!(saving.unsure(7, Unsure::Busy, true), Step::GiveUp);
    }

    #[test]
    fn a_refusal_is_the_songs_own_unless_the_song_before_had_it_too() {
        let saving = Saving::default();
        let refused = Unsure::Refused(Code::BadRequest);
        // The first song refused is given up on at once.
        assert_eq!(saving.unsure(1, refused, true), Step::GiveUp);
        // The next song refused the same way: the add-on refuses every request (a plan that lapsed). Saving waits and
        // the song stays in line, given up on only at its third.
        assert_eq!(saving.unsure(2, refused, true), Step::Wait);
        assert_eq!(saving.unsure(3, refused, true), Step::Wait);
        assert_eq!(saving.unsure(2, refused, true), Step::Wait);
        assert_eq!(saving.unsure(3, refused, true), Step::Wait);
        assert_eq!(saving.unsure(2, refused, true), Step::GiveUp);
        // After something else, and for another refusal, it is the song's own again.
        saving.settled();
        assert_eq!(saving.unsure(4, refused, true), Step::GiveUp);
        assert_eq!(
            saving.unsure(5, Unsure::Refused(Code::InvalidResponse), true),
            Step::GiveUp
        );
    }

    #[test]
    fn saving_waits_as_long_as_the_add_on_asked() {
        let saving = Saving::default();
        saving.hold("spent".into(), Some(600));
        let left = saving.held_for().unwrap();
        assert!(
            left > Duration::from_secs(590) && left <= Duration::from_secs(600),
            "{left:?}"
        );
        saving.release();
        saving.hold("down".into(), None);
        assert!(saving.held_for().unwrap() <= HELD_FIRST);
        saving.hold("down".into(), Some(1));
        assert!(
            saving.held_for().unwrap() <= HELD_LEAST,
            "at least a few seconds"
        );
    }
}
