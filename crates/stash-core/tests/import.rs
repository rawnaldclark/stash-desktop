//! Importing a library from another Stash. The fixture is the golden file the Android app's tests write and the web
//! player's tests read (`stash-web-library` v1): the same bytes, so all three readers agree on one file.

use stash_core::import::web_library::read;
use stash_core::import::{ImportError, Summary};
use stash_core::{Library, NewTrack, Origin};

const GOLDEN: &[u8] = include_bytes!("fixtures/app-export-v1.json");
const NOW_MS: i64 = 1_800_000_000_000;

fn file_track(title: &str, artist: &str, path: &str) -> NewTrack {
    NewTrack {
        title: title.into(),
        artist: artist.into(),
        album_artist: None,
        album: None,
        year: None,
        disc_no: 1,
        track_no: 0,
        duration_ms: 0,
        isrc: None,
        origin: Origin::File,
        addon_id: None,
        locator: path.into(),
        codec: Some("flac".into()),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        added_at: 1_700_000_000,
        file_size: Some(1),
        file_mtime: Some(1),
    }
}

/// A library file with these likes: (title, artist, isrc).
fn likes_file(likes: &[(&str, &str, Option<&str>)]) -> Vec<u8> {
    let likes: Vec<serde_json::Value> = likes
        .iter()
        .enumerate()
        .map(|(n, (title, artist, isrc))| {
            let mut item = serde_json::json!({ "title": title, "artist": artist });
            if let Some(isrc) = isrc {
                item["isrc"] = serde_json::json!(isrc);
            }
            serde_json::json!({ "item": item, "likedAt": 1_759_838_400_000i64 + n as i64 })
        })
        .collect();
    serde_json::json!({ "kind": "stash-web-library", "v": 1, "likes": likes })
        .to_string()
        .into_bytes()
}

#[test]
fn the_golden_file_reads_as_the_other_stashes_read_it() {
    let file = read(GOLDEN, NOW_MS).unwrap();
    assert_eq!(
        (file.source, file.generator.as_deref()),
        ("web-library", Some("Stash for Android (golden)"))
    );
    assert_eq!(
        (file.likes.len(), file.playlists.len(), file.plays.len()),
        (3, 2, 3)
    );

    let (pink, liked_at) = &file.likes[1];
    assert_eq!(
        (
            pink.title.as_str(),
            pink.artist.as_str(),
            pink.album.as_deref()
        ),
        ("Pink + White", "Frank Ocean", Some("Blonde"))
    );
    assert_eq!(
        (pink.duration_ms, pink.isrc.as_deref(), *liked_at),
        (Some(184_000), Some("USUM71607009"), 1_759_838_400_100)
    );
    // The YouTube id and the Spotify id are both names the song goes by.
    assert_eq!(
        pink.refs,
        [
            ("youtube".to_string(), "uxpDa-c-4Mc".to_string()),
            ("spotify".to_string(), "3xKsf9qdS1CyvXSMEid6g8".to_string())
        ]
    );
    assert_eq!(
        pink.art_url.as_deref(),
        Some("https://i.scdn.co/image/ab67616d0000b273c5649add07ed3720be9d5526")
    );

    let (drive, mix) = (&file.playlists[0], &file.playlists[1]);
    assert_eq!(
        (
            drive.external_id.as_str(),
            drive.name.as_str(),
            drive.items.len(),
            drive.follow_id.as_deref()
        ),
        ("app-2", "Night drive", 2, None)
    );
    assert_eq!(
        (mix.name.as_str(), mix.follow_id.as_deref()),
        ("Robin’s mix", Some("Fw12ab34"))
    );
    // Plays come newest first.
    assert_eq!(
        file.plays
            .iter()
            .map(|(s, at)| (s.title.as_str(), *at))
            .collect::<Vec<_>>(),
        [
            ("Ivy", 1_759_838_402_000),
            ("Nights", 1_759_838_401_500),
            ("Pink + White", 1_759_838_401_000)
        ]
    );
}

