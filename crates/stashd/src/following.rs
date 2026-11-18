//! Playlists that follow a YouTube Music playlist: its songs are that list's, kept in the library and read again
//! from time to time, so a list that changes (a chart, an editor's playlist, a friend's) changes here too.
//!
//! `POST /api/youtube/follow {id}` follows the list `id` ("VL…"), or reads it again when a playlist follows it already;
//! the answer carries the library's lists, like any change to them. A thread reads each followed list again once it
//! is [`STALE`], while YouTube Music is on and Stash is online. The playlist is recorded in the library's
//! `playlists.source_list` ("youtube:VL…"), not as a shared mix: other Stashes see a plain playlist.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::addons::AddonError;
use stash_core::import::Song;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, offline, refuse, reply_json};

/// How old a followed list's songs may be before the thread reads it again.
const STALE: Duration = Duration::from_secs(6 * 3600);
/// How often the thread looks; and how long after the core starts it first looks.
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);
const FIRST_LOOK: Duration = Duration::from_secs(90);
/// The prefix of a YouTube Music list in `playlists.source_list`.
const YOUTUBE: &str = "youtube:";

/// Why a list could not be followed, as the window is told.
enum Trouble {
    Off,
    Offline,
    YouTube(AddonError),
    Core(anyhow::Error),
}

impl From<anyhow::Error> for Trouble {
    fn from(e: anyhow::Error) -> Self {
        Trouble::Core(e)
    }
}

impl From<stash_core::Error> for Trouble {
    fn from(e: stash_core::Error) -> Self {
        Trouble::Core(e.into())
    }
}

/// Reads the YouTube Music list `id` and makes the playlist that follows it (or brings it up to date). Answers the
/// playlist's id and whether its songs changed.
fn follow(state: &State, id: &str) -> Result<(i64, bool), Trouble> {
    let Some(youtube) = state.youtube_on() else {
        return Err(Trouble::Off);
    };
    if offline(state) {
        return Err(Trouble::Offline);
    }
    let page = youtube.playlist(id).map_err(Trouble::YouTube)?;
    let now = now_ms();
    let songs: Vec<Song> = page
        .songs
        .iter()
        .filter(|s| !s.artist.is_empty() && !s.title.is_empty())
        .map(|s| Song {
            title: s.title.clone(),
            artist: s.artist.clone(),
            album: s.album.clone(),
            duration_ms: s.duration_ms,
            refs: vec![(stash_core::youtube::ID.to_string(), s.id.clone())],
            art_url: s.art.clone(),
            added_at_ms: Some(now),
            ..Song::default()
        })
        .collect();
    let mut library = state.library.lock().unwrap();
    let rows = library.adopt_songs(&songs)?;
    let name = if page.title.trim().is_empty() {
        "YouTube Music playlist"
    } else {
        page.title.as_str()
    };
    Ok(library.follow_list(&format!("{YOUTUBE}{id}"), name, &rows, now)?)
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if path != "/api/youtube/follow" || *request.method() != Method::Post {
        return Ok(None);
    }
    let body = body_json(request)?;
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if !id.starts_with("VL") || id.len() > 200 {
        return Ok(Some(refuse(400, "that is not a YouTube Music playlist")));
    }
    let (playlist, changed) = match follow(state, &id) {
        Ok(done) => done,
        Err(Trouble::Off) => return Ok(Some(refuse(404, "YouTube Music is switched off"))),
        Err(Trouble::Offline) => {
            return Ok(Some(refuse(
                409,
                "Stash is offline: YouTube Music is not asked",
            )));
        }
        Err(Trouble::YouTube(e)) => {
            let status = match e.code {
                stash_core::addons::Code::NotFound => 404,
                stash_core::addons::Code::RateLimited => 429,
                _ => 502,
            };
            return Ok(Some(refuse(status, &e.in_words())));
        }
        Err(Trouble::Core(e)) => return Err(e),
    };
    let library = state.library.lock().unwrap();
    let answer = json!({
        "id": playlist,
        "changed": changed,
        "playlists": library.playlists()?,
        "folders": library.playlist_folders()?,
        "counts": library.counts()?,
    });
    drop(library);
    state.edited();
    let mut answer = answer;
    answer["edits"] = json!(state.edits.load(Ordering::Relaxed));
    Ok(Some(reply_json(200, answer)))
}

/// Reads each followed list again once it is stale, while YouTube Music is on and Stash is online.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let wait = |for_: Duration| {
        let until = Instant::now() + for_;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        true
    };
    if !wait(FIRST_LOOK) {
        return;
    }
    loop {
        let due: Vec<String> = if state.youtube_on().is_none() || offline(state) {
            Vec::new()
        } else {
            let now = now_ms();
            state
                .library
                .lock()
                .unwrap()
                .followed_lists()
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, _, checked)| now - checked >= STALE.as_millis() as i64)
                .filter_map(|(_, list, _)| list.strip_prefix(YOUTUBE).map(str::to_string))
                .collect()
        };
        let mut changed = false;
        for id in due {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match follow(state, &id) {
                Ok((_, true)) => changed = true,
                Ok(_) => {}
                // Read again on a later look.
                Err(Trouble::YouTube(e)) => {
                    eprintln!("a followed playlist was not read again: {}", e.in_words())
                }
                Err(Trouble::Core(e)) => eprintln!("a followed playlist was not read again: {e:#}"),
                Err(_) => break,
            }
        }
        // The window reads the lists again when the count of changes goes up.
        if changed {
            state.edited();
        }
        if !wait(LOOK_EVERY) {
            return;
        }
    }
}
