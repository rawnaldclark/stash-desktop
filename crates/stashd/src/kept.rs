//! The queue is kept between runs: what was queued, the place in it and the seconds into the track, shuffle and
//! repeat. A program that is closed (or loses power) in the middle of an album opens on that album, paused where
//! it was.
//!
//! It lives in the library's settings as two values. `queue` holds the tracks and is written when the queue
//! changes; `queue.place` is small and is written as the place moves (every few seconds while a track plays). A
//! song of the library is kept by its row; an add-on's track is kept whole, as the add-on told it.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{Value, json};
use stash_core::Library;
use stash_core::player::{AddonTrack, Kept, Player, QueueItem, Repeat};

const QUEUE: &str = "queue";
const PLACE: &str = "queue.place";
/// While a track plays its place is kept to this many milliseconds: a write every few seconds, not on every look.
/// Paused, it is kept to the second (it does not move, so that is one write).
const STEP_MS: u64 = 5_000;

fn kept_item(item: &QueueItem) -> Value {
    match (&item.addon, item.id) {
        (None, Some(id)) => json!(id),
        (Some(addon), _) => json!({
            "addon": addon.addon,
            "track": addon.track,
            "title": item.title,
            "artist": item.artist,
            "album": item.album,
            "art": item.art,
            "duration_ms": item.duration_ms,
        }),
        // Not a song of the library and not an add-on's: nothing to find it by next time.
        (None, None) => Value::Null,
    }
}

fn addon_item(value: &Value) -> Option<QueueItem> {
    let text = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_string);
    Some(QueueItem {
        id: None,
        title: text("title").unwrap_or_default(),
        artist: text("artist").unwrap_or_default(),
        album: text("album"),
        addon: Some(AddonTrack {
            addon: text("addon").filter(|a| !a.is_empty())?,
            track: text("track").filter(|t| !t.is_empty())?,
        }),
        art: text("art"),
        duration_ms: value.get("duration_ms").and_then(Value::as_u64),
        qid: 0,
        locator: String::new(),
    })
}

/// Writes the queue down when it changed since the last look.
#[derive(Default)]
pub struct Keeper {
    version: Option<u64>,
    place: Option<String>,
}

impl Keeper {
    /// Looks at the player and writes down what changed. Called about once a second.
    pub fn look(&mut self, library: &Mutex<Library>, player: &Player) {
        let state = player.state();
        if self.version != Some(state.queue_version) {
            let (version, items, original) = player.keep();
            let queue = json!({
                "v": 1,
                "items": items.iter().map(kept_item).collect::<Vec<_>>(),
                "original": original,
            });
            if library
                .lock()
                .unwrap()
                .set_setting(QUEUE, &queue.to_string())
                .is_ok()
            {
                self.version = Some(version);
            }
        }
        // Playing: the last step passed (never ahead of the music). Paused: the nearest second.
        let (step, lean) = if state.status == "playing" {
            (STEP_MS, 0)
        } else {
            (1_000, 500)
        };
        let place = json!({
            "place": state.index,
            "seconds": ((state.position_ms + lean) / step * step) as f64 / 1000.0,
            "shuffle": state.shuffle,
            "repeat": state.repeat,
            "from": state.from,
        })
        .to_string();
        if self.place.as_deref() != Some(&place)
            && library.lock().unwrap().set_setting(PLACE, &place).is_ok()
        {
            self.place = Some(place);
        }
    }
}

/// Brings back the queue that was kept: the player rests where it was left, and nothing plays until the listener
/// says so. A song the library no longer has (or whose file is gone) is left out. Shuffle and repeat are set as
/// they were, also with nothing queued.
pub fn restore(library: &Mutex<Library>, player: &Player) {
    let (queue, place) = {
        let library = library.lock().unwrap();
        let read = |key: &str| {
            library
                .setting(key)
                .ok()
                .flatten()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        };
        (read(QUEUE), read(PLACE).unwrap_or(Value::Null))
    };
    let repeat = Repeat::from_word(place["repeat"].as_str().unwrap_or("off"));
    let shuffle = place["shuffle"].as_bool().unwrap_or(false);
    let entries = queue
        .as_ref()
        .and_then(|q| q["items"].as_array())
        .cloned()
        .unwrap_or_default();
    if entries.is_empty() {
        player.set_repeat(repeat);
        player.set_shuffle(shuffle);
        return;
    }

    let mut ids: Vec<i64> = entries.iter().filter_map(Value::as_i64).collect();
    ids.sort_unstable();
    ids.dedup();
    let found: HashMap<i64, QueueItem> = library
        .lock()
        .unwrap()
        .queue_items(&ids)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| item.id.map(|id| (id, item)))
        .collect();
    // Where each kept entry is in the queue brought back; an entry left out points at the one after it.
    let mut now_at = Vec::with_capacity(entries.len());
    let mut here = Vec::with_capacity(entries.len());
    let mut items = Vec::with_capacity(entries.len());
    for entry in &entries {
        let item = match entry.as_i64() {
            Some(id) => found.get(&id).cloned(),
            None => addon_item(entry),
        };
        now_at.push(items.len());
        here.push(item.is_some());
        items.extend(item);
    }
    if items.is_empty() {
        return;
    }
    let was = place["place"].as_u64().unwrap_or(0) as usize;
    let same_track = here.get(was).copied().unwrap_or(false);
    let original = queue
        .as_ref()
        .and_then(|q| q["original"].as_array())
        .map(|order| {
            order
                .iter()
                .filter_map(Value::as_u64)
                .map(|i| i as usize)
                .filter(|i| here.get(*i).copied().unwrap_or(false))
                .map(|i| now_at[i])
                .collect::<Vec<_>>()
        });
    player.restore(Kept {
        place: now_at.get(was).copied().unwrap_or(0).min(items.len() - 1),
        // The seconds belong to the track that was playing: when it is gone, the one after it starts at its start.
        seconds: if same_track {
            place["seconds"].as_f64().unwrap_or(0.0)
        } else {
            0.0
        },
        from: place["from"].as_str().map(str::to_string),
        shuffle,
        original,
        repeat,
        items,
    });
}