#[test]
fn an_import_adds_and_importing_again_adds_nothing() {
    let mut library = Library::open_in_memory().unwrap();
    let file = read(GOLDEN, NOW_MS).unwrap();

    let first = library.import(&file).unwrap();
    // Five songs in all: Halo, Pink + White, Nights, Ivy, Shared Tune. Each is one row however often the file names it.
    assert_eq!(
        first,
        Summary {
            songs_added: 5,
            songs_already: 0,
            likes_added: 3,
            likes_already: 0,
            playlists_added: 2,
            playlists_merged: 0,
            playlist_songs_added: 0,
            plays_added: 3,
            plays_already: 0,
            blocks_added: 0
        }
    );
    assert_eq!(library.counts().unwrap().tracks, 5);
    // The songs are searchable like any other, and say they are liked.
    let found = library.search("pink white", 10).unwrap();
    assert_eq!(
        (
            found.len(),
            found[0].title.as_str(),
            found[0].origin.as_str(),
            found[0].liked,
            found[0].duration_ms
        ),
        (1, "Pink + White", "described", true, 184_000)
    );
    assert!(!library.search("ivy", 10).unwrap()[0].liked);

    // The shared mix is here already and is left as it is, so its one song is not looked at again.
    let again = library.import(&file).unwrap();
    assert_eq!(
        again,
        Summary {
            songs_added: 0,
            songs_already: 4,
            likes_added: 0,
            likes_already: 3,
            playlists_added: 0,
            playlists_merged: 1,
            playlist_songs_added: 0,
            plays_added: 0,
            plays_already: 3,
            blocks_added: 0
        }
    );
    assert_eq!(library.counts().unwrap().tracks, 5);
}

#[test]
fn a_like_lands_on_the_file_the_library_already_has() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([
            file_track("Halo (feat. Nobody)", "BEYONCE", "C:/music/halo.flac"),
            file_track("Other", "Someone", "C:/music/other.flac"),
        ])
        .unwrap();

    let summary = library.import(&read(GOLDEN, NOW_MS).unwrap()).unwrap();
    // Halo is the file: four songs are new, one was here.
    assert_eq!(
        (
            summary.songs_added,
            summary.songs_already,
            summary.likes_added
        ),
        (4, 1, 3)
    );
    assert_eq!(library.counts().unwrap().tracks, 6);
    let halo = library.search("halo", 10).unwrap();
    assert_eq!(
        (
            halo.len(),
            halo[0].origin.as_str(),
            halo[0].liked,
            halo[0].codec.as_deref()
        ),
        (1, "file", true, Some("flac"))
    );
    // The file gained the length the import knew; it is still the file.
    assert_eq!(halo[0].duration_ms, 261_000);
    // It plays as the file it is.
    let queued = library.queue_items(&[halo[0].id]).unwrap();
    assert_eq!(
        (queued[0].locator.as_str(), queued[0].addon.is_none()),
        ("C:/music/halo.flac", true)
    );
    // A described song has nowhere of its own to play from: the add-ons are asked.
    let ivy = library.search("ivy", 10).unwrap();
    let queued = library.queue_items(&[ivy[0].id]).unwrap();
    assert_eq!(
        (queued[0].title.as_str(), queued[0].locator.as_str()),
        ("Ivy", "")
    );
}

#[test]
fn the_same_words_are_one_song_unless_two_isrcs_say_otherwise() {
    let mut library = Library::open_in_memory().unwrap();
    let file = likes_file(&[
        ("Song", "Band", Some("AAAAA0000001")),
        // The same words without an ISRC: the same song.
        ("song", "BAND", None),
        // The same words with another ISRC: another recording.
        ("Song", "Band", Some("BBBBB0000002")),
        // Another spelling with the first ISRC: the same recording.
        ("Song - 2011 Remaster", "Band", Some("aaaaa0000001")),
    ]);
    let summary = library.import(&read(&file, NOW_MS).unwrap()).unwrap();
    assert_eq!(
        (
            summary.songs_added,
            summary.likes_added,
            summary.likes_already
        ),
        (2, 2, 2)
    );
    assert_eq!(library.counts().unwrap().tracks, 2);
}

