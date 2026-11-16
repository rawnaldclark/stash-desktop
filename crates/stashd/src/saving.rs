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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Saving waits after trouble that is not a song's own (see [`Held`]).
    held: Mutex<Option<Held>>,
    /// Songs saved since Stash started, and for each song held: how many were saved when it was last held, and how many
    /// times it was held again after others were saved (its trouble is its own after all: see [`Saving::struck`]).
    saved: AtomicU64,
    strikes: Mutex<HashMap<i64, (u64, u32)>>,
    /// "No"s that were not believed (see [`Saving::later`]): how many each song has had, and the song the last one was
    /// for, while nothing else has happened since.
    doubts: Mutex<(HashMap<i64, u32>, Option<i64>)>,
}

/// What comes of a "no" that was not believed.
#[derive(Debug, PartialEq, Eq)]
enum Doubt {
    /// The song goes to the back of the line, and the next is tried at once.
    Next,
    /// Two songs in a row had one: the add-ons may be broken, and saving waits before it asks again.
    Wait,
    /// The song's third: it is given up on (its "no"s are not counted as "not found": they were not believed).
    GiveUp,
}

/// A song given up on after this many "no"s that were not believed.
const DOUBTS: u32 = 3;

/// Saving waits: the add-on's allowance is spent, it (or a file's server) could not be reached, no add-on can be asked,
/// or the downloads folder cannot be written. Nothing is asked until `until`; then the song first in line is tried (the
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
/// A song held again this many times after other songs were saved in between is given up on.
const STRIKES: u32 = 2;

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
        self.saved.fetch_add(1, Ordering::Relaxed);
        self.strikes.lock().unwrap().remove(&id);
        let mut doubts = self.doubts.lock().unwrap();
        doubts.0.remove(&id);
        doubts.1 = None;
    }

    /// Something other than a "no" that was not believed came of a song: the run of them ends.
    fn settled(&self) {
        self.doubts.lock().unwrap().1 = None;
    }

    /// A "no" that was not believed, for the song `id`: tried again later, at once unless the last song had one too
    /// (with nothing else in between); given up on at its third. A "no" said a little while ago is not asked again
    /// (see `Lacks`), so trying again costs no allowance until the add-on is asked anew.
    fn later(&self, id: i64) -> Doubt {
        let mut doubts = self.doubts.lock().unwrap();
        let (counts, last) = &mut *doubts;
        let n = counts.entry(id).or_insert(0);
        *n += 1;
        if *n >= DOUBTS {
            counts.remove(&id);
            *last = None;
            return Doubt::GiveUp;
        }
        let run = last.is_some_and(|other| other != id);
        *last = Some(id);
        if run { Doubt::Wait } else { Doubt::Next }
    }

    /// Notes that the song was held. True when it has now been held [`STRIKES`] more times after other songs were
    /// saved in between: trouble that lets other songs through is the song's own, and it is given up on. Trouble
    /// that holds every song (an allowance spent, no connection) never gives one up.
    fn struck(&self, id: i64) -> bool {
        let saved = self.saved.load(Ordering::Relaxed);
        let mut strikes = self.strikes.lock().unwrap();
        let (at, struck) = strikes.entry(id).or_insert((saved, 0));
        if saved > *at {
            *struck += 1;
        }
        *at = saved;
        if *struck >= STRIKES {
            strikes.remove(&id);
            return true;
        }
        false
    }

    /// The listener asks to try again: every song starts with a clean slate.
    fn forgive(&self) {
        self.strikes.lock().unwrap().clear();
        *self.doubts.lock().unwrap() = (HashMap::new(), None);
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
    match outcome {
        Ok(()) => {
            state.saving.release();
            state.saving.saved_one(id);
        }
        Err(Trouble::Failed(why)) => {
            state.saving.settled();
            let _ = state.library.lock().unwrap().download_failed(id, &why);
        }
        // Held again after other songs were saved: the trouble is the song's own after all.
        Err(Trouble::Held(why, _)) if state.saving.struck(id) => {
            state.saving.settled();
            let _ = state.library.lock().unwrap().download_failed(id, &why);
        }
        // Not the song's own trouble: it waits its turn again at the back of the line, and saving waits before it
        // asks again (an add-on whose allowance is spent is not asked for every song on the list).
        Err(Trouble::Held(why, after)) => {
            state.saving.settled();
            let _ = state.library.lock().unwrap().download_held(id, now_ms());
            state.saving.hold(why, after);
        }
        // "No" from add-ons that are not finding other songs either: the song is tried again later.
        Err(Trouble::Later(why)) => match state.saving.later(id) {
            Doubt::GiveUp => {
                let _ = state.library.lock().unwrap().download_failed(id, &why);
            }
            Doubt::Next => {
                let _ = state.library.lock().unwrap().download_held(id, now_ms());
            }
            Doubt::Wait => {
                let _ = state.library.lock().unwrap().download_held(id, now_ms());
                state.saving.hold(why, None);
            }
        },
        // Stopped because Stash went offline: it waits for the connection, first on the list as it was.
        Err(Trouble::Cancelled) if crate::serve::offline(state) => {
            let _ = state.library.lock().unwrap().requeue_downloads();
        }
        _ => {}
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

/// What saving does when the add-ons did not give the song. Given up on: a song no add-on has (when that is believed),
/// and an add-on's answer about this song itself (a request it refused, a file it cannot give as asked). Waited out:
/// trouble that is not the song's own (an allowance spent, too many asks, a key refused, an add-on down or out of
/// reach), no add-on to ask, and the library not read. Tried again later: "no"s from add-ons that are not finding
/// other songs either.
fn trouble_of(failure: Failure) -> Trouble {
    match failure.code {
        Some(code) => {
            let passing = matches!(
                code,
                Code::QuotaExceeded
                    | Code::RateLimited
                    | Code::Unauthorized
                    | Code::Unavailable
                    | Code::Timeout
                    | Code::Network
            );
            if passing {
                Trouble::Held(failure.message, failure.retry_after)
            } else {
                Trouble::Failed(failure.message)
            }
        }
        None if failure.nothing_to_ask || !failure.miss => Trouble::Held(failure.message, None),
        None if failure.doubtful => Trouble::Later(failure.message),
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
            // The listener asks to try again: saving does not wait out trouble it had with an add-on.
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
            nothing_to_ask: false,
        }
    }

    #[test]
    fn trouble_that_is_not_the_songs_own_is_waited_out_and_its_own_gives_it_up() {
        let held = |t: Trouble| matches!(t, Trouble::Held(_, _));
        for code in [
            Code::QuotaExceeded,
            Code::RateLimited,
            Code::Unauthorized,
            Code::Unavailable,
            Code::Timeout,
            Code::Network,
        ] {
            assert!(
                held(trouble_of(failure(Some(code), false, false))),
                "{code:?}"
            );
        }
        // The add-on's answer about this song: a file it cannot give as asked, a request it refused.
        for code in [
            Code::InvalidResponse,
            Code::BadRequest,
            Code::Refused,
            Code::NotFound,
        ] {
            assert!(
                !held(trouble_of(failure(Some(code), false, false))),
                "{code:?}"
            );
        }
        // No add-on has it: believed, given up on; doubted, tried later; no add-on to ask, waited out.
        assert!(matches!(
            trouble_of(failure(None, true, false)),
            Trouble::Failed(_)
        ));
        assert!(matches!(
            trouble_of(failure(None, true, true)),
            Trouble::Later(_)
        ));
        assert!(held(trouble_of(Failure {
            nothing_to_ask: true,
            ..failure(None, true, false)
        })));
        // The add-on's "ask again in" goes along.
        assert!(matches!(
            trouble_of(failure(Some(Code::RateLimited), false, false)),
            Trouble::Held(_, Some(30))
        ));
    }

    #[test]
    fn a_song_held_again_after_others_were_saved_is_given_up_on() {
        let saving = Saving::default();
        // Trouble that holds every song (nothing saved in between) never gives one up.
        for _ in 0..10 {
            assert!(!saving.struck(1));
            assert!(!saving.struck(2));
        }
        // Song 1 is held, others are saved, it is held again: once is a strike, twice gives it up.
        saving.saved_one(3);
        assert!(!saving.struck(1));
        saving.saved_one(4);
        assert!(saving.struck(1));
        // Given up on, it starts over if it comes back; saved, its strikes go.
        assert!(!saving.struck(1));
        saving.saved_one(5);
        assert!(!saving.struck(2));
        saving.saved_one(2);
        saving.saved_one(6);
        assert!(!saving.struck(2));
        // "Try again" forgives every song.
        saving.saved_one(7);
        saving.forgive();
        assert!(!saving.struck(1));
    }

    #[test]
    fn a_no_that_is_not_believed_is_tried_later_and_two_in_a_row_wait() {
        let saving = Saving::default();
        // One song alone: tried again at once, given up on at its third.
        assert_eq!(saving.later(1), Doubt::Next);
        assert_eq!(saving.later(1), Doubt::Next);
        assert_eq!(saving.later(1), Doubt::GiveUp);
        // Two songs in a row: the add-ons may be broken, saving waits.
        assert_eq!(saving.later(2), Doubt::Next);
        assert_eq!(saving.later(3), Doubt::Wait);
        // Something else in between (a song saved, one failed, trouble) ends the run.
        saving.settled();
        assert_eq!(saving.later(4), Doubt::Next);
        saving.saved_one(9);
        assert_eq!(saving.later(2), Doubt::Next);
        assert_eq!(saving.later(2), Doubt::GiveUp);
        // "Try again" starts every song over: song 4, at its second, would be given up on at its next.
        assert_eq!(saving.later(4), Doubt::Next);
        saving.forgive();
        assert_eq!(saving.later(4), Doubt::Next);
        assert_eq!(saving.later(4), Doubt::Next);
        assert_eq!(saving.later(4), Doubt::GiveUp);
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
