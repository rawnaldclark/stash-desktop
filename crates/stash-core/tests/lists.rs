//! What the listener does to their library (likes, playlists, plays), and the library written back out as the file
//! the other Stashes read.

use stash_core::import::web_library::read;
use stash_core::import::{Incoming, Song};
use stash_core::{Error, Library, NewTrack, Origin};

const GOLDEN: &[u8] = include_bytes!("fixtures/app-export-v1.json");
const NOW_MS: i64 = 1_800_000_000_000;

fn file_track(title: &str, artist: &str, path: &str) -> NewTrack {
    NewTrack {
        title: title.into(),
        artist: artist.into(),
        album_artist: None,
        album: Some("An Album".into()),
        year: None,
        disc_no: 1,
        track_no: 0,
        duration_ms: 200_000,
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

fn id_of(library: &Library, title: &str) -> i64 {
    library.search(title, 1).unwrap()[0].id
}

fn titles(library: &Library, playlist: i64) -> Vec<String> {
    library
        .playlist_tracks_at(playlist, 0, 100)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect()
}

/// What matters of a file's contents, for comparing two files: likes, playlists and plays by their words and times.
type Timed = Vec<(String, i64)>;
/// A playlist's id, name, song titles and share id.
type Listed = Vec<(String, String, Vec<String>, Option<String>)>;

fn essence(file: &Incoming) -> (Timed, Listed, Timed) {
    (
        file.likes
            .iter()
            .map(|(s, at)| (format!("{} / {}", s.title, s.artist), *at))
            .collect(),
        file.playlists
            .iter()
            .map(|p| {
                (
                    p.external_id.clone(),
                    p.name.clone(),
                    p.items.iter().map(|s| s.title.clone()).collect(),
                    p.follow_id.clone(),
                )
            })
            .collect(),
        file.plays
            .iter()
            .map(|(s, at)| (s.title.clone(), *at))
            .collect(),
    )
}

#[test]
fn a_song_is_liked_and_the_like_taken_back() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([
            file_track("One", "A", "C:/m/1.flac"),
            file_track("Two", "A", "C:/m/2.flac"),
        ])
        .unwrap();
    let (one, two) = (id_of(&library, "One"), id_of(&library, "Two"));

    assert!(library.set_liked(one, true, 1_000).unwrap());
    assert!(library.set_liked(two, true, 2_000).unwrap());
    // Liking again keeps the first like's time: the order of the liked songs does not jump.
    assert!(library.set_liked(one, true, 9_000).unwrap());
    assert_eq!(
        library
            .liked_at(0, 10)
            .unwrap()
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        ["Two", "One"]
    );
    assert_eq!(library.counts().unwrap().liked, 2);

    assert!(library.set_liked(two, false, 3_000).unwrap());
    assert_eq!(library.counts().unwrap().liked, 1);
    assert!(!library.search("Two", 1).unwrap()[0].liked);
    assert!(!library.set_liked(999, true, 1).unwrap());
}

#[test]
fn a_playlist_is_made_filled_reordered_and_deleted() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks(
            ["One", "Two", "Three", "Four"].map(|t| file_track(t, "A", &format!("C:/m/{t}.flac"))),
        )
        .unwrap();
    let ids: Vec<i64> = ["One", "Two", "Three", "Four"]
        .iter()
        .map(|t| id_of(&library, t))
        .collect();

    assert!(matches!(
        library.create_playlist("  \n ", 1),
        Err(Error::Refused(_))
    ));
    let mix = library.create_playlist("  Night\tdrive ", 1_000).unwrap();
    let other = library.create_playlist("Other", 1_001).unwrap();
    assert_eq!(
        library
            .playlists()
            .unwrap()
            .iter()
            .map(|p| (p.name.as_str(), p.tracks))
            .collect::<Vec<_>>(),
        [("Night drive", 0), ("Other", 0)]
    );

    // Songs go at the end; one that is there already, or that the library does not have, is left out.
    assert_eq!(
        library
            .add_to_playlist(mix, &[ids[0], ids[1]], 2_000)
            .unwrap(),
        2
    );
    assert_eq!(
        library
            .add_to_playlist(mix, &[ids[1], ids[2], 999, ids[3]], 3_000)
            .unwrap(),
        2
    );
    assert_eq!(titles(&library, mix), ["One", "Two", "Three", "Four"]);
    assert_eq!(library.add_to_playlist(777, &[ids[0]], 3_000).unwrap(), 0);

    assert!(library.move_in_playlist(mix, 3, 0, 4_000).unwrap());
    assert_eq!(titles(&library, mix), ["Four", "One", "Two", "Three"]);
    assert!(library.move_in_playlist(mix, 0, 2, 4_000).unwrap());
    assert_eq!(titles(&library, mix), ["One", "Two", "Four", "Three"]);
    assert!(!library.move_in_playlist(mix, 1, 1, 4_000).unwrap());
    assert!(!library.move_in_playlist(mix, 9, 0, 4_000).unwrap());

    assert!(library.remove_from_playlist(mix, 1, 5_000).unwrap());
    assert_eq!(titles(&library, mix), ["One", "Four", "Three"]);
    assert!(!library.remove_from_playlist(mix, 3, 5_000).unwrap());
    // What plays from a row of the playlist follows the new order.
    assert_eq!(
        library.playlist_track_ids(mix, 1, 10).unwrap(),
        [ids[3], ids[2]]
    );

    assert!(library.rename_playlist(mix, "Late drive", 6_000).unwrap());
    assert!(matches!(
        library.rename_playlist(mix, "", 6_000),
        Err(Error::Refused(_))
    ));
    assert!(library.delete_playlist(other).unwrap());
    assert!(!library.delete_playlist(other).unwrap());
    assert_eq!(
        library
            .playlists()
            .unwrap()
            .iter()
            .map(|p| (p.name.as_str(), p.tracks))
            .collect::<Vec<_>>(),
        [("Late drive", 3)]
    );
    // Deleting a playlist leaves its songs in the library.
    assert!(library.delete_playlist(mix).unwrap());
    assert_eq!(library.counts().unwrap().tracks, 4);
}