#[test]
fn a_playlist_that_is_here_gains_only_what_it_lacks() {
    let mut library = Library::open_in_memory().unwrap();
    library.import(&read(GOLDEN, NOW_MS).unwrap()).unwrap();
    let newer = serde_json::json!({
        "kind": "stash-web-library", "v": 1,
        "playlists": [
            { "id": "app-2", "name": "Night drive (renamed)", "items": [
                { "title": "Nights", "artist": "Frank Ocean" },
                { "title": "Ivy", "artist": "Frank Ocean" },
                { "title": "Brand New", "artist": "Someone" } ] },
            // The shared mix is here: it is left as it is, whatever this file says is in it.
            { "id": "app-6", "name": "Robin’s mix", "items": [{ "title": "Not Taken", "artist": "Nobody" }],
              "follow": { "id": "Fw12ab34", "version": 4 } },
            { "id": "web_1", "name": "From the web", "items": [{ "title": "Ivy", "artist": "Frank Ocean" }] } ]
    })
    .to_string();
    let summary = library
        .import(&read(newer.as_bytes(), NOW_MS).unwrap())
        .unwrap();
    // Night drive had Ivy and Pink + White: it gains Nights and Brand New. One new playlist; one new song in all.
    assert_eq!(
        (
            summary.playlists_added,
            summary.playlists_merged,
            summary.playlist_songs_added,
            summary.songs_added
        ),
        (1, 1, 2, 1)
    );
    assert_eq!(library.counts().unwrap().tracks, 6);
    assert!(library.search("not taken", 10).unwrap().is_empty());
}

#[test]
fn what_is_not_a_stash_library_file_is_refused_and_what_does_not_fit_is_left_out() {
    assert!(matches!(
        read(b"not json", NOW_MS),
        Err(ImportError::NotStash)
    ));
    assert!(matches!(
        read(br#"{"kind":"something-else","v":1}"#, NOW_MS),
        Err(ImportError::NotStash)
    ));
    assert!(matches!(
        read(br#"[1,2,3]"#, NOW_MS),
        Err(ImportError::NotStash)
    ));
    assert!(matches!(
        read(br#"{"kind":"stash-web-library","v":2}"#, NOW_MS),
        Err(ImportError::Newer)
    ));
    // Every part is optional.
    let empty = read(br#"{"kind":"stash-web-library","v":1}"#, NOW_MS).unwrap();
    assert_eq!(
        (empty.likes.len(), empty.playlists.len(), empty.plays.len()),
        (0, 0, 0)
    );

    let odd = serde_json::json!({
        "kind": "stash-web-library", "v": 1, "somethingNew": { "ignored": true },
        "likes": [
            { "item": { "title": "  Kept \u{202E}\n song ", "artist": "A", "isrc": "too-short", "durationMs": -5,
                        "refs": { "ok.source": "id1", "bad source!": "id2", "n": 7 },
                        "artwork": [{ "url": "http://plain.example/a.jpg" }, { "url": "javascript:alert(1)" }, { "url": "https://covers.example/a.jpg" }],
                        "spotifyId": "short" } },
            { "item": { "title": "", "artist": "No title" }, "likedAt": 5 },
            { "item": { "title": "No artist" }, "likedAt": 5 },
            "not an object",
            { "item": { "title": 7, "artist": "A number for a title" } }
        ],
        "playlists": [
            { "id": "has spaces", "name": "Bad id", "items": [] },
            { "id": "ok", "name": "", "items": [] },
            { "id": "ok", "name": "First with this id", "items": [{ "title": "x" }, { "title": "T", "artist": "A" }] },
            { "id": "ok", "name": "Second with this id", "items": [] },
            { "id": "no-items", "name": "No items" }
        ],
        "history": [
            { "item": { "title": "T", "artist": "A" }, "playedAt": 0 },
            { "item": { "title": "T", "artist": "A" }, "playedAt": "yesterday" },
            { "item": { "title": "T", "artist": "A" }, "playedAt": 1_759_838_400_000i64 }
        ]
    })
    .to_string();
    let file = read(odd.as_bytes(), NOW_MS).unwrap();
    assert_eq!(file.likes.len(), 1);
    let (kept, liked_at) = &file.likes[0];
    assert_eq!(
        (kept.title.as_str(), kept.isrc.as_deref(), kept.duration_ms),
        ("Kept song", None, None)
    );
    assert_eq!(kept.refs, [("ok.source".to_string(), "id1".to_string())]);
    assert_eq!(
        kept.art_url.as_deref(),
        Some("https://covers.example/a.jpg")
    );
    // A like without a time is given the time of the import.
    assert_eq!(*liked_at, NOW_MS);
    assert_eq!(
        file.playlists
            .iter()
            .map(|p| (p.name.as_str(), p.items.len()))
            .collect::<Vec<_>>(),
        [("First with this id", 1)]
    );
    assert_eq!(file.plays.len(), 1);
}
