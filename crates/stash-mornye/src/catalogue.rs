//! An add-on's catalogue: its albums, artists and playlists, for an extension that is a metadata provider (manifest
//! `type` names `metadata_provider`; SpotiFLAC's extension interface, which the official Mornye extensions follow).
//! It is asked with `customSearch(query, {filter})` for albums or artists, and `getAlbum`, `getArtist` and
//! `getPlaylist` for their pages (an extension without `getPlaylist` is asked `getAlbum`, as Lokal asks).
//!
//! Each answer is read the way SpotiFLAC reads it: an album `{id, name, artists, artist_id, release_date, album_type,
//! total_tracks, cover_url | images, tracks}`, an artist `{id, name, image_url | images, albums | releases,
//! top_tracks}`, a playlist `{id, name, owner, cover_url | images, tracks}`; a search answer is a list (or `{items}`,
//! `{results}`, `{albums}`, `{artists}`) whose rows say what they are in `item_type`.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::manifest::{Manifest, text_of};
use crate::track::{MTrack, artists, artwork, first, image, present, whole, year};

const MAX_NAME: usize = 500;

/// What a catalogue is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Album,
    Artist,
    Playlist,
}

impl Kind {
    /// Its name in a row's `item_type`.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Album => "album",
            Kind::Artist => "artist",
            Kind::Playlist => "playlist",
        }
    }

    /// The extension's function for its page.
    pub fn getter(self) -> &'static str {
        match self {
            Kind::Album => "getAlbum",
            Kind::Artist => "getArtist",
            Kind::Playlist => "getPlaylist",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MAlbum {
    pub id: String,
    pub name: String,
    /// Joined with ", ".
    pub artists: String,
    pub artist_id: Option<String>,
    pub year: Option<String>,
    /// `album`, `single`, `ep` or `compilation`, as the add-on says (else `album`).
    pub kind: String,
    pub total_tracks: Option<u32>,
    pub artwork: Option<String>,
    /// Its songs, on its own page; none in a list of albums.
    pub tracks: Vec<MTrack>,
}

impl MAlbum {
    /// A single or an EP: shown apart from the albums.
    pub fn single(&self) -> bool {
        matches!(self.kind.as_str(), "single" | "ep")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MArtist {
    pub id: String,
    pub name: String,
    pub artwork: Option<String>,
    pub albums: Vec<MAlbum>,
    pub top_tracks: Vec<MTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MPlaylist {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub artwork: Option<String>,
    pub tracks: Vec<MTrack>,
}

/// Whether the extension says it is a metadata provider: its albums and artists can be asked for.
pub fn provides_catalogue(manifest: &Manifest) -> bool {
    let named = |kind: &Value| kind.as_str() == Some("metadata_provider");
    match manifest.raw.get("type") {
        Some(Value::Array(kinds)) => kinds.iter().any(named),
        Some(kind) => named(kind),
        None => false,
    }
}

/// The search filter the extension names for `kind` (`album` or `albums`, ...), or the plain word when it names none.
pub fn filter_for(manifest: &Manifest, kind: Kind) -> Value {
    let word = kind.word();
    let plural = format!("{word}s");
    manifest
        .raw
        .pointer("/searchBehavior/filters")
        .and_then(Value::as_array)
        .and_then(|filters| {
            filters.iter().find_map(|filter| {
                let id = filter.as_object()?.get("id")?;
                let named = text_of(Some(id)).to_ascii_lowercase();
                (named == word || named == plural).then(|| id.clone())
            })
        })
        .unwrap_or_else(|| Value::String(word.to_string()))
}

/// The rows of a search answer that are `kind` (a row that does not say what it is counts, as the search asked for
/// that kind).
pub fn found_rows(result: &Value, kind: Kind) -> Vec<Value> {
    let list = match result {
        Value::Array(list) => list.clone(),
        Value::Object(map) => ["items", "results", &format!("{}s", kind.word())]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_array).cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    list.into_iter()
        .filter(|row| {
            row.get("item_type")
                .and_then(Value::as_str)
                .is_none_or(|said| said.eq_ignore_ascii_case(kind.word()))
        })
        .collect()
}

fn name_of(map: &Map<String, Value>) -> Option<String> {
    let name = text_of(present(map, "name").or_else(|| present(map, "title")));
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    (!name.is_empty()).then_some(name)
}

fn id_of(map: &Map<String, Value>) -> Option<String> {
    let id = text_of(Some(present(map, "id")?));
    (!id.trim().is_empty()).then_some(id)
}

/// The largest picture of a row: a cover (as a song's is read), or an artist's own picture.
fn picture(map: &Map<String, Value>) -> Option<String> {
    let mut best = artwork(map).map(|url| (url, 0.0));
    for key in [
        "image_url",
        "imageUrl",
        "header_image",
        "picture",
        "avatar_url",
    ] {
        if let Some(found) = map.get(key).and_then(image)
            && best.as_ref().is_none_or(|(_, size)| found.1 > *size)
        {
            best = Some(found);
        }
    }
    best.map(|(url, _)| shown_size(url))
}

/// A picture at a size a page shows: SoundCloud's "original" is the uploaded file (often several megabytes), and
/// its 500-pixel one is what a page needs.
fn shown_size(url: String) -> String {
    if url.contains(".sndcdn.com/") && url.contains("-original.") {
        url.replacen("-original.", "-t500x500.", 1)
    } else {
        url
    }
}

fn tracks_in(value: Option<&Value>) -> Vec<MTrack> {
    let list = match value {
        Some(Value::Array(list)) => list.as_slice(),
        Some(Value::Object(map)) => map
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
        _ => &[],
    };
    list.iter()
        .filter_map(|row| MTrack::of(row, true))
        .collect()
}

/// An album as a page or a list gives it. `None` without an id or a name.
pub fn album_of(value: &Value) -> Option<MAlbum> {
    let map = value.as_object()?;
    let kind = text_of(first(
        map,
        &["album_type", "albumType", "release_type", "type"],
    ))
    .to_ascii_lowercase();
    Some(MAlbum {
        id: id_of(map)?,
        name: name_of(map)?,
        artists: artists(map),
        artist_id: first(map, &["artist_id", "artistId"])
            .map(|id| text_of(Some(id)))
            .filter(|id| !id.trim().is_empty()),
        year: year(map),
        kind: match kind.as_str() {
            "single" | "ep" | "compilation" => kind,
            _ => "album".to_string(),
        },
        total_tracks: whole(first(map, &["total_tracks", "totalTracks", "tracks_count"]))
            .filter(|n| *n > 0),
        artwork: picture(map),
        tracks: tracks_in(map.get("tracks")),
    })
}

/// An artist as their page or a list gives them. `None` without an id or a name.
pub fn artist_of(value: &Value) -> Option<MArtist> {
    let map = value.as_object()?;
    let listed = first(map, &["albums", "releases"]);
    let albums = match listed {
        Some(Value::Array(list)) => list.iter().filter_map(album_of).collect(),
        Some(Value::Object(page)) => page
            .get("items")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(album_of).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    Some(MArtist {
        id: id_of(map)?,
        name: name_of(map)?,
        artwork: picture(map),
        albums: dedupe(albums),
        top_tracks: tracks_in(first(map, &["top_tracks", "topTracks"])),
    })
}

/// A playlist as its page gives it. `None` without an id or a name.
pub fn playlist_of(value: &Value) -> Option<MPlaylist> {
    let map = value.as_object()?;
    let owner = match first(map, &["owner", "owner_name", "author", "creator"]) {
        Some(Value::Object(owner)) => {
            name_of(owner).or_else(|| owner.get("display_name").map(|n| text_of(Some(n))))
        }
        Some(other) => Some(text_of(Some(other))),
        None => None,
    }
    .filter(|owner| !owner.trim().is_empty());
    Some(MPlaylist {
        id: id_of(map)?,
        name: name_of(map)?,
        owner,
        artwork: picture(map),
        tracks: tracks_in(map.get("tracks")),
    })
}

/// The same release listed twice (the same id) once.
fn dedupe(albums: Vec<MAlbum>) -> Vec<MAlbum> {
    let mut seen = std::collections::HashSet::new();
    albums
        .into_iter()
        .filter(|album| seen.insert(album.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Shaped as the official Qobuz extension (qobuz-web 1.2.27) answers.
    fn qobuz_album() -> Value {
        json!({
            "id": "qobuz:0060254795614", "name": "Culture", "artists": "Migos", "artist_id": "qobuz:487452",
            "cover_url": "https://static.qobuz.com/images/covers/14/56/0060254795614_600.jpg",
            "release_date": "2017-01-27", "total_tracks": 13, "album_type": "album", "item_type": "album",
            "tracks": [
                { "id": "qobuz:40981735", "name": "T-Shirt", "artists": "Migos", "album_name": "Culture",
                  "duration_ms": 242000, "track_number": 2, "isrc": "USUM71700626", "item_type": "track" },
                { "id": "qobuz:40981736", "name": "Call Casting", "artists": "Migos", "duration_ms": 226000 }
            ]
        })
    }

    #[test]
    fn an_album_page_is_read_with_its_songs() {
        let album = album_of(&qobuz_album()).unwrap();
        assert_eq!(
            (
                album.id.as_str(),
                album.name.as_str(),
                album.artists.as_str()
            ),
            ("qobuz:0060254795614", "Culture", "Migos")
        );
        assert_eq!(
            (
                album.artist_id.as_deref(),
                album.year.as_deref(),
                album.total_tracks
            ),
            (Some("qobuz:487452"), Some("2017"), Some(13))
        );
        assert!(!album.single());
        assert!(
            album
                .artwork
                .as_deref()
                .is_some_and(|a| a.ends_with("_600.jpg"))
        );
        assert_eq!(album.tracks.len(), 2);
        assert_eq!(album.tracks[0].name, "T-Shirt");
        assert_eq!(album.tracks[1].duration_ms, Some(226_000));
    }

    #[test]
    fn an_artist_page_is_read_with_all_their_releases() {
        let mut culture = qobuz_album();
        culture["tracks"] = json!([]);
        let artist = artist_of(&json!({
            "id": "qobuz:487452", "name": "Migos", "item_type": "artist",
            "image_url": "https://static.qobuz.com/images/artists/covers/large/abc.jpg",
            "albums": [
                culture.clone(),
                { "id": "qobuz:1", "name": "Bad and Boujee", "artists": "Migos", "album_type": "single",
                  "release_date": "2016-10-28", "total_tracks": 1 },
                { "id": "qobuz:2", "name": "Culture III", "artists": "Migos", "album_type": "ep" },
                culture
            ],
            "top_tracks": [{ "id": "qobuz:9", "name": "Walk It Talk It", "artists": "Migos, Drake" }]
        }))
        .unwrap();
        assert_eq!(
            (artist.id.as_str(), artist.name.as_str()),
            ("qobuz:487452", "Migos")
        );
        assert!(
            artist
                .artwork
                .as_deref()
                .is_some_and(|a| a.ends_with("abc.jpg"))
        );
        // The same release twice is listed once; singles and EPs are told apart.
        let names: Vec<(&str, bool)> = artist
            .albums
            .iter()
            .map(|a| (a.name.as_str(), a.single()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Culture", false),
                ("Bad and Boujee", true),
                ("Culture III", true)
            ]
        );
        assert_eq!(artist.top_tracks.len(), 1);

        // Releases under `releases`, or a page of them, are read too.
        let paged = artist_of(&json!({ "id": "a", "name": "A", "releases": { "items": [{ "id": "r", "name": "R" }] } }))
            .unwrap();
        assert_eq!(paged.albums.len(), 1);
    }

    #[test]
    fn a_search_answer_gives_only_what_was_asked_for() {
        let answer = json!([
            { "id": "t", "name": "Song", "item_type": "track" },
            { "id": "qobuz:487452", "name": "Migos", "image_url": "https://x.example/m.jpg", "item_type": "artist" },
            { "id": "qobuz:1", "name": "Culture", "artists": "Migos", "item_type": "album" }
        ]);
        let artists: Vec<String> = found_rows(&answer, Kind::Artist)
            .iter()
            .filter_map(artist_of)
            .map(|a| a.name)
            .collect();
        assert_eq!(artists, vec!["Migos"]);
        assert_eq!(found_rows(&answer, Kind::Album).len(), 1);
        // A wrapped answer whose rows do not say what they are.
        let wrapped = json!({ "albums": [{ "id": "1", "name": "X" }] });
        assert_eq!(found_rows(&wrapped, Kind::Album).len(), 1);
        assert!(found_rows(&json!("nothing"), Kind::Album).is_empty());
    }

    #[test]
    fn a_playlist_names_its_owner() {
        let playlist = playlist_of(&json!({
            "id": "p", "name": "Hip-Hop Essentials", "owner": { "name": "Qobuz" },
            "tracks": [{ "id": "1", "name": "One", "artists": "A" }]
        }))
        .unwrap();
        assert_eq!(
            (playlist.owner.as_deref(), playlist.tracks.len()),
            (Some("Qobuz"), 1)
        );
        assert_eq!(
            playlist_of(&json!({ "id": "p", "name": "P", "owner": "Me" }))
                .unwrap()
                .owner
                .as_deref(),
            Some("Me")
        );
    }

    #[test]
    fn rows_without_an_id_or_a_name_are_not_read() {
        assert!(album_of(&json!({ "name": "No id" })).is_none());
        assert!(artist_of(&json!({ "id": "1" })).is_none());
        assert!(playlist_of(&json!(null)).is_none());
    }
}