#[test]
fn a_song_met_at_an_add_on_becomes_the_librarys_and_a_play_is_kept_once() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([file_track("Here Already", "A", "C:/m/here.flac")])
        .unwrap();
    let found = |title: &str, theirs: &str| Song {
        title: title.into(),
        artist: "A".into(),
        duration_ms: Some(180_000),
        refs: vec![("some.addon".into(), theirs.into())],
        ..Song::default()
    };
    // A song the library has is that song; a new one becomes a row known by its description, with the add-on's name
    // for it.
    let here = library.adopt(&found("here already", "t1")).unwrap();
    assert_eq!(here, id_of(&library, "Here Already"));
    let new = library.adopt(&found("Brand New", "t2")).unwrap();
    assert_eq!(library.adopt(&found("Brand New", "t2")).unwrap(), new);
    assert_eq!(library.counts().unwrap().tracks, 2);
    assert_eq!(
        library.describe(new).unwrap().unwrap().refs,
        [("some.addon".to_string(), "t2".to_string())]
    );
    assert_eq!(library.search("brand", 1).unwrap()[0].origin, "described");

    // A song from YouTube alone is asked for as the song its title names, and kept as it is (a saved file is tagged
    // with what the library calls it).
    let upload = library
        .adopt(&Song {
            title: "Planet Caravan - Black Sabbath".into(),
            artist: "Some Uploader".into(),
            refs: vec![("youtube".into(), "dQw4w9WgXcQ".into())],
            ..Song::default()
        })
        .unwrap();
    let described = library.describe(upload).unwrap().unwrap();
    assert_ne!(described.wanted, described.kept);
    assert_eq!(
        (
            described.kept.title.as_str(),
            described.kept.artist.as_str()
        ),
        ("Planet Caravan - Black Sabbath", "Some Uploader")
    );

    assert!(library.record_play(new, 5_000).unwrap());
    assert!(!library.record_play(new, 5_000).unwrap());
    assert!(library.record_play(new, 9_000).unwrap());
    assert!(!library.record_play(999, 9_000).unwrap());
}

