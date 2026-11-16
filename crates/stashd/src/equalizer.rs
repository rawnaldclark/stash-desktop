//! The equalizer of the output in use (see `stash_core::eq`): Settings › Sound.
//!
//! POST /api/player/eq `{on?, bass?, treble?, correction?: null}` changes it: Bass and Treble are "less", "as-is" or
//! "more" (a tone other than "as-is" turns the switch on, unless `on` is given too), and a correction can only be
//! removed here. POST /api/player/eq/pick: the system's own dialog, for an equalizer file; answers its path (null when
//! none was chosen). POST /api/player/eq/file `{path}`: reads that file as Equalizer APO reads it (AutoEq's
//! "ParametricEQ.txt" files are that kind) and puts it on the output in use, switched on. Each answers the player's
//! state. What each output keeps is kept in the library (`eq`), one write a change.

use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::eq::{self, Tilt};
use stash_core::player::Player;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, player_reply, refuse, reply_json};

/// Where what each output keeps is kept.
pub(crate) const SETTING: &str = "eq";

/// One change at a time: each reads the equalizer, changes it and keeps it, and two at once (the window and the mini
/// player) would lose one.
static CHANGING: Mutex<()> = Mutex::new(());

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if *request.method() != Method::Post {
        return Ok(None);
    }
    let reply = match path {
        "/api/player/eq/pick" => {
            let picked = rfd::FileDialog::new()
                .set_title("Choose an equalizer file")
                .add_filter("Equalizer APO files", &["txt"])
                .add_filter("All files", &["*"])
                .pick_file()
                .map(|file| file.to_string_lossy().into_owned());
            reply_json(200, json!({ "path": picked }))
        }
        "/api/player/eq/file" => with_player(state, |player| from_file(state, player, request))?,
        "/api/player/eq" => with_player(state, |player| change(state, player, request))?,
        _ => return Ok(None),
    };
    Ok(Some(reply))
}

fn with_player(state: &State, act: impl FnOnce(&Player) -> Result<Reply>) -> Result<Reply> {
    match &state.player {
        Ok(player) => act(player),
        Err(why) => Ok(refuse(409, why)),
    }
}

/// Keeps what each output keeps, then answers the player's state.
fn kept(state: &State, player: &Player) -> Result<Reply> {
    state
        .library
        .lock()
        .unwrap()
        .set_setting(SETTING, &player.equalizers().to_json())?;
    Ok(player_reply(state))
}

fn tilt(value: &Value, name: &str) -> std::result::Result<Option<Tilt>, String> {
    let Some(value) = value.get(name) else {
        return Ok(None);
    };
    match value.as_str() {
        Some("less") => Ok(Some(Tilt::Less)),
        Some("as-is") => Ok(Some(Tilt::AsIs)),
        Some("more") => Ok(Some(Tilt::More)),
        _ => Err(format!("{name} is less, as-is or more")),
    }
}

/// `POST /api/player/eq`.
fn change(state: &State, player: &Player, request: &mut Request) -> Result<Reply> {
    let body = body_json(request)?;
    let (bass, treble) = match (tilt(&body, "bass"), tilt(&body, "treble")) {
        (Err(words), _) | (_, Err(words)) => return Ok(refuse(400, &words)),
        (Ok(bass), Ok(treble)) => (bass, treble),
    };
    let on = match body.get("on") {
        None => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(_) => return Ok(refuse(400, "on is true or false")),
    };
    let remove = match body.get("correction") {
        None => false,
        Some(Value::Null) => true,
        Some(_) => {
            return Ok(refuse(
                400,
                "A correction comes from a file; here it can only be removed (null)",
            ));
        }
    };
    if on.is_none() && bass.is_none() && treble.is_none() && !remove {
        return Ok(refuse(400, "say on, bass, treble or correction"));
    }
    let _one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let mut eq = player.eq();
    if let Some(bass) = bass {
        eq.bass = bass;
    }
    if let Some(treble) = treble {
        eq.treble = treble;
    }
    if remove {
        eq.correction = None;
    }
    // A tone chosen turns the equalizer on, unless the same request says otherwise.
    let toned = [bass, treble]
        .iter()
        .any(|t| t.is_some_and(|t| t != Tilt::AsIs));
    eq.on = on.unwrap_or(eq.on || toned);
    player.set_eq(eq)?;
    kept(state, player)
}

/// `POST /api/player/eq/file`.
fn from_file(state: &State, player: &Player, request: &mut Request) -> Result<Reply> {
    let body = body_json(request)?;
    let Some(path) = body
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return Ok(refuse(400, "Choose an equalizer file"));
    };
    let file = Path::new(path);
    let Ok(meta) = std::fs::metadata(file) else {
        return Ok(refuse(400, "That file was not found"));
    };
    if !meta.is_file() {
        return Ok(refuse(400, "That file was not found"));
    }
    if meta.len() > eq::FILE_MOST {
        return Ok(refuse(400, "That file is too big to be an equalizer file"));
    }
    // No more is read than an equalizer file may hold, whatever the file says of its size (a file still growing, or
    // one of a kind that says 0).
    let mut bytes = Vec::new();
    let read = std::fs::File::open(file)
        .and_then(|opened| opened.take(eq::FILE_MOST + 1).read_to_end(&mut bytes));
    if read.is_err() {
        return Ok(refuse(400, "That file could not be read"));
    }
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let correction = match eq::read_apo(&bytes, &name) {
        Ok(correction) => correction,
        Err(stash_core::Error::Refused(words)) => return Ok(refuse(400, &words)),
        Err(e) => return Err(e.into()),
    };
    let _one = CHANGING.lock().unwrap_or_else(|e| e.into_inner());
    let mut eq = player.eq();
    eq.correction = Some(correction);
    eq.on = true;
    player.set_eq(eq)?;
    kept(state, player)
}

/// Brings back what each output keeps (when the core starts), and keeps what changes by itself: the output opened
/// last, or an equalizer the engine could not use and turned off.
pub(crate) fn bring_back(player: &Player, kept: Option<String>) {
    player.set_equalizers(eq::Book::from_json(&kept.unwrap_or_default()));
}
