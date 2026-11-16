//! Songs and artists the listener chose not to play (`stash_core::blocks`): the routes, and what the queue does with them.
//!
//! Stash never plays them on its own. A list played through leaves them out, and so does a group of songs added to Up
//! next; autoplay never picks them (`Library::autoplay_picks`). One song chosen by itself still plays: the listener
//! asked for that very song. Choosing not to play a song takes it out of Up next, and skips it if it is playing.

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::blocks::BlockKind;
use stash_core::player::QueueItem;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, now_ms, refuse, reply_json};

/// What is said when every song from the one asked for on is one the listener chose not to play.
pub(crate) const ALL_BLOCKED: &str = "You chose not to play any of those songs";

/// Whether a queued song is one the listener chose not to play: a song of the library by its row, an add-on's by its
/// words.
fn blocked(library: &Library, item: &QueueItem, rows: &std::collections::HashSet<i64>) -> bool {
    match item.id {
        Some(id) => rows.contains(&id),
        None => library
            .words_blocked(&item.title, &item.artist)
            .unwrap_or(false),
    }
}

/// A list to play (or add to Up next) without the songs the listener chose not to play. One song stays as it is: it was
/// asked for by itself. `start` is the place to begin; when that song is left out, the next one that is not takes its
/// place. `None` when nothing from `start` on is left.
pub(crate) fn leave_out(
    library: &Library,
    items: Vec<QueueItem>,
    start: usize,
) -> stash_core::Result<Option<(Vec<QueueItem>, usize)>> {
    if items.len() <= 1 || !library.anything_blocked()? {
        return Ok(Some((items, start)));
    }
    let ids: Vec<i64> = items.iter().filter_map(|i| i.id).collect();
    let rows = library.blocked_of(&ids)?;
    let out: Vec<bool> = items.iter().map(|i| blocked(library, i, &rows)).collect();
    let Some(first) = (start..items.len()).find(|&place| !out[place]) else {
        return Ok(None);
    };
    let mut kept = Vec::with_capacity(items.len());
    let mut begins = 0;
    for (place, item) in items.into_iter().enumerate() {
        if place == first {
            begins = kept.len();
        }
        if !out[place] {
            kept.push(item);
        }
    }
    Ok(Some((kept, begins)))
}

/// After a choice not to play: the songs it covers leave Up next, and the song playing is skipped if it is one.
fn clear_queue(state: &State) {
    let Ok(player) = &state.player else { return };
    let now = player.state();
    let Some(index) = now.index else { return };
    let from_here = player.queue_at(index, now.queue_len.saturating_sub(index));
    let out: Vec<bool> = {
        let library = state.library.lock().unwrap();
        let ids: Vec<i64> = from_here.iter().filter_map(|i| i.id).collect();
        let rows = library.blocked_of(&ids).unwrap_or_default();
        from_here
            .iter()
            .map(|i| blocked(&library, i, &rows))
            .collect()
    };
    // The last first, so the places before stay where they are.
    for (offset, _) in out
        .iter()
        .enumerate()
        .skip(1)
        .rev()
        .filter(|(_, out)| **out)
    {
        player.remove_at(index + offset);
    }
    if out.first() == Some(&true) {
        let _ = player.next();
    }
}

/// `/api/blocks`. GET: every song and artist the listener chose not to play, the latest first. POST `{kind: "song" |
/// "artist", title (a song's), artist, blocked}`: chooses not to play it (`blocked` true) or to play it again.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if path != "/api/blocks" {
        return Ok(None);
    }
    match *request.method() {
        Method::Get => {}
        Method::Post => {
            let body = body_json(request)?;
            let text = |name: &str| {
                body.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };
            let Some(kind) = body
                .get("kind")
                .and_then(Value::as_str)
                .and_then(BlockKind::from_word)
            else {
                return Ok(Some(refuse(400, "kind is \"song\" or \"artist\"")));
            };
            let Some(on) = body.get("blocked").and_then(Value::as_bool) else {
                return Ok(Some(refuse(400, "blocked is true or false")));
            };
            let (title, artist) = (text("title"), text("artist"));
            if artist.is_empty() || (kind == BlockKind::Song && title.is_empty()) {
                return Ok(Some(refuse(
                    400,
                    "a song needs its title and artist, an artist its name",
                )));
            }
            let (key, songs) = {
                let mut library = state.library.lock().unwrap();
                let key = library.set_blocked(kind, &title, &artist, on, now_ms())?;
                // The library's songs it covers, as they are now: the window marks them at once.
                let songs = match &key {
                    Some(key) => library.blocked_kinds(&library.ids_blocked_by(kind, key)?)?,
                    None => Vec::new(),
                };
                (key, songs)
            };
            if key.is_none() {
                return Ok(Some(refuse(
                    400,
                    "those words have no letters or numbers to know it by",
                )));
            }
            if on {
                clear_queue(state);
            }
            // The songs' rows say which are chosen not to be played: every window reads its lists again.
            state.edited();
            let rows = state.library.lock().unwrap().blocks()?;
            let songs: Vec<Value> = songs
                .into_iter()
                .map(|(id, kind)| json!({ "id": id, "blocked": kind }))
                .collect();
            let edits = state.edits.load(std::sync::atomic::Ordering::Relaxed);
            return Ok(Some(reply_json(
                200,
                json!({ "rows": rows, "songs": songs, "edits": edits }),
            )));
        }
        _ => return Ok(None),
    }
    let rows = state.library.lock().unwrap().blocks()?;
    Ok(Some(reply_json(200, json!({ "rows": rows }))))
}
