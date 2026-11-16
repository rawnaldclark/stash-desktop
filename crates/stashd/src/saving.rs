//! Saving songs as files: the list of songs to save, worked through one at a time on a thread of its own, and the
//! routes the window uses for it.
//!
//! A song of the library is found through the listener's add-ons as it is for playing (its known add-on first, then
//! each add-on in the listener's order), and the add-on is asked for a file to keep (contract section 4.5). The
//! file goes under the downloads folder; its row in the library becomes that file, keeping its likes, playlists and
//! plays. Taking it off the list again deletes the file and the song goes back to playing through the add-ons.

use std::cell::Cell;
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
use stash_core::lists::{SavedDownload, UnsureTries};
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
    /// The song tried last, while it had trouble that may be its own and nothing came after it (a song saved, a song
    /// failed for its own reasons, trouble that is everyone's): the next song with such trouble makes a run (see
    /// [`Saving::unsure`]).
    last: Mutex<Option<(i64, Unsure)>>,
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

/// What saving does with a song that had trouble that may be its own: gives it up or puts it at the back of the line,
/// and either way asks the next song at once or waits first.
#[derive(Debug, PartialEq, Eq)]
struct Step {
    give_up: bool,
    wait: bool,
    /// The trouble came again (the song before had it too, or this one is the only one left): saving waits longer
    /// than the time before, however little the add-on asked for.
    again: bool,
}

/// A song is given up on at its third strike: a try with trouble that may be its own, after another song was saved
/// since its try before (see `Library::download_unsure`).
const STRIKES: u32 = 3;
/// A song that gets no strikes (it is the only one left, or no song is saved) is given up on once it has had such
/// trouble for a day and been asked at least [`STRIKES`] times in it. It is not asked often meanwhile: each time such
/// trouble comes again, saving waits twice as long as the time before, up to an hour.
const UNSURE_FOR_MS: i64 = 24 * 60 * 60 * 1000;

/// Saving waits: after trouble that is everyone's (the add-on's allowance is spent or its key refused, too many asks,
/// it cannot be reached, no add-on can be asked, the downloads folder cannot be written), when two songs in a row had
/// trouble that may be their own, when the one song left had it again, or when the add-on asked to be left alone. Nothing is asked until `until`; then the song first in line is tried (the
/// one held went to the back). Each time it is held again it waits twice as long, up to an hour, or as long as the
/// add-on asked; a song an add-on gave ends it, and so do the listener's "Try again", an add-on added or switched on,
/// and a new downloads folder. Going back online ends a wait for add-ons out of reach.
struct Held {
    why: String,
    until: Instant,
    until_ms: i64,
    wait: Duration,
    /// It waits because no add-on could be reached.
    away: bool,
}

/// Why saving waits, which says for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// Trouble that is everyone's, or trouble that may be the song's own when the add-on asked to be left alone: as
    /// long as the add-on asked, else a minute, then twice as long each time, up to an hour.
    AsAsked,
    /// No add-on could be reached: a minute, then twice as long each time, up to a few minutes.
    Away,
    /// Trouble that may be the songs' own came again: twice as long as the time before (at least as long as the add-on
    /// asked), up to an hour, so a song is not asked often however little the add-on asks to wait.
    Again,
}

