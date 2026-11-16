//! Scrobbling: the plays that count go to the listener's services (ListenBrainz, or a server that speaks its
//! language), and each is told the song playing now. Plays wait in the library (`stash_core::scrobble`) until a service
//! takes them, so with no connection they go out when there is one. A service that is down or busy is left a while
//! (longer each time, up to half an hour, or as long as it asked); one that refused the token is not asked again until
//! the token is given again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::library::Library;
use stash_core::listenbrainz::{self, Sent};
use stash_core::player::{OnPlay, QueueItem};
use stash_core::scrobble::Waiting;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

/// How long a service that could not take plays is left at first; it doubles each time, up to `LONGEST_REST`.
const FIRST_REST: Duration = Duration::from_secs(30);
const LONGEST_REST: Duration = Duration::from_secs(30 * 60);
/// How often what waits is looked at when nothing rings.
const LOOK_EVERY: Duration = Duration::from_secs(60);
/// The longest token taken.
const TOKEN_CAP: usize = 500;

#[derive(Default)]
pub(crate) struct Scrobbling {
    bell: Mutex<Bell>,
    rung: Condvar,
}

#[derive(Default)]
struct Bell {
    /// Plays wait to be sent now.
    rung: bool,
    /// The song that began playing, to be told to each service.
    playing: Option<Value>,
    /// Services left a while: until when, and how many tries in a row failed.
    resting: HashMap<i64, (Instant, u32)>,
}

impl Scrobbling {
    /// Plays wait: the sender looks at once. `again`: a service to ask again now, though it was left a while.
    pub(crate) fn ring(&self, again: Option<i64>) {
        let mut bell = self.bell.lock().unwrap();
        if let Some(id) = again {
            bell.resting.remove(&id);
        }
        bell.rung = true;
        self.rung.notify_all();
    }

    fn tell_playing(&self, song: Value) {
        self.bell.lock().unwrap().playing = Some(song);
        self.rung.notify_all();
    }

    fn resting(&self, target: i64) -> bool {
        self.bell
            .lock()
            .unwrap()
            .resting
            .get(&target)
            .is_some_and(|(until, _)| Instant::now() < *until)
    }

    /// Service `target` could not take plays: it is left a while, longer each time, or as long as it asked.
    fn rest(&self, target: i64, asked: Option<Duration>) {
        let mut bell = self.bell.lock().unwrap();
        let failures = bell.resting.get(&target).map_or(0, |r| r.1) + 1;
        let grown = FIRST_REST
            .saturating_mul(1 << (failures - 1).min(6))
            .min(LONGEST_REST);
        let wait = asked.unwrap_or(grown).max(Duration::from_secs(1));
        bell.resting
            .insert(target, (Instant::now() + wait, failures));
    }

    fn rested(&self, target: i64) {
        self.bell.lock().unwrap().resting.remove(&target);
    }
}

/// What the player tells when a play counts: the play waits for each service that is on.
pub(crate) fn on_play(library: Arc<Mutex<Library>>, scrobbling: Arc<Scrobbling>) -> OnPlay {
    Arc::new(move |item: &QueueItem, started_at_ms: i64| {
        let song = song_of(&library, item);
        if !listenbrainz::sendable(&song) {
            return;
        }
        let queued = library
            .lock()
            .unwrap()
            .queue_scrobble(started_at_ms.div_euclid(1000), &song)
            .unwrap_or(0);
        if queued > 0 {
            scrobbling.ring(None);
        }
    })
}

/// What the player tells when a song begins to be heard: each service shows it as playing now.
pub(crate) fn on_start(library: Arc<Mutex<Library>>, scrobbling: Arc<Scrobbling>) -> OnPlay {
    Arc::new(move |item: &QueueItem, _: i64| {
        let song = song_of(&library, item);
        if listenbrainz::sendable(&song) {
            scrobbling.tell_playing(song);
        }
    })
}

