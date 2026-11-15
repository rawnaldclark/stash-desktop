//! Saving songs as files: the list of songs to save, worked through one at a time on a thread of its own, and the
//! routes the window uses for it.
//!
//! A song of the library is found through the listener's add-ons as it is for playing (its known add-on first, then
//! each add-on in the listener's order), and the add-on is asked for a file to keep (contract section 4.5). The
//! file goes under the downloads folder; its row in the library becomes that file, keeping its likes, playlists and
//! plays. Taking it off the list again deletes the file and the song goes back to playing through the add-ons.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::addons::StreamRequest;
use stash_core::downloads::{self, Song, Trouble};
use stash_core::lists::SavedDownload;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json, through_addons};

/// The setting that names the downloads folder.
const FOLDER: &str = "downloads.folder";
/// How often the one coming in checks it is still wanted.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// The song coming in, how far, and a bell to ring when there is something new on the list.
#[derive(Default)]
pub(crate) struct Saving {
    now: Mutex<Option<Progress>>,
    bell: (Mutex<bool>, Condvar),
}

#[derive(Clone, Copy, serde::Serialize)]
struct Progress {
    id: i64,
    bytes: u64,
    of: Option<u64>,
}

impl Saving {
    /// Something was put on the list: the worker looks at once.
    pub fn ring(&self) {
        *self.bell.0.lock().unwrap() = true;
        self.bell.1.notify_all();
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
    while !stop.load(Ordering::Relaxed) {
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
    if let Err(Trouble::Failed(why)) = outcome {
        let _ = state.library.lock().unwrap().download_failed(id, &why);
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
    let (info, addon) = through_addons(
        &state.addons,
        &state.library,
        &state.lacks,
        id,
        "there is no add-on to save it from",
        |addon| addon.offers_download(),
        |addon, _, track| addon.download(track, &StreamRequest::default()),
    )
    .map_err(|failure| Trouble::Failed(failure.message))?;

    // It comes in under a name of its own at the top of the folder, and is moved where it belongs once it is whole
    // and its kind is known.
    std::fs::create_dir_all(&root)?;
    let part = root.join(format!(".stash-{id}.part"));
    let (looked, wanted) = (Cell::new(Instant::now()), Cell::new(true));
    let fetched = downloads::fetch(
        &info.url,
        &part,
        &|| !wanted.get() || stop.load(Ordering::Relaxed),
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
    let (head, _) = match fetched {
        Ok(done) => done,
        Err(trouble) => {
            let _ = std::fs::remove_file(&part);
            return Err(trouble);
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
            tier: Some(info.tier.as_str().to_string()),
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
        Ok(true) => Ok(()),
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
