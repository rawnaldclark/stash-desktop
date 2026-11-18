//! Artists the listener follows, and their new records: an artist's YouTube Music page is read from time to time, and
//! an album or a single on it that was not there before is new (Home's "New releases").
//!
//! `POST /api/artists/follow {source, id, name, art?}` follows the artist `id` ("UC…"), and their page is read at once,
//! so this year's records show straight away; `POST /api/artists/unfollow {source, id}` stops, and their records go.
//! Both answer the artists followed, and the count of changes. `GET /api/artists/followed` answers the artists followed,
//! `GET /api/releases` their records found in the last [`NEW_FOR`]. A thread reads each followed artist's page again
//! once it is [`STALE`], a few at each look, while YouTube Music is on and Stash is online.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::addons::{AddonError, Code};
use stash_core::lists::Release;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, offline, refuse, reply_json};

/// How old what is known of a followed artist's page may be before the thread reads it again.
const STALE: Duration = Duration::from_secs(12 * 3600);
/// How often the thread looks; and how long after the core starts it first looks.
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);
const FIRST_LOOK: Duration = Duration::from_secs(120);
/// How many artists' pages one look reads at most, and the pause between two: YouTube Music is not hurried.
const PER_LOOK: usize = 10;
const BETWEEN: Duration = Duration::from_secs(3);
/// How long a record found is new.
const NEW_FOR: Duration = Duration::from_secs(60 * 24 * 3600);
/// How many new records the window is given.
const SHOWN: usize = 30;
/// The one source whose artists can be followed for now.
const SOURCE: &str = stash_core::youtube::ID;

/// Why an artist's page was not read.
enum Trouble {
    Off,
    Offline,
    YouTube(AddonError),
    Core(anyhow::Error),
}

impl From<stash_core::Error> for Trouble {
    fn from(e: stash_core::Error) -> Self {
        Trouble::Core(e.into())
    }
}

/// Reads the page of the followed artist `id` and keeps the records on it. Answers how many are new.
fn check(state: &State, id: &str) -> Result<usize, Trouble> {
    let Some(youtube) = state.youtube_on() else {
        return Err(Trouble::Off);
    };
    if offline(state) {
        return Err(Trouble::Offline);
    }
    let page = youtube.artist(id).map_err(Trouble::YouTube)?;
    let releases: Vec<Release> = page
        .albums
        .iter()
        .chain(&page.singles)
        .map(|record| Release {
            id: record.id.clone(),
            title: record.title.clone(),
            single: record.single,
            year: record.year,
            art: record.art.clone(),
        })
        .collect();
    let now = now_ms();
    Ok(state.library.lock().unwrap().note_releases(
        SOURCE,
        id,
        &releases,
        now,
        stash_core::lists::year_at(now),
    )?)
}

/// The artists followed, as every change to them answers, with the count of changes.
fn answer_followed(state: &State) -> Result<Reply> {
    let followed = state.library.lock().unwrap().followed_artists()?;
    state.edited();
    Ok(reply_json(
        200,
        json!({
            "followed": followed,
            "edits": state.edits.load(Ordering::Relaxed),
        }),
    ))
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if *request.method() == Method::Get {
        return Ok(match path {
            "/api/artists/followed" => {
                let rows = state.library.lock().unwrap().followed_artists()?;
                Some(reply_json(200, json!({ "rows": rows })))
            }
            "/api/releases" => {
                let since = now_ms() - NEW_FOR.as_millis() as i64;
                let rows = state.library.lock().unwrap().new_releases(since, SHOWN)?;
                Some(reply_json(200, json!({ "rows": rows })))
            }
            _ => None,
        });
    }
    let follow = match path {
        "/api/artists/follow" => true,
        "/api/artists/unfollow" => false,
        _ => return Ok(None),
    };
    if *request.method() != Method::Post {
        return Ok(None);
    }
    let body = body_json(request)?;
    let text = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    if text("source") != SOURCE {
        return Ok(Some(refuse(
            409,
            "only YouTube Music artists can be followed for now",
        )));
    }
    let id = text("id");
    if id.is_empty() || id.len() > 200 {
        return Ok(Some(refuse(400, "that is not an artist")));
    }
    if !follow {
        state.library.lock().unwrap().unfollow_artist(SOURCE, &id)?;
        return answer_followed(state).map(Some);
    }
    let name = text("name");
    if name.is_empty() || name.len() > 500 {
        return Ok(Some(refuse(400, "an artist needs a name")));
    }
    let art = body
        .get("art")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|art| art.starts_with("https://") && art.len() <= 2000);
    state
        .library
        .lock()
        .unwrap()
        .follow_artist(SOURCE, &id, &name, art, now_ms())?;
    // Their page is read now, on the thread that reads them, so their records of this year are on Home at once.
    state.artist_followed.store(true, Ordering::Relaxed);
    answer_followed(state).map(Some)
}

/// Reads the pages of followed artists not read for a while (one just followed first), a few at each look, while
/// YouTube Music is on and Stash is online.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    // Waits `for_`, or less when `or_followed` and an artist is followed meanwhile. False when the core stops.
    let wait = |for_: Duration, or_followed: bool| {
        let until = Instant::now() + for_;
        while Instant::now() < until {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            if or_followed && state.artist_followed.swap(false, Ordering::Relaxed) {
                return true;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        true
    };
    if !wait(FIRST_LOOK, true) {
        return;
    }
    loop {
        let due: Vec<String> = if state.youtube_on().is_none() || offline(state) {
            Vec::new()
        } else {
            let older_than = now_ms() - STALE.as_millis() as i64;
            state
                .library
                .lock()
                .unwrap()
                .artists_to_check(older_than)
                .unwrap_or_default()
                .into_iter()
                .filter(|(source, _)| source == SOURCE)
                .map(|(_, id)| id)
                .take(PER_LOOK)
                .collect()
        };
        let mut found = false;
        for (n, id) in due.iter().enumerate() {
            if n > 0 && !wait(BETWEEN, false) {
                return;
            }
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match check(state, id) {
                Ok(new) => found |= new > 0,
                // YouTube Music asked to be left alone: the others wait for a later look.
                Err(Trouble::YouTube(e)) if matches!(e.code, Code::RateLimited) => break,
                // Read again on a later look.
                Err(Trouble::YouTube(e)) => {
                    eprintln!("a followed artist's page was not read: {}", e.in_words())
                }
                Err(Trouble::Core(e)) => eprintln!("a followed artist's page was not read: {e:#}"),
                Err(_) => break,
            }
        }
        // The window reads the new records again when the count of changes goes up.
        if found {
            state.edited();
        }
        if !wait(LOOK_EVERY, true) {
            return;
        }
    }
}
