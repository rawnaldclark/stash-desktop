//! Reading the Stash library file (`stash-web-library`, version 1): what the web player writes with "Save a backup"
//! and the Android app writes with "Export for Stash on the web", and sends a computer with its songs over USB
//! ([`read_from_phone`]).
//!
//! The file is someone else's input. Nothing is taken as it stands: every value is rebuilt from the fields the format
//! names, text is cleaned and cut to size, and what does not fit the format is left out (a song without a title, an
//! ISRC of the wrong shape, a cover that is not https). Unknown fields are ignored, as the format asks.

use std::collections::{HashMap, HashSet};

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
    Ok(walk(&parse(bytes)?, now_ms, |_| {}, |_, _| {}))
}

/// Where a like was made, as a phone says it: in Stash itself, or on a service it syncs with. In this order.
pub const LIKED_ON: [&str; 4] = ["stash", "spotify", "youtube", "soundcloud"];
/// The services a phone syncs playlists from.
pub const FROM: [&str; 3] = ["spotify", "youtube", "soundcloud"];
/// The kinds of list a phone says a playlist is.
pub const KINDS: [&str; 5] = ["playlist", "album", "mix", "radio", "stash_mix"];

/// The library Stash for Android sends a computer beside its songs over USB (`stash-library.json`): the same file,
/// with more. The send it goes with (its list's `sent_at_ms`), where each like was made, which playlists the phone
/// synced from a service, and what kind of list each is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FromPhone {
    pub library: Incoming,
    pub sent_at_ms: Option<i64>,
    /// For each of `library.likes`, in its order: where it was made (of [`LIKED_ON`], in that order). None said for a
    /// like of an older file.
    pub liked_on: Vec<Vec<&'static str>>,
    /// By playlist (its `external_id`): the service the phone synced it from (of [`FROM`]). Its own playlists, its
    /// Stash Mixes and shared mixes have none.
    pub from: HashMap<String, &'static str>,
    /// By playlist: what kind of list it is (of [`KINDS`]), when the phone says.
    pub kind: HashMap<String, &'static str>,
}

