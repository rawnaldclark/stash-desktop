//! Songs no add-on could find, fixed by the listener: the list (`GET /api/unfound`), an add-on's closest results for
//! one song (`GET /api/unfound/{id}/candidates`, one search, asked for by the listener), the pick
//! (`POST /api/unfound/fix`: kept as that add-on's name for the song, so it plays from there from now on), and "none
//! of these" (`POST /api/unfound/aside`).
//!
//! Songs come onto the list only from playing or saving them (`through_addons` in `serve.rs`), never from a check of
//! the library: an add-on's allowance is the listener's.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::matching;
use tiny_http::{Method, Request};

use crate::serve::{Loaded, Reply, State, body_json, now_ms, refuse, reply_json, search_addons};

/// How many of each add-on's results are shown for a song.
const CANDIDATES: usize = 8;

fn counts(state: &State, offset: usize, limit: usize) -> Result<Value> {
    let library = state.library.lock().unwrap();
    let counts = library.unfound_counts()?;
    let rows = if limit == 0 {
        Vec::new()
    } else {
        library.unfound_at(offset, limit)?
    };
    Ok(json!({ "rows": rows, "open": counts.open, "marked": counts.marked }))
}

/// One of the routes of the list of songs no add-on could find. `None`: the path is not one of them.
pub(crate) fn route(
    state: &State,
    request: &mut Request,
    path: &str,
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Reply>> {
    if !path.starts_with("/api/unfound") {
        return Ok(None);
    }
    let number = |name: &str, default: usize| {
        param(name)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default)
    };
    // An add-on's results for one song: /api/unfound/{id}/candidates[?q=other words]
    if *request.method() == Method::Get
        && let Some(id) = path
            .strip_prefix("/api/unfound/")
            .and_then(|rest| rest.strip_suffix("/candidates"))
    {
        let Ok(id) = id.parse::<i64>() else {
            return Ok(Some(refuse(404, "no such song in the library")));
        };
        let Some(described) = state.library.lock().unwrap().describe(id)? else {
            return Ok(Some(refuse(404, "no such song in the library")));
        };
        let query = param("q")
            .map(|q| q.trim().chars().take(200).collect::<String>())
            .filter(|q| !q.is_empty())
            .unwrap_or_else(|| matching::query(&described.wanted));
        let askable: Vec<Arc<Loaded>> = state
            .addons
            .read()
            .unwrap()
            .iter()
            .filter(|a| {
                a.usable().is_some_and(|addon| {
                    addon.offers_resource("search") && addon.offers_resource("stream")
                })
            })
            .cloned()
            .collect();
        let groups = search_addons(&askable, &query, CANDIDATES);
        return Ok(Some(reply_json(
            200,
            json!({ "query": query, "groups": groups }),
        )));
    }
    match (request.method(), path) {
        // The list (newest miss first), how many are on it, and every song known not to play.
        (Method::Get, "/api/unfound") => Ok(Some(reply_json(
            200,
            counts(state, number("offset", 0), number("limit", 300).min(1_000))?,
        ))),
        // The listener's pick: { id, addon, track }.
        (Method::Post, "/api/unfound/fix") => {
            let body = body_json(request)?;
            let (Some(id), Some(addon), Some(track)) = (
                body.get("id").and_then(Value::as_i64),
                body.get("addon").and_then(Value::as_str),
                body.get("track")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty() && t.len() <= 512),
            ) else {
                return Ok(Some(refuse(
                    400,
                    "a song, an add-on and its track are needed",
                )));
            };
            let plays = state.addons.read().unwrap().iter().any(|a| {
                a.stored.id == addon
                    && a.usable()
                        .is_some_and(|usable| usable.offers_resource("stream"))
            });
            if !plays {
                return Ok(Some(refuse(
                    400,
                    "that add-on is not one that plays songs here",
                )));
            }
            let fixed = state
                .library
                .lock()
                .unwrap()
                .fix_unfound(id, addon, track, now_ms())?;
            let Some(saving) = fixed else {
                return Ok(Some(refuse(404, "no such song in the library")));
            };
            state.lacks.forget(id);
            if saving {
                state.saving.ring();
            }
            let mut reply = counts(state, 0, 0)?;
            reply["saving"] = json!(saving);
            Ok(Some(reply_json(200, reply)))
        }
        // "None of these": { ids, aside } (aside false puts them back on the list).
        (Method::Post, "/api/unfound/aside") => {
            let body = body_json(request)?;
            let ids: Vec<i64> = body
                .get("ids")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_i64).take(10_000).collect())
                .unwrap_or_default();
            let aside = body.get("aside").and_then(Value::as_bool).unwrap_or(true);
            state
                .library
                .lock()
                .unwrap()
                .set_unfound_aside(&ids, aside, now_ms())?;
            Ok(Some(reply_json(200, counts(state, 0, 0)?)))
        }
        _ => Ok(None),
    }
}
