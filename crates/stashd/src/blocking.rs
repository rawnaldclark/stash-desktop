//! Songs and artists the listener chose not to play (`stash_core::blocks`): the routes, and what the queue does with them.
//!
//! Stash never plays them on its own. A list played through leaves them out, and so does a group of songs added to Up
//! next; autoplay never picks them (`Library::autoplay_picks`). One song chosen by itself still plays: the listener
//! asked for that very song. Choosing not to play a song takes it out of the queue, and skips it if it is playing.

use std::collections::HashSet;

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
fn blocked(library: &Library, item: &QueueItem, rows: &HashSet<i64>) -> bool {
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

/// After new choices not to play (each a kind and the key it is kept under): the songs they cover leave the queue,
/// wherever they are in it (before the song playing too: the queue may start over, and a queue kept from last time
/// rests on one), and the song playing is skipped if it is one. Only what these choices cover: a song chosen not to be
/// played before, and then played or added by itself, stays.
pub(crate) fn clear_queue(state: &State, choices: &[(BlockKind, String)]) {
    let Ok(player) = &state.player else { return };
    if choices.is_empty() {
        return;
    }
    let rows: HashSet<i64> = {
        let library = state.library.lock().unwrap();
        choices
            .iter()
            .filter_map(|(kind, key)| library.ids_blocked_by(*kind, key).ok())
            .flatten()
            .collect()
    };
    // An add-on's song is known by its words: its keys are made once, and looked up.
    let chosen: HashSet<(BlockKind, &str)> = choices
        .iter()
        .map(|(kind, key)| (*kind, key.as_str()))
        .collect();
    player.retain(|item| match item.id {
        Some(id) => !rows.contains(&id),
        None => ![BlockKind::Song, BlockKind::Artist]
            .into_iter()
            .any(|kind| {
                kind.key(&item.title, &item.artist)
                    .is_some_and(|key| chosen.contains(&(kind, key.as_str())))
            }),
    });
}

/// The answer to a change: every choice as it is now, how the songs it touched are chosen now (so the window marks
/// them at once), and the count of changes.
fn answered(state: &State, ids: &[i64]) -> Result<Reply> {
    let (rows, songs) = {
        let library = state.library.lock().unwrap();
        (library.blocks()?, library.blocked_kinds(ids)?)
    };
    let songs: Vec<Value> = songs
        .into_iter()
        .map(|(id, kind)| json!({ "id": id, "blocked": kind }))
        .collect();
    let edits = state.edits.load(std::sync::atomic::Ordering::Relaxed);
    Ok(reply_json(
        200,
        json!({ "rows": rows, "songs": songs, "edits": edits }),
    ))
}

/// `/api/blocks`. GET: every song and artist the listener chose not to play, the latest first. POST `{kind: "song" |
/// "artist", title (a song's), artist, blocked}`: chooses not to play it (`blocked` true) or to play it again; to play
/// one again, `key` (as the list gives it) may stand for the words. The answer says how the library's songs it covers
/// are chosen now, so the window marks them at once.
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
            // Several songs at once (picked in a list), each by its words.
            if let Some(songs) = body
                .get("songs")
                .and_then(Value::as_array)
                .filter(|_| kind == BlockKind::Song)
            {
                let songs: Vec<(String, String)> = songs
                    .iter()
                    .filter_map(|s| {
                        let words = |name: &str| Some(s.get(name)?.as_str()?.trim().to_string());
                        Some((words("title")?, words("artist")?))
                    })
                    .take(crate::serve::PICKS_CAP)
                    .collect();
                let (keys, ids) = {
                    let mut library = state.library.lock().unwrap();
                    let keys = library.set_songs_blocked(&songs, on, now_ms())?;
                    let mut ids = Vec::new();
                    for key in &keys {
                        ids.extend(library.ids_blocked_by(BlockKind::Song, key)?);
                    }
                    (keys, ids)
                };
                if on {
                    let choices: Vec<(BlockKind, String)> =
                        keys.into_iter().map(|key| (BlockKind::Song, key)).collect();
                    clear_queue(state, &choices);
                }
                state.edited();
                return answered(state, &ids).map(Some);
            }
            let given = Some(text("key")).filter(|k| !k.is_empty());
            let (key, ids) = {
                let mut library = state.library.lock().unwrap();
                let key = match given {
                    // Played again by the key it is kept under: what was kept of its words may be shorter than they were.
                    Some(key) if !on => {
                        library.unblock_key(kind, &key)?;
                        Some(key)
                    }
                    _ => {
                        let (title, artist) = (text("title"), text("artist"));
                        if artist.is_empty() || (kind == BlockKind::Song && title.is_empty()) {
                            return Ok(Some(refuse(
                                400,
                                "a song needs its title and artist, an artist its name",
                            )));
                        }
                        library.set_blocked(kind, &title, &artist, on, now_ms())?
                    }
                };
                let Some(key) = key else {
                    return Ok(Some(refuse(
                        400,
                        "those words have no letters or numbers to know it by",
                    )));
                };
                let ids = library.ids_blocked_by(kind, &key)?;
                (key, ids)
            };
            if on {
                clear_queue(state, &[(kind, key)]);
            }
            // The songs' rows say which are chosen not to be played: every window reads its lists again.
            state.edited();
            return answered(state, &ids).map(Some);
        }
        _ => return Ok(None),
    }
    let rows = state.library.lock().unwrap().blocks()?;
    Ok(Some(reply_json(200, json!({ "rows": rows }))))
}
