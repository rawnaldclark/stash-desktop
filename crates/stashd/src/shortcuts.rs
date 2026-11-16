//! Shortcuts that work in any app: keys the listener picks for playing and pausing, the next and previous song, the
//! volume, liking the song playing and bringing Stash to the front. The core keeps the keys and does what each one
//! asks (`POST /api/shortcut`); the program around it (the desktop shell) holds them system-wide and hands each key
//! pressed back as its action. Without such a program (`stashd serve`) there are none.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::Library;
use stash_core::import::Song;
use tiny_http::{Method, Request};

use crate::serve::{KeysShell, Reply, State, View, body_json, now_ms, refuse, reply_json};

/// Each action, with the keys it has until the listener picks others ("" for none). None of them is Ctrl+Alt with a
/// letter or a number: on many keyboards that is AltGr, which types characters (€, @, ł).
pub(crate) const ACTIONS: &[(&str, &str)] = &[
    ("toggle", "Ctrl+Alt+Home"),
    ("next", "Ctrl+Alt+PageDown"),
    ("previous", "Ctrl+Alt+PageUp"),
    ("volume_up", "Ctrl+Alt+Up"),
    ("volume_down", "Ctrl+Alt+Down"),
    ("like", ""),
    ("show", ""),
];

/// The library setting that keeps them: `{"on": bool, "keys": {action: keys}}`.
const SETTING: &str = "shortcuts";
/// How far one press of a volume shortcut moves the volume, in percent.
const VOLUME_STEP: f64 = 5.0;

/// The shortcuts held for the listener.
#[derive(Default)]
pub(crate) struct Shortcuts {
    /// The program that holds keys system-wide, when the core runs inside one.
    pub(crate) shell: Option<KeysShell>,
    /// Let go of while the listener picks new keys in the window: keys held system-wide never reach the window.
    paused: AtomicBool,
    /// The keys the shell could not hold, by action, with why. Its lock also keeps two changes from crossing.
    trouble: Mutex<BTreeMap<String, String>>,
}

/// The listener's choice: whether the shortcuts are on, and each action's keys (its own, or the first ones).
struct Chosen {
    on: bool,
    keys: BTreeMap<String, String>,
}

fn chosen(library: &Library) -> Result<Chosen> {
    let stored: Value = library
        .setting(SETTING)?
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let keys = ACTIONS
        .iter()
        .map(|(action, first)| {
            let keys = stored["keys"]
                .get(*action)
                .and_then(Value::as_str)
                .unwrap_or(first);
            (action.to_string(), keys.to_string())
        })
        .collect();
    Ok(Chosen {
        on: stored["on"].as_bool().unwrap_or(false),
        keys,
    })
}

/// The modifiers, in the order a shortcut names them.
const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Super"];

/// Which of [`MODIFIERS`] a word names, if any.
fn modifier(word: &str) -> Option<usize> {
    match word.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(0),
        "alt" | "option" => Some(1),
        "shift" => Some(2),
        "super" | "cmd" | "command" | "meta" | "win" => Some(3),
        _ => None,
    }
}

/// `keys` as the shell reads them: the modifiers first, always in one order, then one key as the window names it
/// ("Home", "KeyA", "ArrowUp", "F13"). `None` for keys that would take a key from the listener's typing everywhere: a
/// shortcut needs Ctrl, Alt or the Windows key (Command on a Mac), unless its key is F13 to F24, which nothing types.
pub(crate) fn canonical(keys: &str) -> Option<String> {
    let words: Vec<&str> = keys.split('+').map(str::trim).collect();
    let (key, modifiers) = words.split_last()?;
    if key.is_empty()
        || key.len() > 24
        || !key.chars().all(|c| c.is_ascii_alphanumeric())
        || modifier(key).is_some()
    {
        return None;
    }
    let mut held = [false; 4];
    for word in modifiers {
        held[modifier(word)?] = true;
    }
    let beyond_typing = key
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (13..=24).contains(&n));
    if !(held[0] || held[1] || held[3] || beyond_typing) {
        return None;
    }
    let mut named: Vec<&str> = MODIFIERS
        .iter()
        .zip(held)
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();
    named.push(key);
    Some(named.join("+"))
}

/// Has the shell hold the listener's keys: none while the shortcuts are off or new keys are being picked. Notes those
/// it could not hold.
pub(crate) fn apply(state: &State) -> Result<()> {
    let Some(shell) = &state.shortcuts.shell else {
        return Ok(());
    };
    // The library is read and let go of before the shell is asked: the shell waits for the window's own thread.
    let chosen = chosen(&state.library.lock().unwrap())?;
    let wanted: Vec<(String, String)> =
        if chosen.on && !state.shortcuts.paused.load(Ordering::Relaxed) {
            chosen
                .keys
                .into_iter()
                .filter(|(_, keys)| !keys.is_empty())
                .collect()
        } else {
            Vec::new()
        };
    let mut trouble = state.shortcuts.trouble.lock().unwrap();
    *trouble = shell(&wanted).into_iter().collect();
    Ok(())
}

/// The shortcuts as the window shows them.
fn described(state: &State) -> Result<Value> {
    let chosen = chosen(&state.library.lock().unwrap())?;
    Ok(json!({
        "available": state.shortcuts.shell.is_some(),
        "on": chosen.on,
        "actions": ACTIONS.iter().map(|(action, _)| action).collect::<Vec<_>>(),
        "keys": chosen.keys,
        "trouble": *state.shortcuts.trouble.lock().unwrap(),
    }))
}

