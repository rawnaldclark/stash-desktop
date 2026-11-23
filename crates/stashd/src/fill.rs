//! Filling in missing tags (`stash_core::fill`): the files that say little of themselves are looked up in a public
//! music catalogue, and what was found is kept for the listener to look over; only what they accept is written.
//!
//! The lookup sends each song's title and artist to the catalogue, so it runs only when the listener starts it, and
//! it does so whatever the offline setting says: starting it is the listener asking for exactly that.
//!
//! - `GET /api/fill`: how many files have blanks, how far a lookup is, and what it has found so far.
//! - `POST /api/fill/start`: looks the files up, one every few seconds (the catalogue allows about twenty a minute).
//! - `POST /api/fill/stop`: stops looking; what was found stays.
//! - `POST /api/fill/apply {ids}`: writes what was found for those songs into their files.
//! - `POST /api/fill/drop`: forgets what was found.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use stash_core::edit::{EditReport, TagEdit};
use stash_core::fill::{CATALOGUE, look_up};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, refuse, reply_json};

/// Time between two questions to the catalogue.
const PACE: Duration = Duration::from_millis(3_200);
/// How long to wait before asking again after the catalogue could not be reached, once.
const AGAIN: Duration = Duration::from_secs(25);

/// What was found for one song: the fields that would be written.
#[derive(Clone, Serialize)]
struct Offer {
    id: i64,
    title: String,
    artist: String,
    set: TagEdit,
}

#[derive(Default)]
struct Job {
    running: bool,
    total: usize,
    done: usize,
    /// Songs the catalogue does not have, or has nothing new for.
    missed: usize,
    /// Why the lookup stopped by itself, when it did.
    problem: Option<String>,
    found: Vec<Offer>,
}

/// The lookup, and what it has found.
#[derive(Default)]
pub(crate) struct Fill {
    job: Mutex<Job>,
    /// The listener asked for a lookup; the thread takes it up.
    wanted: AtomicBool,
    /// The listener asked the lookup to stop.
    halt: AtomicBool,
}

fn catalogue() -> String {
    std::env::var("STASH_FILL_CATALOGUE").ok().filter(|a| !a.is_empty()).unwrap_or_else(|| CATALOGUE.to_string())
}

fn pace() -> Duration {
    std::env::var("STASH_FILL_PACE_MS").ok().and_then(|ms| ms.parse().ok()).map_or(PACE, Duration::from_millis)
}

/// The thread that looks songs up when asked to.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let fill = &state.fill;
    // Waits, and says whether to go on (neither Stash closing nor the listener stopping the lookup).
    let wait = |time: Duration, fill: &Fill| {
        let mut left = time;
        while !left.is_zero() {
            if stop.load(Ordering::Relaxed) || fill.halt.load(Ordering::Relaxed) {
                return false;
            }
            let step = left.min(Duration::from_millis(200));
            std::thread::sleep(step);
            left -= step;
        }
        !stop.load(Ordering::Relaxed) && !fill.halt.load(Ordering::Relaxed)
    };
    while !stop.load(Ordering::Relaxed) {
        if !fill.wanted.swap(false, Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(300));
            continue;
        }
        fill.halt.store(false, Ordering::Relaxed);
        let songs = state.library.lock().unwrap().files_with_blanks().unwrap_or_default();
        *fill.job.lock().unwrap() = Job { running: true, total: songs.len(), ..Default::default() };
        let (base, pace) = (catalogue(), pace());
        for (i, song) in songs.iter().enumerate() {
            if i > 0 && !wait(pace, fill) {
                break;
            }
            let found = match look_up(&base, song) {
                Ok(found) => found,
                // Once more after a while: the catalogue turns away a client that asks too fast.
                Err(_) if wait(AGAIN, fill) => match look_up(&base, song) {
                    Ok(found) => found,
                    Err(why) => {
                        fill.job.lock().unwrap().problem = Some(format!("The catalogue could not be reached ({}). What was found so far is kept.", why.0));
                        break;
                    }
                },
                Err(_) => break,
            };
            let set = found.and_then(|found| state.library.lock().unwrap().fill_edit(song.id, &found).ok().flatten());
            let mut job = fill.job.lock().unwrap();
            job.done += 1;
            match set {
                Some(set) => job.found.push(Offer { id: song.id, title: song.title.clone(), artist: song.artist.clone(), set }),
                None => job.missed += 1,
            }
        }
        fill.job.lock().unwrap().running = false;
    }
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let fill = &state.fill;
    match (request.method(), path) {
        (Method::Get, "/api/fill") => {
            let blanks = state.library.lock().unwrap().files_with_blanks()?.len();
            let job = fill.job.lock().unwrap();
            Ok(Some(reply_json(
                200,
                json!({
                    "blanks": blanks,
                    "running": job.running || fill.wanted.load(Ordering::Relaxed),
                    "total": job.total,
                    "done": job.done,
                    "missed": job.missed,
                    "problem": job.problem,
                    "found": job.found,
                }),
            )))
        }
        (Method::Post, "/api/fill/start") => {
            if !fill.job.lock().unwrap().running {
                fill.wanted.store(true, Ordering::Relaxed);
            }
            Ok(Some(reply_json(200, json!({ "ok": true }))))
        }
        (Method::Post, "/api/fill/stop") => {
            fill.halt.store(true, Ordering::Relaxed);
            Ok(Some(reply_json(200, json!({ "ok": true }))))
        }
        (Method::Post, "/api/fill/drop") => {
            let mut job = fill.job.lock().unwrap();
            if job.running {
                return Ok(Some(refuse(409, "Stop the lookup first")));
            }
            *job = Job::default();
            Ok(Some(reply_json(200, json!({ "ok": true }))))
        }
        (Method::Post, "/api/fill/apply") => {
            let body = body_json(request)?;
            let ids: Vec<i64> = body
                .get("ids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();
            if ids.is_empty() {
                return Ok(Some(refuse(400, "ids are the songs to write")));
            }
            // The offers are taken out first, so the library is never held together with the job.
            let offers: Vec<Offer> = {
                let mut job = fill.job.lock().unwrap();
                let (take, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut job.found).into_iter().partition(|o| ids.contains(&o.id));
                job.found = keep;
                take
            };
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            let mut report = EditReport::default();
            for offer in &offers {
                // One at a time, the library let go between: a long run must not hold up the window.
                let one = state.library.lock().unwrap().edit_tags(&[offer.id], &offer.set, now)?;
                report.written += one.written;
                report.skipped += one.skipped;
                report.failed.extend(one.failed);
            }
            if report.written > 0 {
                state.edited();
            }
            Ok(Some(reply_json(200, json!(report))))
        }
        _ => Ok(None),
    }
}