/// How long saving first waits after trouble with an add-on, and the longest it waits.
const HELD_FIRST: Duration = Duration::from_secs(60);
const HELD_MOST: Duration = Duration::from_secs(3600);
/// The longest saving waits while the add-on cannot be reached: nothing is spent asking, and saving goes on soon after
/// the connection is back.
const HELD_AWAY_MOST: Duration = Duration::from_secs(300);
/// The shortest wait an add-on's own "ask again in" is taken for.
const HELD_LEAST: Duration = Duration::from_secs(5);
/// Why saving waits after "no"s that were not believed.
const NOT_FINDING: &str = "your add-ons are not finding songs just now";
/// Why saving waits when no add-on could be reached.
const UNREACHED: &str = "no add-on can be reached (is this computer online?)";

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
        self.hold_as(why, after, Hold::AsAsked);
    }

    /// Saving waits while no add-on can be reached: a minute, then twice as long, up to a few minutes.
    fn hold_away(&self, why: String) {
        self.hold_as(why, None, Hold::Away);
    }

    fn hold_as(&self, why: String, after: Option<u32>, how: Hold) {
        let mut held = self.held.lock().unwrap();
        let most = if how == Hold::Away {
            HELD_AWAY_MOST
        } else {
            HELD_MOST
        };
        let step = held.as_ref().map_or(HELD_FIRST, |h| (h.wait * 2).min(most));
        let asked = after.map(|s| Duration::from_secs(u64::from(s)).clamp(HELD_LEAST, HELD_MOST));
        let wait = match (how, asked) {
            (Hold::Again, Some(asked)) => asked.max(step),
            (_, Some(asked)) => asked,
            (_, None) => step,
        };
        *held = Some(Held {
            why,
            until: Instant::now() + wait,
            until_ms: now_ms() + wait.as_millis() as i64,
            wait,
            away: how == Hold::Away,
        });
    }

    /// The trouble is over (an add-on gave a song, the listener asked to try again, an add-on was added or switched
    /// on, the downloads folder changed).
    pub(crate) fn release(&self) {
        *self.held.lock().unwrap() = None;
    }

    /// The connection is back (Stash was offline): a wait for add-ons out of reach is over. Other waits are not (an
    /// allowance spent is spent whatever the connection).
    pub(crate) fn release_away(&self) {
        let mut held = self.held.lock().unwrap();
        if held.as_ref().is_some_and(|h| h.away) {
            *held = None;
        }
    }

    /// An add-on gave a song: a run of trouble ends (its tries start over in the library).
    fn saved_one(&self) {
        *self.last.lock().unwrap() = None;
    }

    /// Something else came of a song than trouble that may be its own (it failed for its own reasons, or the trouble
    /// was everyone's): a run of such trouble ends.
    fn settled(&self) {
        *self.last.lock().unwrap() = None;
    }

    /// Trouble that may be the song's own (`kind`), for the song `id`, which has had `tries` of it so far (kept in the
    /// library across starts). Trouble that is everyone's never comes here (see [`Trouble::Held`]), so a spent
    /// allowance gives no song up.
    ///
    /// A try that asked something (`asked`) gives the song up at its third strike (a try after another song was saved
    /// since its try before: the add-on gives other songs, so the trouble is likely this one's), at once for a refusal
    /// the song before did not have, or when nothing has told whose trouble it is for a long while (it has had such
    /// trouble for a day, and been asked three times in it), so no song is asked for ever.
    ///
    /// Saving waits before it asks the next song when the song before had such trouble too (likely everyone's: the
    /// add-on or the line is down, its lookup is broken, it refuses every request), when the same song comes again (it
    /// is the only one left), or when the add-on asked to be left alone a while (`asked_to_wait`), whether or not the
    /// song was given up on: nothing is asked over and over, and a run never ends in a burst of asks. When the trouble
    /// came again, the wait is longer than the one before (see [`Hold::Again`]).
    fn unsure(
        &self,
        id: i64,
        kind: Unsure,
        tries: UnsureTries,
        asked: bool,
        asked_to_wait: bool,
        now_ms: i64,
    ) -> Step {
        let mut last = self.last.lock().unwrap();
        let run = matches!(*last, Some((other, _)) if other != id);
        let again = matches!(*last, Some((other, _)) if other == id);
        let first_refusal = match kind {
            Unsure::Refused(code) => {
                !matches!(*last, Some((other, Unsure::Refused(c))) if other != id && c == code)
            }
            _ => false,
        };
        *last = Some((id, kind));
        let long_unsure = tries.asks >= STRIKES && now_ms - tries.since_ms >= UNSURE_FOR_MS;
        Step {
            give_up: asked && (first_refusal || tries.strikes >= STRIKES || long_unsure),
            wait: run || again || asked_to_wait,
            again: run || again,
        }
    }

    /// The listener asks to try again: no run is under way (every song's tries start over in the library).
    fn forgive(&self) {
        self.settled();
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
    let unsure = |kind: Unsure, why: String, waits: String, after: Option<u32>, asked: bool| {
        let now = now_ms();
        let tries = state
            .library
            .lock()
            .unwrap()
            .download_unsure(id, asked, now)
            .unwrap_or_default();
        let step = state
            .saving
            .unsure(id, kind, tries, asked, after.is_some(), now);
        if step.give_up {
            failed(&why);
        } else {
            back();
        }
        if step.wait {
            let how = if step.again {
                Hold::Again
            } else {
                Hold::AsAsked
            };
            state.saving.hold_as(waits, after, how);
        }
    };
    match outcome {
        // An add-on gave it: whatever trouble came before is over.
        Ok(true) => {
            state.saving.release();
            state.saving.saved_one();
        }
        // From a copy kept from playing it: no add-on was asked, so it says nothing of them.
        Ok(false) => {}
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
        // No add-on could be reached: everyone's too, and nothing was asked; saving waits only a little.
        Err(Trouble::Away(_)) => {
            state.saving.settled();
            back();
            state.saving.hold_away(UNREACHED.into());
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

/// Brings the song in. Answers whether an add-on gave it (else it came from a copy kept from playing it).
fn fetch_and_keep(state: &State, id: i64, stop: &AtomicBool) -> Result<bool, Trouble> {
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
    let (head, addon, tier, fetched) = if let Some((kept, copy)) = copy {
        let copied = std::fs::copy(&kept, &part).and_then(|_| {
            let mut head = vec![0u8; 64];
            let n = std::fs::File::open(&part)?.read(&mut head)?;
            head.truncate(n);
            Ok(head)
        });
        match copied {
            Ok(head) if downloads::kind_of(&head).is_some() => {
                (head, copy.addon.unwrap_or_default(), copy.tier, false)
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
            Ok((head, _)) => (head, addon, Some(info.tier.as_str().to_string()), true),
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
            fetched,
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
            Ok(fetched)
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
/// key refused, no add-on that could be reached, no add-on to ask, the library not read. Maybe the song's own: an
/// add-on not working or too slow, or one out of reach while another said it does not have the song; "no"s from
/// add-ons that are not finding other songs either; a request the add-on refused, or answered with what the contract
/// does not allow. The song's own: no add-on has it (when that is believed).
fn trouble_of(failure: Failure) -> Trouble {
    match failure.code {
        Some(Code::QuotaExceeded | Code::RateLimited | Code::Unauthorized) => {
            Trouble::Held(failure.message, failure.retry_after)
        }
        // No add-on answered at all: no connection, or their servers down. The same for every song, and nothing was
        // spent.
        Some(Code::Network) if !failure.answered => Trouble::Away(failure.message),
        // One could not be reached while another answered (the line is up): maybe that add-on has the song.
        Some(Code::Network | Code::Unavailable | Code::Timeout) => {
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
        // Everyone's, and nothing asked: out of reach. But not while another add-on answered (it does not have the
        // song): the line is up, and the one out of reach may have it.
        assert!(matches!(
            trouble_of(failure(Some(Code::Network), false, false)),
            Trouble::Away(_)
        ));
        assert!(matches!(
            trouble_of(Failure {
                answered: true,
                ..failure(Some(Code::Network), false, false)
            }),
            Trouble::Busy(_, Some(30))
        ));
        // Either: an add-on down or too slow...
        for code in [Code::Unavailable, Code::Timeout] {
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

    /// What a song has had of trouble that may be its own.
    fn had(strikes: u32, asks: u32, since_ms: i64) -> UnsureTries {
        UnsureTries {
            strikes,
            asks,
            since_ms,
        }
    }
    const NOW: i64 = 10 * UNSURE_FOR_MS;
    const FRESH: UnsureTries = UnsureTries {
        strikes: 0,
        asks: 1,
        since_ms: NOW,
    };
    /// What becomes of a song (with no "ask again in" from the add-on: a wait is for trouble that came again).
    fn step(give_up: bool, wait: bool) -> Step {
        Step {
            give_up,
            wait,
            again: wait,
        }
    }

    #[test]
    fn a_song_is_given_up_on_at_its_third_strike_or_when_nothing_tells_for_long() {
        let saving = Saving::default();
        let busy = |id, tries| saving.unsure(id, Unsure::Busy, tries, true, false, NOW);
        // One song alone: tried again at once, then after waits; given up on at its third strike.
        assert_eq!(busy(1, FRESH), step(false, false));
        assert_eq!(busy(1, had(0, 2, NOW)), step(false, true));
        assert_eq!(busy(1, had(2, 4, NOW)), step(false, true));
        assert_eq!(busy(1, had(3, 5, NOW)), step(true, true));
        // With no strikes: once it has had such trouble for a day and been asked three times in it...
        assert!(!busy(2, had(0, 2, NOW - UNSURE_FOR_MS)).give_up);
        assert!(!busy(2, had(0, 3, NOW - UNSURE_FOR_MS + 1)).give_up);
        assert!(busy(2, had(0, 3, NOW - UNSURE_FOR_MS)).give_up);
        // However often it was asked in that day (saving waits longer each time, so that is not often).
        assert!(!busy(3, had(0, 40, NOW - UNSURE_FOR_MS + 1)).give_up);
        // A try that asked nothing (only "no"s remembered from before) gives no song up.
        assert!(
            !saving
                .unsure(4, Unsure::Doubt, had(5, 50, 0), false, false, NOW)
                .give_up
        );
    }

    #[test]
    fn saving_waits_after_such_trouble_twice_in_a_row_and_never_asks_in_a_burst() {
        let saving = Saving::default();
        let unsure = |id, kind, tries| saving.unsure(id, kind, tries, true, false, NOW);
        // The first song: the next is asked at once. The next with any such trouble (likely everyone's): saving waits.
        assert_eq!(unsure(1, Unsure::Busy, FRESH), step(false, false));
        assert_eq!(unsure(2, Unsure::Doubt, FRESH), step(false, true));
        // A song given up on in a run: saving still waits before the next, so a run never ends in a burst of asks.
        assert_eq!(unsure(3, Unsure::Busy, had(0, 3, 0)), step(true, true));
        assert_eq!(unsure(4, Unsure::Busy, had(0, 3, 0)), step(true, true));
        // A song saved, or one failed for its own reasons or held for everyone's trouble, ends the run.
        saving.saved_one();
        assert_eq!(unsure(5, Unsure::Busy, FRESH), step(false, false));
        saving.settled();
        assert_eq!(unsure(6, Unsure::Busy, FRESH), step(false, false));
        // The add-on's "ask again in" is waited for, even after the first song (as asked: the trouble did not come
        // again).
        saving.settled();
        let asked = saving.unsure(7, Unsure::Busy, FRESH, true, true, NOW);
        assert!(asked.wait && !asked.again, "{asked:?}");
        // "Try again": no run under way.
        saving.forgive();
        assert_eq!(unsure(8, Unsure::Busy, FRESH), step(false, false));
    }

    #[test]
    fn a_refusal_is_the_songs_own_unless_the_song_before_had_it_too() {
        let saving = Saving::default();
        let refused = Unsure::Refused(Code::BadRequest);
        let unsure = |id, kind| saving.unsure(id, kind, FRESH, true, false, NOW);
        // The first song refused is given up on at once.
        assert_eq!(unsure(1, refused), step(true, false));
        // The next refused the same way: the add-on refuses every request (a plan that lapsed). The song stays in
        // line, and saving waits.
        assert_eq!(unsure(2, refused), step(false, true));
        assert_eq!(unsure(3, refused), step(false, true));
        // Another refusal is not the same: the song's own (saving still waits, as the song before had trouble).
        assert_eq!(
            unsure(4, Unsure::Refused(Code::InvalidResponse)),
            step(true, true)
        );
        // After something else, the song's own again.
        saving.settled();
        assert_eq!(unsure(5, refused), step(true, false));
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

    #[test]
    fn trouble_that_comes_again_waits_longer_each_time_however_little_the_add_on_asks() {
        let saving = Saving::default();
        let waits = || saving.held.lock().unwrap().as_ref().unwrap().wait;
        // The first time, as asked; each time after, twice as long as the time before.
        saving.hold_as("busy".into(), Some(5), Hold::AsAsked);
        assert_eq!(waits(), Duration::from_secs(5));
        saving.hold_as("busy".into(), Some(5), Hold::Again);
        assert_eq!(waits(), Duration::from_secs(10));
        saving.hold_as("busy".into(), Some(5), Hold::Again);
        assert_eq!(waits(), Duration::from_secs(20));
        // Never less than asked, and never more than an hour.
        saving.hold_as("busy".into(), Some(600), Hold::Again);
        assert_eq!(waits(), Duration::from_secs(600));
        for _ in 0..10 {
            saving.hold_as("busy".into(), Some(5), Hold::Again);
        }
        assert_eq!(waits(), HELD_MOST);
    }

    #[test]
    fn going_back_online_ends_only_a_wait_for_add_ons_out_of_reach() {
        let saving = Saving::default();
        saving.hold("spent".into(), Some(600));
        saving.release_away();
        assert!(
            saving.held_for().is_some(),
            "an allowance spent is still spent"
        );
        saving.hold_away("unreachable".into());
        assert!(saving.held_for().unwrap() <= HELD_AWAY_MOST);
        saving.release_away();
        assert!(saving.held_for().is_none());
    }
}