/// `/api/settings/shortcuts` (GET: the shortcuts; POST `{on?, keys?: {action: keys}, paused?}`: changes them, and
/// answers them as they now are) and `/api/shortcut` (POST `{action}`: what a key pressed asks for).
pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    match (request.method(), path) {
        (Method::Get, "/api/settings/shortcuts") => Ok(Some(reply_json(200, described(state)?))),
        (Method::Post, "/api/settings/shortcuts") => {
            let body = body_json(request)?;
            if let Some(paused) = body.get("paused").and_then(Value::as_bool) {
                state.shortcuts.paused.store(paused, Ordering::Relaxed);
            }
            if body.get("on").is_some() || body.get("keys").is_some() {
                let mut library = state.library.lock().unwrap();
                let mut chosen = chosen(&library)?;
                if let Some(on) = body.get("on").and_then(Value::as_bool) {
                    chosen.on = on;
                }
                for (action, keys) in body
                    .get("keys")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    if !chosen.keys.contains_key(action) {
                        return Ok(Some(refuse(404, "no such shortcut")));
                    }
                    let keys = keys.as_str().unwrap_or_default().trim();
                    let keys = if keys.is_empty() {
                        String::new()
                    } else {
                        let Some(keys) = canonical(keys) else {
                            return Ok(Some(refuse(
                                400,
                                "A shortcut needs Ctrl, Alt or the Windows key, with one other key",
                            )));
                        };
                        keys
                    };
                    if !keys.is_empty()
                        && let Some((other, _)) = chosen.keys.iter().find(|(it, theirs)| {
                            *it != action && theirs.eq_ignore_ascii_case(&keys)
                        })
                    {
                        return Ok(Some(reply_json(
                            409,
                            json!({ "error": {
                                "message": "Those keys are already another shortcut's",
                                "taken_by": other,
                            } }),
                        )));
                    }
                    chosen.keys.insert(action.clone(), keys);
                }
                library.set_setting(
                    SETTING,
                    &json!({ "on": chosen.on, "keys": chosen.keys }).to_string(),
                )?;
            }
            apply(state)?;
            Ok(Some(reply_json(200, described(state)?)))
        }
        (Method::Post, "/api/shortcut") => {
            let body = body_json(request)?;
            let action = body
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default();
            act(state, action).map(Some)
        }
        _ => Ok(None),
    }
}

/// Does what a shortcut asks.
fn act(state: &State, action: &str) -> Result<Reply> {
    match action {
        "show" => {
            let shown = state.window.as_ref().is_some_and(|shell| shell(View::Full));
            return Ok(reply_json(200, json!({ "done": shown })));
        }
        "like" => return like_playing(state),
        _ => {}
    }
    let Ok(playing) = &state.player else {
        return Ok(refuse(503, "there is no player"));
    };
    match action {
        "toggle" => playing.set_paused(playing.state().status == "playing")?,
        "next" => playing.next()?,
        "previous" => playing.previous()?,
        "volume_up" | "volume_down" => {
            let step = if action == "volume_up" {
                VOLUME_STEP
            } else {
                -VOLUME_STEP
            };
            let percent = (f64::from(playing.state().volume) + step).clamp(0.0, 100.0);
            playing.set_volume(percent)?;
            state
                .library
                .lock()
                .unwrap()
                .set_setting("volume", &format!("{percent:.0}"))?;
        }
        _ => return Ok(refuse(404, "no such shortcut")),
    }
    Ok(reply_json(200, json!({ "done": true })))
}

/// Likes the song playing. A song an add-on serves becomes a song of the library first, as a like in the window makes
/// it. It only likes: with no window to show which way it went, a second press must not take the like back.
fn like_playing(state: &State) -> Result<Reply> {
    let Ok(playing) = &state.player else {
        return Ok(refuse(503, "there is no player"));
    };
    let Some(item) = playing.state().track else {
        return Ok(refuse(409, "Nothing is playing"));
    };
    let now = now_ms();
    let mut library = state.library.lock().unwrap();
    let id = match (item.id, &item.addon) {
        (Some(id), _) => id,
        (None, Some(theirs)) => library.adopt(&Song {
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration_ms: item.duration_ms.filter(|ms| *ms > 0),
            refs: vec![(theirs.addon.clone(), theirs.track.clone())],
            art_url: item.art.clone(),
            added_at_ms: Some(now),
            ..Song::default()
        })?,
        (None, None) => return Ok(refuse(409, "That song cannot be kept in your library")),
    };
    library.set_liked(id, true, now)?;
    drop(library);
    state.edited();
    Ok(reply_json(
        200,
        json!({ "done": true, "id": id, "title": item.title }),
    ))
}

#[cfg(test)]
mod tests {
    use super::canonical;

    #[test]
    fn keys_are_named_one_way_and_never_take_typing() {
        assert_eq!(
            canonical("alt + ctrl + Home").as_deref(),
            Some("Ctrl+Alt+Home")
        );
        assert_eq!(
            canonical("Shift+Super+KeyA").as_deref(),
            Some("Shift+Super+KeyA")
        );
        assert_eq!(
            canonical("Ctrl+Shift+ArrowUp").as_deref(),
            Some("Ctrl+Shift+ArrowUp")
        );
        assert_eq!(canonical("F13").as_deref(), Some("F13"));
        assert_eq!(canonical("Shift+F24").as_deref(), Some("Shift+F24"));
        // A key alone, or with only Shift, would be taken from every app's typing.
        for taken in ["KeyA", "Shift+KeyA", "F5", "Shift+F12", "Space"] {
            assert_eq!(canonical(taken), None, "{taken}");
        }
        // Not keys at all.
        for wrong in [
            "",
            "Ctrl+",
            "Ctrl+Alt",
            "Ctrl+Hyper+KeyA",
            "Ctrl+Alt+Key A",
            "Ctrl+Alt+€",
        ] {
            assert_eq!(canonical(wrong), None, "{wrong}");
        }
    }
}
