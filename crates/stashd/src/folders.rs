//! Playlist folders (`stash_core::folders`): the routes. Every answer to a change carries the folders, the playlists and
//! the counts as they now are, like the playlists' own routes, so the window need not ask again.

use std::sync::atomic::Ordering;

use anyhow::Result;
use serde_json::{Value, json};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, refuse, reply_json};

/// What is said when a folder's name has nothing in it.
const NO_NAME: &str = "A folder needs a name";

/// `/api/folders`: GET lists them, in their order. POST `{name, playlist?}` makes one after the others (and puts that
/// playlist in it). POST `/api/folders/{id}/rename` `{name}`, `/delete` (its playlists stay, out of any folder), `/fold`
/// `{collapsed}`. A playlist goes in or out of a folder by `/api/playlists/{id}/folder`.
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let verb = path
        .strip_prefix("/api/folders/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(id, verb)| Some((id.parse::<i64>().ok()?, verb.to_string())));
    if path != "/api/folders" && verb.is_none() {
        return Ok(None);
    }
    match *request.method() {
        Method::Get if verb.is_none() => {
            let rows = state.library.lock().unwrap().playlist_folders()?;
            return Ok(Some(reply_json(200, json!({ "rows": rows }))));
        }
        Method::Post => {}
        _ => return Ok(None),
    }
    let body = body_json(request)?;
    let name = body.get("name").and_then(Value::as_str).unwrap_or_default();
    let mut library = state.library.lock().unwrap();
    let mut answer = json!({});
    // Folding one shut changes nothing another window shows.
    let mut edited = true;
    match verb {
        None => {
            let Some(id) = library.create_playlist_folder(name)? else {
                return Ok(Some(refuse(400, NO_NAME)));
            };
            if let Some(playlist) = body.get("playlist").and_then(Value::as_i64) {
                library.set_playlist_folder(playlist, Some(id))?;
            }
            answer["id"] = json!(id);
        }
        Some((id, verb)) => {
            let done = match verb.as_str() {
                "rename" => match library.rename_playlist_folder(id, name)? {
                    Some(done) => done,
                    None => return Ok(Some(refuse(400, NO_NAME))),
                },
                "delete" => library.delete_playlist_folder(id)?,
                "fold" => {
                    edited = false;
                    let collapsed = body
                        .get("collapsed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    library.fold_playlist_folder(id, collapsed)?
                }
                _ => return Ok(None),
            };
            if !done {
                return Ok(Some(refuse(404, "no such folder")));
            }
            answer["done"] = json!(true);
        }
    }
    answer["folders"] = json!(library.playlist_folders()?);
    answer["playlists"] = json!(library.playlists()?);
    answer["counts"] = json!(library.counts()?);
    drop(library);
    if edited {
        state.edited();
    }
    answer["edits"] = json!(state.edits.load(Ordering::Relaxed));
    Ok(Some(reply_json(200, answer)))
}
