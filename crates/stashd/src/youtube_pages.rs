//! YouTube Music's own pages, for the window: everything a search finds beside the songs (albums, artists,
//! playlists, and what it puts first), and the pages of an album, an artist and a playlist.
//!
//! `GET /api/youtube/search?q=`, `/api/youtube/album?id=`, `/api/youtube/artist?id=`, `/api/youtube/playlist?id=`.
//! Each asks YouTube once (a long playlist: once for every hundred songs). Not found when this core has no YouTube
//! Music or the listener switched it off; refused while Stash is offline.

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::addons::{AddonError, Code};
use stash_core::youtube::pages::{AlbumCard, ArtistCard, PlaylistCard, Top};
use stash_core::youtube::parse::Song;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, offline, refuse, reply_json};

fn song(song: &Song) -> Value {
    json!({
        "id": song.id,
        "title": song.title,
        "artist": song.artist,
        "album": song.album,
        "duration_ms": song.duration_ms,
        "art": song.art,
        "explicit": song.explicit,
        "artist_id": song.artist_id,
        "album_id": song.album_id,
    })
}

fn album(album: &AlbumCard) -> Value {
    json!({
        "id": album.id,
        "title": album.title,
        "artist": album.artist,
        "artist_id": album.artist_id,
        "year": album.year,
        "single": album.single,
        "art": album.art,
    })
}

fn artist(artist: &ArtistCard) -> Value {
    json!({ "id": artist.id, "name": artist.name, "art": artist.art })
}

fn playlist(playlist: &PlaylistCard) -> Value {
    json!({ "id": playlist.id, "title": playlist.title, "author": playlist.author, "art": playlist.art })
}

fn songs(list: &[Song]) -> Vec<Value> {
    list.iter()
        .filter(|s| !s.artist.is_empty())
        .map(song)
        .collect()
}

/// Why YouTube gave no answer, as the window is told.
fn failed(e: &AddonError) -> Reply {
    let status = match e.code {
        Code::NotFound => 404,
        Code::RateLimited => 429,
        _ => 502,
    };
    refuse(status, &e.in_words())
}

pub(crate) fn route(
    state: &State,
    request: &mut Request,
    path: &str,
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Reply>> {
    if !path.starts_with("/api/youtube/") || *request.method() != Method::Get {
        return Ok(None);
    }
    let Some(youtube) = state.youtube_on() else {
        return Ok(Some(refuse(404, "YouTube Music is switched off")));
    };
    if offline(state) {
        return Ok(Some(refuse(
            409,
            "Stash is offline: YouTube Music is not asked",
        )));
    }
    let id = param("id").unwrap_or_default();
    let answer = match path {
        "/api/youtube/search" => youtube.found(&param("q").unwrap_or_default()).map(|found| {
            let top = found.top.as_ref().map(|top| match top {
                Top::Artist(card) => json!({ "kind": "artist", "artist": artist(card) }),
                Top::Album(card) => json!({ "kind": "album", "album": album(card) }),
                Top::Song(card) => json!({ "kind": "song", "song": song(card) }),
            });
            json!({
                "top": top,
                "albums": found.albums.iter().map(album).collect::<Vec<_>>(),
                "artists": found.artists.iter().map(artist).collect::<Vec<_>>(),
                "playlists": found.playlists.iter().map(playlist).collect::<Vec<_>>(),
            })
        }),
        "/api/youtube/album" => youtube.album(&id).map(|page| {
            json!({
                "id": id,
                "title": page.title,
                "artist": page.artist,
                "artist_id": page.artist_id,
                "year": page.year,
                "single": page.single,
                "art": page.art,
                "songs": songs(&page.songs),
            })
        }),
        "/api/youtube/artist" => youtube.artist(&id).map(|page| {
            json!({
                "id": id,
                "name": page.name,
                "art": page.art,
                "songs": songs(&page.songs),
                "albums": page.albums.iter().map(album).collect::<Vec<_>>(),
                "singles": page.singles.iter().map(album).collect::<Vec<_>>(),
                "related": page.related.iter().map(artist).collect::<Vec<_>>(),
            })
        }),
        "/api/youtube/playlist" => youtube.playlist(&id).map(|page| {
            json!({
                "id": id,
                "title": page.title,
                "author": page.author,
                "art": page.art,
                "songs": songs(&page.songs),
                // More than was read: the list is longer than Stash reads through.
                "more": page.more.is_some(),
            })
        }),
        _ => return Ok(None),
    };
    Ok(Some(match answer {
        Ok(value) => reply_json(200, value),
        Err(e) => failed(&e),
    }))
}