/// `value` when it is one of `known`.
fn one_of(value: Option<&Value>, known: &[&'static str]) -> Option<&'static str> {
    let value = value?.as_str()?;
    known.iter().copied().find(|k| *k == value)
}

/// Reads the library a phone sent with its songs: read as [`read`] reads the file, with its send, where each like was
/// made, and where each of its playlists came from and what kind it is. What this does not know is left out.
pub fn read_from_phone(bytes: &[u8], now_ms: i64) -> Result<FromPhone, ImportError> {
    let file = parse(bytes)?;
    let (mut liked_on, mut from, mut kind) = (Vec::new(), HashMap::new(), HashMap::new());
    let library = walk(
        &file,
        now_ms,
        |raw| {
            let said = raw.get("liked_on").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let on: Vec<&'static str> = LIKED_ON
                .into_iter()
                .filter(|service| said.iter().any(|s| s.as_str() == Some(service)))
                .collect();
            liked_on.push(on);
        },
        |raw, playlist| {
            if let Some(service) = one_of(raw.get("from"), &FROM) {
                from.insert(playlist.external_id.clone(), service);
            }
            if let Some(k) = one_of(raw.get("kind"), &KINDS) {
                kind.insert(playlist.external_id.clone(), k);
            }
        },
    );
    Ok(FromPhone {
        library,
        sent_at_ms: file.get("sent_at_ms").and_then(Value::as_i64),
        liked_on,
        from,
        kind,
    })
}

/// The file's JSON, when it is a library file of this version (and not too big to be one).
fn parse(bytes: &[u8]) -> Result<Value, ImportError> {
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
    Ok(file)
}

/// What the file holds. `each_like` is shown the JSON of every like kept, in order; `each_playlist` every playlist kept,
/// beside the JSON it was read from.
fn walk(
    file: &Value,
    now_ms: i64,
    mut each_like: impl FnMut(&Value),
    mut each_playlist: impl FnMut(&Value, &Playlist),
) -> Incoming {
    let list = |name: &str| {
        file.get(name)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };

    let mut likes = Vec::new();
    for like in list("likes").iter().take(MAX_LIKES) {
        let Some(item) = song(like.get("item")) else {
            continue;
        };
        let liked_at = time(like.get("likedAt"))
            .or(item.added_at_ms)
            .unwrap_or(now_ms);
        each_like(like);
        likes.push((item, liked_at));
    }

    // Two playlists with one id: the first wins.
    let mut seen = HashSet::new();
    let mut playlists = Vec::new();
    for raw in list("playlists").iter().take(MAX_PLAYLISTS) {
        if let Some(p) = playlist(raw).filter(|p| seen.insert(p.external_id.clone())) {
            each_playlist(raw, &p);
            playlists.push(p);
        }
    }

    let mut plays: Vec<(Song, i64)> = list("history")
        .iter()
        .filter_map(|play| Some((song(play.get("item"))?, time(play.get("playedAt"))?)))
        .collect();
    plays.sort_by_key(|(_, played_at)| std::cmp::Reverse(*played_at));

    Incoming {
        source: "web-library",
        generator: text(file.get("generator"), NAME),
        likes,
        playlists,
        plays,
        songs: Vec::new(),
        blocked: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library a phone sent with its send made at 7: its own playlist, one synced from Spotify, one from YouTube
    /// Music, one from somewhere this does not know, and the Spotify one again under its id.
    const SENT: &str = r#"{"kind":"stash-web-library","v":1,"sent_at_ms":7,
        "likes":[{"item":{"title":"Halo","artist":"Beyoncé"},"likedAt":1759838400200}],
        "playlists":[
            {"id":"app-2","name":"Night drive","items":[{"title":"Ivy","artist":"Frank Ocean"}]},
            {"id":"bad id!","name":"Not kept","from":"spotify","items":[]},
            {"id":"app-9","name":"Daily Mix 1","from":"spotify","items":[]},
            {"id":"app-9","name":"Daily Mix 1 again","from":"youtube","items":[]},
            {"id":"app-12","name":"Supermix","from":"youtube","items":[]},
            {"id":"app-13","name":"Odd","from":"tidal","items":[]}],
        "history":[{"item":{"title":"Ivy","artist":"Frank Ocean"},"playedAt":1759838402000}]}"#;

    #[test]
    fn a_library_sent_by_a_phone_says_its_send_and_where_its_playlists_came_from() {
        let sent = read_from_phone(SENT.as_bytes(), 1).unwrap();
        assert_eq!(sent.sent_at_ms, Some(7));
        assert_eq!(
            sent.from,
            HashMap::from([("app-9".to_string(), "spotify"), ("app-12".to_string(), "youtube")]),
            "the first of one id wins, as its playlist does; a service not known is none"
        );
        // Read as the web file is: the same likes, playlists and plays.
        assert_eq!(sent.library, read(SENT.as_bytes(), 1).unwrap());
        let names: Vec<&str> = sent.library.playlists.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Night drive", "Daily Mix 1", "Supermix", "Odd"]);
        // A file without where its likes were made or what kind its lists are (an older phone's): none said of either.
        assert_eq!(sent.liked_on, [Vec::<&str>::new()]);
        assert!(sent.kind.is_empty());
        // The web file has neither: no send, and no playlist from anywhere.
        let web = read_from_phone(br#"{"kind":"stash-web-library","v":1,"playlists":[{"id":"a","name":"A","items":[]}]}"#, 1).unwrap();
        assert_eq!((web.sent_at_ms, web.from.len()), (None, 0));
        assert!(matches!(read_from_phone(b"{}", 1), Err(ImportError::NotStash)));
    }

    #[test]
    fn a_phone_says_where_each_like_was_made_and_what_kind_each_list_is() {
        let sent = read_from_phone(
            br#"{"kind":"stash-web-library","v":1,"sent_at_ms":7,
            "likes":[
                {"item":{"title":"Halo","artist":"Beyonce"},"liked_on":["youtube","spotify","tidal","spotify"]},
                {"item":{"title":"No title"}, "liked_on":["stash"]},
                {"item":{"title":"Ivy","artist":"Frank Ocean"},"liked_on":"stash"},
                {"item":{"title":"Nights","artist":"Frank Ocean"},"liked_on":["stash","soundcloud"]}],
            "playlists":[
                {"id":"spotify-album-4yP0hdKOZPNshxUOjY0cZj","name":"After Hours","from":"spotify","kind":"album","items":[]},
                {"id":"stash-mix-3","name":"Stash Mix 3","kind":"stash_mix","items":[]},
                {"id":"soundcloud-1234","name":"Related tracks","from":"soundcloud","kind":"radio","items":[]},
                {"id":"custom_1","name":"Mine","kind":"folder","items":[]}]}"#,
            1,
        )
        .unwrap();
        // Unknown services dropped, each once, in their order; a like left out (no artist) has none; one whose list
        // is not a list has none.
        assert_eq!(sent.liked_on, [vec!["spotify", "youtube"], vec![], vec!["stash", "soundcloud"]]);
        assert_eq!(sent.library.likes.len(), 3);
        assert_eq!(
            sent.from,
            HashMap::from([
                ("spotify-album-4yP0hdKOZPNshxUOjY0cZj".to_string(), "spotify"),
                ("soundcloud-1234".to_string(), "soundcloud"),
            ])
        );
        assert_eq!(
            sent.kind,
            HashMap::from([
                ("spotify-album-4yP0hdKOZPNshxUOjY0cZj".to_string(), "album"),
                ("stash-mix-3".to_string(), "stash_mix"),
                ("soundcloud-1234".to_string(), "radio"),
            ]),
            "a kind not known is none"
        );
    }
}