/// The song as the services take it: the queue's title, artist, album and length, and what the library knows besides.
fn song_of(library: &Mutex<Library>, item: &QueueItem) -> Value {
    let facts = item
        .id
        .and_then(|id| library.lock().unwrap().scrobble_facts(id).ok())
        .unwrap_or_default();
    // "Unknown artist" names no one: a file without tags is not sent.
    let artist = if item.artist == stash_core::scan::UNKNOWN_ARTIST {
        ""
    } else {
        item.artist.as_str()
    };
    listenbrainz::song(
        &item.title,
        artist,
        item.album.as_deref(),
        item.duration_ms.map(|ms| ms as i64).or(facts.duration_ms),
        facts.isrc.as_deref(),
        facts.track_no,
    )
}

/// Sends what waits, and tells the song playing now, until the core stops.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let agent = listenbrainz::agent();
    let scrobbling = &state.scrobbling;
    // Plays left waiting last time are looked at once the core is up.
    let mut looked: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        let playing = {
            let bell = scrobbling.bell.lock().unwrap();
            let (mut bell, _) = scrobbling
                .rung
                .wait_timeout_while(bell, Duration::from_secs(1), |b| {
                    !b.rung && b.playing.is_none()
                })
                .unwrap();
            let rung = std::mem::take(&mut bell.rung);
            let playing = bell.playing.take();
            let due = looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY);
            if !rung && playing.is_none() && !due {
                continue;
            }
            playing
        };
        looked = Some(Instant::now());
        let targets = state
            .library
            .lock()
            .unwrap()
            .scrobble_targets()
            .unwrap_or_default();
        for target in targets.iter().filter(|t| t.enabled && !t.refused) {
            if scrobbling.resting(target.id) {
                continue;
            }
            let Some((_, url, token)) = state
                .library
                .lock()
                .unwrap()
                .scrobble_token(target.id)
                .ok()
                .flatten()
            else {
                continue;
            };
            if let Some(song) = &playing {
                // Told once, as it happens: a service that misses it has the play itself later.
                let _ = listenbrainz::now_playing(&agent, &url, &token, song);
            }
            if target.waiting > 0 {
                send_waiting(state, &agent, target.id, &url, &token);
            }
        }
    }
}

/// Sends the plays waiting for service `target`, a batch at a time, until none wait or it cannot take more now.
fn send_waiting(state: &State, agent: &listenbrainz::Agent, target: i64, url: &str, token: &str) {
    let library = || state.library.lock().unwrap();
    let ids = |plays: &[Waiting]| plays.iter().map(|w| w.id).collect::<Vec<_>>();
    loop {
        let batch = library()
            .waiting_scrobbles(target, listenbrainz::AT_ONCE)
            .unwrap_or_default();
        if batch.is_empty() {
            return;
        }
        // A play no service could take (no title or artist) is let go without asking.
        let (good, bad): (Vec<Waiting>, Vec<Waiting>) = batch
            .into_iter()
            .partition(|w| listenbrainz::sendable(&w.song));
        if !bad.is_empty() {
            let _ = library().scrobbles_dropped(target, &ids(&bad));
        }
        if good.is_empty() {
            continue;
        }
        match send(agent, url, token, &good) {
            Sent::Taken => {
                let _ = library().scrobbles_sent(target, &ids(&good), now_ms());
                state.scrobbling.rested(target);
            }
            // One of them is wrong: each is sent alone, and only the wrong ones are let go.
            Sent::Wrong(_) if good.len() > 1 => {
                for one in good {
                    match send(agent, url, token, std::slice::from_ref(&one)) {
                        Sent::Taken => {
                            let _ = library().scrobbles_sent(target, &[one.id], now_ms());
                        }
                        Sent::Wrong(why) => let_go(state, target, one.id, &why),
                        other => return failed(state, target, other),
                    }
                }
                state.scrobbling.rested(target);
            }
            Sent::Wrong(why) => let_go(state, target, good[0].id, &why),
            other => return failed(state, target, other),
        }
    }
}

fn send(agent: &listenbrainz::Agent, url: &str, token: &str, plays: &[Waiting]) -> Sent {
    let plays: Vec<(i64, &Value)> = plays.iter().map(|w| (w.listened_at, &w.song)).collect();
    listenbrainz::send(agent, url, token, &plays)
}

