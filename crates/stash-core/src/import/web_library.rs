//! Reading the Stash library file (`stash-web-library`, version 1): what the web player writes with "Save a backup"
//! and the Android app writes with "Export for Stash on the web".
//!
//! The file is someone else's input. Nothing is taken as it stands: every value is rebuilt from the fields the format
//! names, text is cleaned and cut to size, and what does not fit the format is left out (a song without a title, an
//! ISRC of the wrong shape, a cover that is not https). Unknown fields are ignored, as the format asks.

use std::collections::HashSet;

use serde_json::Value;

use super::{ImportError, Incoming, Playlist, Song};
use crate::identity::{clean_text, isrc};

/// The format's own limits.
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_LIKES: usize = 50_000;
const MAX_PLAYLISTS: usize = 2_000;
const MAX_PLAYLIST_ITEMS: usize = 10_000;
const MAX_REFS: usize = 16;
const TEXT: usize = 300;
const NAME: usize = 100;
const ID: usize = 200;
const URL: usize = 2_048;
const MAX_DURATION_MS: f64 = 24.0 * 3_600_000.0;

/// A time in the file: milliseconds since 1970, a positive number.
fn time(value: Option<&Value>) -> Option<i64> {
    let ms = value?.as_f64()?;
    (ms.is_finite() && ms > 0.0 && ms < 1e14).then(|| ms.round() as i64)
}

fn text(value: Option<&Value>, max: usize) -> Option<String> {
    let cleaned = clean_text(value?.as_str()?, max);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn made_of(text: &str, lengths: std::ops::RangeInclusive<usize>, also: &[u8]) -> bool {
    lengths.contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || also.contains(&b))
}

/// A cover's address: https, of a sane length, with nothing odd in it.
fn cover(value: &Value) -> Option<String> {
    let address = value.get("url")?.as_str()?;
    let plain = !address.is_empty()
        && address.len() <= URL
        && !address
            .chars()
            .any(|c| c.is_control() || c == ' ' || c == '\\');
    let parsed = url::Url::parse(address).ok()?;
    (plain && parsed.scheme() == "https" && parsed.host_str().is_some())
        .then(|| address.to_string())
}

fn song(value: Option<&Value>) -> Option<Song> {
    let value = value?;
    let title = text(value.get("title"), TEXT)?;
    let artist = text(value.get("artist"), TEXT)?;
    let mut refs: Vec<(String, String)> = Vec::new();
    if let Some(given) = value.get("refs").and_then(Value::as_object) {
        for (source, theirs) in given {
            let Some(theirs) = text(Some(theirs), ID) else {
                continue;
            };
            if made_of(source, 1..=64, b"._-") && refs.len() < MAX_REFS {
                refs.push((source.clone(), theirs));
            }
        }
    }
    // The Spotify id rides beside the refs in the file; here it is one more name the song goes by.
    if let Some(spotify) = value.get("spotifyId").and_then(Value::as_str)
        && made_of(spotify, 22..=22, b"")
        && !refs.iter().any(|(source, _)| source == "spotify")
    {
        refs.push(("spotify".into(), spotify.to_string()));
    }
    Some(Song {
        title,
        artist,
        album: text(value.get("album"), TEXT),
        duration_ms: value
            .get("durationMs")
            .and_then(Value::as_f64)
            .filter(|ms| ms.is_finite() && *ms > 0.0 && *ms <= MAX_DURATION_MS)
            .map(|ms| ms.round() as u64),
        isrc: value.get("isrc").and_then(Value::as_str).and_then(isrc),
        explicit: None,
        refs,
        art_url: value
            .get("artwork")
            .and_then(Value::as_array)
            .and_then(|covers| covers.iter().take(8).find_map(cover)),
        added_at_ms: time(value.get("addedAt")),
    })
}

fn playlist(value: &Value) -> Option<Playlist> {
    let id = value.get("id")?.as_str()?;
    if !made_of(id, 1..=64, b"_-") {
        return None;
    }
    let name = text(value.get("name"), NAME)?;
    let items = value
        .get("items")?
        .as_array()?
        .iter()
        .take(MAX_PLAYLIST_ITEMS)
        .filter_map(|item| song(Some(item)))
        .collect();
    let created_at_ms = time(value.get("createdAt"));
    // A shared mix someone else owns: an 8-character share id and a version of 1 or more.
    let follow = value.get("follow").and_then(|follow| {
        let id = follow.get("id")?.as_str()?;
        let version = follow.get("version")?.as_i64()?;
        (made_of(id, 8..=8, b"") && version >= 1)
            .then(|| (id.to_string(), version, text(follow.get("sharedBy"), 40)))
    });
    Some(Playlist {
        external_id: id.to_string(),
        name,
        items,
        created_at_ms,
        updated_at_ms: time(value.get("updatedAt")).or(created_at_ms),
        follow_id: follow.as_ref().map(|(id, _, _)| id.clone()),
        follow_version: follow.as_ref().map(|(_, version, _)| *version),
        follow_shared_by: follow.and_then(|(_, _, by)| by),
        // A smart playlist's rules, from another desktop Stash; rules that cannot be used make it a plain playlist.
        rules: value
            .get("rules")
            .and_then(|rules| crate::smart::Rules::read(rules).ok()),
    })
}

/// Reads the file's bytes. `now_ms` stands in for a time the file leaves out.
pub fn read(bytes: &[u8], now_ms: i64) -> Result<Incoming, ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError::TooBig);
    }
    let file: Value = serde_json::from_slice(bytes).map_err(|_| ImportError::NotStash)?;
    if file.get("kind").and_then(Value::as_str) != Some("stash-web-library") {
        return Err(ImportError::NotStash);
    }
    if file.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(ImportError::Newer);
    }
    let list = |name: &str| {
        file.get(name)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };

    let likes = list("likes")
        .iter()
        .take(MAX_LIKES)
        .filter_map(|like| {
            let item = song(like.get("item"))?;
            let liked_at = time(like.get("likedAt"))
                .or(item.added_at_ms)
                .unwrap_or(now_ms);
            Some((item, liked_at))
        })
        .collect();

    // Two playlists with one id: the first wins.
    let mut seen = HashSet::new();
    let playlists = list("playlists")
        .iter()
        .take(MAX_PLAYLISTS)
        .filter_map(playlist)
        .filter(|p| seen.insert(p.external_id.clone()))
        .collect();

    let mut plays: Vec<(Song, i64)> = list("history")
        .iter()
        .filter_map(|play| Some((song(play.get("item"))?, time(play.get("playedAt"))?)))
        .collect();
    plays.sort_by_key(|(_, played_at)| std::cmp::Reverse(*played_at));

    Ok(Incoming {
        source: "web-library",
        generator: text(file.get("generator"), NAME),
        likes,
        playlists,
        plays,
        songs: Vec::new(),
        blocked: Vec::new(),
    })
}