#[test]
fn the_library_goes_out_as_the_file_it_came_in_as() {
    let mut library = Library::open_in_memory().unwrap();
    let golden = read(GOLDEN, NOW_MS).unwrap();
    library.import(&golden).unwrap();

    let out = library.export(NOW_MS, "Stash desktop (test)").unwrap();
    assert_eq!(
        (
            out["kind"].as_str(),
            out["v"].as_i64(),
            out["exportedAt"].as_str(),
            out["generator"].as_str()
        ),
        (
            Some("stash-web-library"),
            Some(1),
            Some("2027-01-15T08:00:00.000Z"),
            Some("Stash desktop (test)")
        )
    );
    // A song is written as the format writes it: the Spotify id beside the refs, the cover as artwork.
    let pink = &out["likes"][1]["item"];
    assert_eq!(
        (
            pink["title"].as_str(),
            pink["album"].as_str(),
            pink["durationMs"].as_i64(),
            pink["isrc"].as_str(),
            pink["spotifyId"].as_str(),
            pink["refs"]["youtube"].as_str(),
            pink["artwork"][0]["url"].as_str()
        ),
        (
            Some("Pink + White"),
            Some("Blonde"),
            Some(184_000),
            Some("USUM71607009"),
            Some("3xKsf9qdS1CyvXSMEid6g8"),
            Some("uxpDa-c-4Mc"),
            Some("https://i.scdn.co/image/ab67616d0000b273c5649add07ed3720be9d5526")
        )
    );
    assert!(pink.get("id").is_none() && pink.get("locator").is_none());
    // The shared mix goes out as the follow it is.
    assert_eq!(
        out["playlists"][1]["follow"],
        serde_json::json!({ "id": "Fw12ab34", "version": 3, "sharedBy": "Robin", "checkedAt": 0 })
    );

    // Read back by the same reader, it says what the golden file said.
    let back = read(out.to_string().as_bytes(), NOW_MS).unwrap();
    assert_eq!(essence(&back), essence(&golden));
    // And coming back in, it is all here already.
    let again = library.import(&back).unwrap();
    assert_eq!(
        (
            again.songs_added,
            again.likes_added,
            again.playlists_added,
            again.plays_added
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn what_was_done_here_goes_out_and_what_only_works_here_stays() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([
            file_track(
                "Tagged",
                "Real Artist",
                "C:/music/secret folder/tagged.flac",
            ),
            file_track(
                "untagged-file",
                stash_core::scan::UNKNOWN_ARTIST,
                "C:/music/untagged-file.flac",
            ),
        ])
        .unwrap();
    let (tagged, untagged) = (id_of(&library, "Tagged"), id_of(&library, "untagged-file"));
    library.set_liked(tagged, true, 5_000).unwrap();
    library.set_liked(untagged, true, 6_000).unwrap();
    let mix = library.create_playlist("Made here", 7_000).unwrap();
    library
        .add_to_playlist(mix, &[untagged, tagged], 8_000)
        .unwrap();
    library.record_play(tagged, 9_000).unwrap();
    library.set_ref(tagged, "some.addon", "t9").unwrap();

    let out = library.export(NOW_MS, "Stash desktop (test)").unwrap();
    let text = out.to_string();
    // A file is described, never located; a file without tags is not something another Stash could find.
    assert!(
        !text.contains("secret folder")
            && !text.contains("untagged-file")
            && !text.contains("flac")
    );
    assert_eq!(out["likes"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            out["likes"][0]["item"]["title"].as_str(),
            out["likes"][0]["likedAt"].as_i64(),
            out["likes"][0]["item"]["refs"]["some.addon"].as_str()
        ),
        (Some("Tagged"), Some(5_000), Some("t9"))
    );
    let playlist = &out["playlists"][0];
    assert_eq!(
        (
            playlist["name"].as_str(),
            playlist["items"].as_array().unwrap().len(),
            playlist["createdAt"].as_i64(),
            playlist["updatedAt"].as_i64()
        ),
        (Some("Made here"), 1, Some(7_000), Some(8_000))
    );
    assert!(playlist["id"].as_str().unwrap().starts_with("desk-"));
    assert!(playlist.get("follow").is_none());
    assert_eq!(
        out["history"],
        serde_json::json!([{ "item": out["likes"][0]["item"], "playedAt": 9_000 }])
    );

    // Another library takes it in: the like, the playlist and the play, each once.
    let mut other = Library::open_in_memory().unwrap();
    let summary = other
        .import(&read(text.as_bytes(), NOW_MS).unwrap())
        .unwrap();
    assert_eq!(
        (
            summary.songs_added,
            summary.likes_added,
            summary.playlists_added,
            summary.plays_added
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn settings_and_music_folders_are_kept_with_the_library() {
    let mut library = Library::open_in_memory().unwrap();
    assert_eq!(library.setting("volume").unwrap(), None);
    library.set_setting("volume", "40").unwrap();
    library.set_setting("volume", "65").unwrap();
    assert_eq!(library.setting("volume").unwrap().as_deref(), Some("65"));

    library.remember_folder("D:/Music", 1_000).unwrap();
    library.remember_folder("E:/More", 2_000).unwrap();
    // Scanning a folder again does not list it twice or move it.
    library.remember_folder("D:/Music", 3_000).unwrap();
    assert_eq!(library.folders().unwrap(), ["D:/Music", "E:/More"]);
}

#[test]
fn autoplay_spreads_an_artist_out_and_does_not_follow_the_last_song_with_it() {
    let mut library = Library::open_in_memory().unwrap();
    let mut songs = Vec::new();
    for n in 0..5 {
        songs.push(file_track(
            &format!("A{n}"),
            "A",
            &format!("C:/m/a{n}.flac"),
        ));
    }
    for artist in ["B", "C", "D", "E", "F"] {
        songs.push(file_track(artist, artist, &format!("C:/m/{artist}.flac")));
    }
    library.add_tracks(songs).unwrap();
    let artist_of = |id: i64| library.describe(id).unwrap().unwrap().kept.artist;
    for _ in 0..20 {
        let picks = library
            .autoplay_picks(&[], &["A".into()], &Default::default(), 0, true, 10)
            .unwrap();
        assert_eq!(picks.len(), 10);
        let artists: Vec<String> = picks.iter().map(|&id| artist_of(id)).collect();
        // Five by A and one each by five others: A every other song, never right after A's song.
        assert_ne!(artists[0], "A", "{artists:?}");
        assert!(
            artists.windows(2).all(|pair| pair[0] != pair[1]),
            "{artists:?}"
        );
    }
}

#[test]
fn a_song_no_add_on_finds_waits_to_be_fixed_and_leaves_when_it_is() {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([file_track("On Disk", "A", "C:/m/disk.flac")])
        .unwrap();
    let described = |title: &str| Song {
        title: title.into(),
        artist: "A".into(),
        duration_ms: Some(180_000),
        refs: vec![("spotify".into(), format!("{title:0>22}").replace(' ', "x"))],
        ..Song::default()
    };
    let (hotel, india) = (
        library.adopt(&described("Hotel")).unwrap(),
        library.adopt(&described("India")).unwrap(),
    );
    let file = id_of(&library, "On Disk");

    // Noted once, with its first and latest miss; a file that is here is never noted.
    assert!(library.note_unfound(hotel, 1_000).unwrap());
    assert!(library.note_unfound(hotel, 2_000).unwrap());
    assert!(library.note_unfound(india, 1_500).unwrap());
    assert!(!library.note_unfound(file, 1_500).unwrap());
    assert!(!library.note_unfound(999, 1_500).unwrap());
    let listed = library.unfound_at(0, 10).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|r| (r.track.title.as_str(), r.first_at_ms, r.last_at_ms))
            .collect::<Vec<_>>(),
        [("Hotel", 1_000, 2_000), ("India", 1_500, 1_500)]
    );
    assert_eq!(library.unfound_counts().unwrap().open, 2);

    // Autoplay leaves out what is known not to play.
    let picks = library
        .autoplay_picks(&[], &[], &Default::default(), 0, false, 10)
        .unwrap();
    assert!(!picks.contains(&hotel) && !picks.contains(&india));
    assert!(picks.contains(&file));

    // Set aside: off the list, still marked, and it stays aside when it is missed again; then put back.
    assert_eq!(library.set_unfound_aside(&[india], true, 3_000).unwrap(), 1);
    library.note_unfound(india, 4_000).unwrap();
    let counts = library.unfound_counts().unwrap();
    assert_eq!((counts.open, counts.marked), (1, vec![hotel, india]));
    library.set_unfound_aside(&[india], false, 5_000).unwrap();
    assert_eq!(library.unfound_counts().unwrap().open, 2);

    // Fixed: the add-on's name for it is kept, it leaves the list, and a save that failed is asked again.
    library.ask_downloads(&[hotel], 6_000).unwrap();
    assert_eq!(library.next_download().unwrap(), Some(hotel));
    library
        .download_failed(hotel, "none of your add-ons has it")
        .unwrap();
    assert_eq!(
        library
            .fix_unfound(hotel, "demo.folder", "41", 7_000)
            .unwrap(),
        Some(true)
    );
    assert!(
        library
            .describe(hotel)
            .unwrap()
            .unwrap()
            .refs
            .contains(&("demo.folder".to_string(), "41".to_string()))
    );
    assert_eq!(library.download_counts().unwrap().queued, 1);
    assert_eq!(library.unfound_counts().unwrap().marked, [india]);
    assert_eq!(
        library.fix_unfound(999, "demo.folder", "1", 7_000).unwrap(),
        None
    );

    // Found by itself later: gone from the list.
    assert!(library.found_again(india).unwrap());
    assert_eq!(library.unfound_counts().unwrap(), Default::default());
}

#[test]
fn copies_of_played_songs_are_kept_in_the_room_given_and_the_least_played_go_first() {
    let mut library = Library::open_in_memory().unwrap();
    let song = |title: &str| Song {
        title: title.into(),
        artist: "A".into(),
        refs: vec![("some.addon".into(), title.into())],
        ..Song::default()
    };
    let ids: Vec<i64> = ["a", "b", "c", "d"]
        .iter()
        .map(|t| library.adopt(&song(t)).unwrap())
        .collect();
    let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
    library.set_played_cap(1).unwrap();
    let keep = |library: &mut Library, id: i64, file: &str, bytes: u64, at: i64| {
        library
            .keep_played_copy(id, file, bytes, Some("lossless"), Some("some.addon"), at)
            .unwrap()
    };
    assert_eq!(keep(&mut library, a, "a.flac", 400_000, 1), (true, vec![]));
    assert_eq!(keep(&mut library, b, "b.flac", 400_000, 2), (true, vec![]));
    // Played again: a is the most recently used now, so b goes when room is short.
    let copy = library.played_copy(a, 3).unwrap().unwrap();
    assert_eq!(
        (copy.file.as_str(), copy.tier.as_deref()),
        ("a.flac", Some("lossless"))
    );
    assert_eq!(
        keep(&mut library, c, "c.flac", 400_000, 4),
        (true, vec!["b.flac".to_string()])
    );
    assert!(library.played_copy(b, 5).unwrap().is_none());
    // A new copy of a song replaces its old file; one larger than the whole room is not kept.
    assert_eq!(
        keep(&mut library, a, "a.mp3", 300_000, 6),
        (true, vec!["a.flac".to_string()])
    );
    assert_eq!(
        keep(&mut library, d, "d.flac", 2_000_000, 7),
        (false, vec!["d.flac".to_string()])
    );
    let room = library.played_room().unwrap();
    assert_eq!((room.songs, room.bytes, room.cap_mb), (2, 700_000, 1));
    let mut files = library.played_files().unwrap();
    files.sort();
    assert_eq!(files, ["a.mp3", "c.flac"]);
    // A smaller room keeps the most recently played; none at all keeps nothing.
    library.played_copy(c, 8).unwrap();
    library.set_played_cap(0).unwrap();
    assert_eq!(library.played_room().unwrap().songs, 0);
    assert_eq!(
        keep(&mut library, a, "a.flac", 10, 9),
        (false, vec!["a.flac".to_string()])
    );
    library.set_played_cap(5).unwrap();
    keep(&mut library, a, "a.flac", 10, 10);
    assert_eq!(
        library.drop_played_copy(a).unwrap().as_deref(),
        Some("a.flac")
    );
    assert_eq!(library.played_room().unwrap().songs, 0);
}