/// The service will never take this play: it waits no more, and the listener is told why.
fn let_go(state: &State, target: i64, id: i64, why: &str) {
    let mut library = state.library.lock().unwrap();
    let _ = library.scrobbles_dropped(target, &[id]);
    let _ = library.scrobble_trouble(
        target,
        &format!("a play was not taken: {why}"),
        false,
        now_ms(),
    );
}

fn failed(state: &State, target: i64, outcome: Sent) {
    let mut library = state.library.lock().unwrap();
    match outcome {
        Sent::Refused(_) => {
            let _ = library.scrobble_trouble(target, "it refused the token", true, now_ms());
        }
        Sent::Later { why, wait } => {
            let _ = library.scrobble_trouble(target, &why, false, now_ms());
            drop(library);
            state.scrobbling.rest(target, wait);
        }
        Sent::Taken | Sent::Wrong(_) => {}
    }
}

/// The services, as the window sees them (never a token).
fn listed(state: &State) -> Result<Value> {
    Ok(json!({ "targets": state.library.lock().unwrap().scrobble_targets()? }))
}

/// /api/scrobbling: the services, one added (its token checked first), turned on or off, removed, or asked again.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/scrobbling") {
        return Ok(None);
    }
    let method = request.method().clone();
    if method == Method::Get && path == "/api/scrobbling" {
        return Ok(Some(reply_json(200, listed(state)?)));
    }
    if method != Method::Post {
        return Ok(None);
    }
    if path == "/api/scrobbling" {
        let body = body_json(request)?;
        let token = body
            .get("token")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if token.is_empty() {
            return Ok(Some(refuse(400, "A token is needed")));
        }
        if token.len() > TOKEN_CAP || token.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Ok(Some(refuse(400, "That is not a token")));
        }
        let given = body
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .unwrap_or(listenbrainz::ADDRESS);
        let root = match listenbrainz::root(given) {
            Ok(root) => root,
            Err(why) => return Ok(Some(refuse(400, &why))),
        };
        let name = if root == listenbrainz::ADDRESS {
            "ListenBrainz"
        } else {
            "That server"
        };
        return Ok(Some(
            match listenbrainz::check(&listenbrainz::agent(), &root, token) {
                Ok(user) => {
                    let user = (!user.is_empty()).then_some(user.as_str());
                    let id = state.library.lock().unwrap().add_scrobble_target(
                        listenbrainz::KIND,
                        &root,
                        token,
                        user,
                        now_ms(),
                    )?;
                    state.scrobbling.ring(Some(id));
                    let mut answer = listed(state)?;
                    answer["id"] = json!(id);
                    reply_json(200, answer)
                }
                Err(Sent::Refused(_)) => refuse(400, &format!("{name} did not take that token")),
                Err(Sent::Wrong(why)) => refuse(400, &why),
                Err(Sent::Later { why, .. }) => {
                    refuse(502, &format!("{name} could not check the token: {why}"))
                }
                Err(Sent::Taken) => refuse(500, "the token was not checked"),
            },
        ));
    }
    // /api/scrobbling/{id}/enabled {enabled}, /api/scrobbling/{id}/remove, /api/scrobbling/{id}/again
    let Some((id, verb)) = path
        .strip_prefix("/api/scrobbling/")
        .and_then(|rest| rest.split_once('/'))
    else {
        return Ok(None);
    };
    let Ok(id) = id.parse::<i64>() else {
        return Ok(Some(refuse(404, "no such service")));
    };
    let found = match verb {
        "enabled" => {
            let on = body_json(request)?
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let found = state
                .library
                .lock()
                .unwrap()
                .set_scrobble_target_enabled(id, on)?;
            if found && on {
                state.scrobbling.ring(Some(id));
            }
            found
        }
        "remove" => state.library.lock().unwrap().remove_scrobble_target(id)?,
        "again" => {
            let found = state.library.lock().unwrap().scrobble_token(id)?.is_some();
            if found {
                state.scrobbling.ring(Some(id));
            }
            found
        }
        _ => return Ok(None),
    };
    Ok(Some(if found {
        reply_json(200, listed(state)?)
    } else {
        refuse(404, "no such service")
    }))
}
