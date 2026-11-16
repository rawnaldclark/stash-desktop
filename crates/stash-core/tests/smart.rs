//! Smart playlists: rules over what the library knows of a song, read against a real library.

use serde_json::{Value, json};
use stash_core::import::Song;
use stash_core::import::web_library::read;
use stash_core::smart::Rules;
use stash_core::{Library, NewTrack, Origin};

const NOW_MS: i64 = 1_800_000_000_000;
const DAY_MS: i64 = 86_400_000;

#[allow(clippy::too_many_arguments)]
fn file(
    title: &str,
    artist: &str,
    album: Option<&str>,
    year: Option<i32>,
    seconds: u64,
    (codec, bits, rate): (&str, Option<u32>, u32),
    added_days_ago: i64,
) -> NewTrack {
    NewTrack {
        title: title.into(),
        artist: artist.into(),
        album_artist: None,
        album: album.map(Into::into),
        year,
        disc_no: 1,
        track_no: 0,
        duration_ms: seconds * 1000,
        isrc: None,
        origin: Origin::File,
        addon_id: None,
        locator: format!("C:/m/{title}.x"),
        codec: Some(codec.into()),
        sample_rate: Some(rate),
        bit_depth: bits,
        added_at: (NOW_MS - added_days_ago * DAY_MS) / 1000,
        file_size: Some(1),
        file_mtime: Some(1),
    }
}

fn id_of(library: &Library, title: &str) -> i64 {
    library.search(title, 1).unwrap()[0].id
}

/// A library of five songs, played and liked as the rules below expect.
fn library() -> Library {
    let mut library = Library::open_in_memory().unwrap();
    library
        .add_tracks([
            file(
                "Quiet Rooms",
                "The Lanterns",
                Some("Harbor"),
                Some(1997),
                200,
                ("flac", Some(16), 44_100),
                2,
            ),
            file(
                "Afterglow",
                "Beyoncé",
                Some("Lights"),
                Some(2016),
                400,
                ("flac", Some(24), 96_000),
                100,
            ),
            file(
                "Paper Boats",
                "The Lanterns feat. Mira",
                Some("Harbor"),
                Some(1997),
                150,
                ("mp3", None, 44_100),
                40,
            ),
            file(
                "Old Song",
                "Oda Lune",
                None,
                None,
                500,
                ("aac", None, 44_100),
                400,
            ),
        ])
        .unwrap();
    library
        .adopt(&Song {
            title: "Streamed".into(),
            artist: "Nowhere".into(),
            refs: vec![("demo.folder".into(), "s1".into())],
            added_at_ms: Some(NOW_MS),
            ..Song::default()
        })
        .unwrap();
    let quiet = id_of(&library, "Quiet Rooms");
    for days in 1..=3 {
        library.record_play(quiet, NOW_MS - days * DAY_MS).unwrap();
    }
    let afterglow = id_of(&library, "Afterglow");
    library
        .record_play(afterglow, NOW_MS - 200 * DAY_MS)
        .unwrap();
    let boats = id_of(&library, "Paper Boats");
    for days in 60..65 {
        library.record_play(boats, NOW_MS - days * DAY_MS).unwrap();
    }
    library.set_liked(quiet, true, NOW_MS).unwrap();
    library
        .set_liked(id_of(&library, "Old Song"), true, NOW_MS)
        .unwrap();
    library
}

fn rules(value: Value) -> Rules {
    Rules::read(&value).unwrap()
}

/// The titles `value`'s rules pick, in their order.
fn picked(library: &Library, value: Value) -> Vec<String> {
    let rules = rules(value);
    let rows = library.smart_tracks_at(&rules, 0, 100, NOW_MS).unwrap();
    assert_eq!(
        library.smart_count(&rules, NOW_MS).unwrap(),
        rows.len() as u64
    );
    rows.into_iter().map(|t| t.title).collect()
}

fn one(field: &str, op: &str, value: Value) -> Value {
    json!({ "rules": [{ "field": field, "op": op, "value": value }] })
}

#[test]
fn each_rule_picks_the_songs_it_says() {
    let library = library();
    let pick = |value: Value| picked(&library, value);
    // Text, without case or accents; the newest added first unless told otherwise.
    assert_eq!(
        pick(one("artist", "contains", json!("lanterns"))),
        ["Quiet Rooms", "Paper Boats"]
    );
    assert_eq!(
        pick(one("artist", "is", json!("THE LANTERNS"))),
        ["Quiet Rooms"]
    );
    assert_eq!(
        pick(one("artist", "contains", json!("beyonce"))),
        ["Afterglow"]
    );
    assert_eq!(
        pick(one("title", "starts_with", json!("pa"))),
        ["Paper Boats"]
    );
    // A song with no album is not on "Harbor".
    assert_eq!(
        pick(one("album", "is_not", json!("harbor"))),
        ["Streamed", "Afterglow", "Old Song"]
    );
    assert_eq!(
        pick(one("album", "not_contains", json!("arb"))),
        ["Streamed", "Afterglow", "Old Song"]
    );
    // Numbers: a song without a year or a length is neither before nor after.
    assert_eq!(pick(one("year", "at_least", json!(2000))), ["Afterglow"]);
    assert_eq!(
        pick(one("year", "below", json!(2000))),
        ["Quiet Rooms", "Paper Boats"]
    );
    assert_eq!(pick(one("length", "below", json!(180))), ["Paper Boats"]);
    assert_eq!(
        pick(one("length", "at_least", json!(400))),
        ["Afterglow", "Old Song"]
    );
    assert_eq!(pick(one("plays", "is", json!(0))), ["Streamed", "Old Song"]);
    let mut most = one("plays", "at_least", json!(3));
    most["order"] = json!("most_played");
    assert_eq!(pick(most), ["Paper Boats", "Quiet Rooms"]);
    // Times.
    assert_eq!(
        pick(
            json!({ "rules": [{ "field": "played_often", "op": "at_least", "value": 3, "days": 30 }] })
        ),
        ["Quiet Rooms"]
    );
    assert_eq!(
        pick(
            json!({ "rules": [{ "field": "played_often", "op": "at_least", "value": 3, "days": 90 }] })
        ),
        ["Quiet Rooms", "Paper Boats"]
    );
    assert_eq!(
        pick(one("last_played", "within", json!(30))),
        ["Quiet Rooms"]
    );
    // Never played is not played within.
    assert_eq!(
        pick(one("last_played", "not_within", json!(30))),
        ["Streamed", "Paper Boats", "Afterglow", "Old Song"]
    );
    assert_eq!(
        pick(one("added", "within", json!(30))),
        ["Streamed", "Quiet Rooms"]
    );
    assert_eq!(pick(one("added", "not_within", json!(365))), ["Old Song"]);
    // Likes, quality from the file, and where the song plays from.
    assert_eq!(
        pick(json!({ "rules": [{ "field": "liked", "op": "is" }] })),
        ["Quiet Rooms", "Old Song"]
    );
    assert_eq!(pick(one("quality", "is", json!("hires"))), ["Afterglow"]);
    assert_eq!(
        pick(one("quality", "at_least", json!("lossless"))),
        ["Quiet Rooms", "Afterglow"]
    );
    assert_eq!(
        pick(one("quality", "is", json!("lossy"))),
        ["Paper Boats", "Old Song"]
    );
    assert_eq!(
        pick(json!({ "rules": [{ "field": "here", "op": "is_not" }] })),
        ["Streamed"]
    );
    // Every rule, or any one.
    let both = json!([
        { "field": "artist", "op": "contains", "value": "oda" },
        { "field": "year", "op": "at_least", "value": 2000 }
    ]);
    assert_eq!(pick(json!({ "rules": both })), Vec::<String>::new());
    assert_eq!(
        pick(json!({ "any": true, "rules": both })),
        ["Afterglow", "Old Song"]
    );
    // No rules: every song, in the order asked, as many as the limit lets in.
    assert_eq!(
        pick(json!({ "order": "least_recently_played", "limit": 3 })),
        ["Old Song", "Streamed", "Afterglow"]
    );
    assert_eq!(
        pick(json!({ "order": "title" })),
        [
            "Afterglow",
            "Old Song",
            "Paper Boats",
            "Quiet Rooms",
            "Streamed"
        ]
    );
}

#[test]
fn a_limit_ends_the_pages_and_a_random_order_holds_for_the_day() {
    let library = library();
    let three = rules(json!({ "order": "title", "limit": 3 }));
    let page = |offset| -> Vec<String> {
        library
            .smart_tracks_at(&three, offset, 2, NOW_MS)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };
    assert_eq!(page(0), ["Afterglow", "Old Song"]);
    assert_eq!(page(2), ["Paper Boats"]);
    assert!(page(3).is_empty());
    assert_eq!(
        library
            .smart_track_ids(&three, 1, 10, NOW_MS)
            .unwrap()
            .len(),
        2
    );

    let random = rules(json!({ "order": "random" }));
    let order = |at| library.smart_track_ids(&random, 0, 100, at).unwrap();
    let today = order(NOW_MS);
    assert_eq!(order(NOW_MS + 3_600_000), today, "the same all day");
    let mut all = today.clone();
    all.sort_unstable();
    assert_eq!(all.len(), 5);
    let days: Vec<Vec<i64>> = (1..=10).map(|d| order(NOW_MS + d * DAY_MS)).collect();
    assert!(
        days.iter().any(|d| *d != today),
        "another order on other days"
    );
}

#[test]
fn a_smart_playlist_fills_itself_and_its_songs_are_not_edited_by_hand() {
    let mut library = library();
    let lanterns = rules(one("artist", "contains", json!("lanterns")));
    let smart = library
        .create_smart_playlist("Lanterns", &lanterns, NOW_MS)
        .unwrap();
    let plain = library.create_playlist("By hand", NOW_MS).unwrap();
    let listed = library.playlists().unwrap();
    let row = listed.iter().find(|p| p.id == smart).unwrap();
    assert_eq!((row.tracks, row.rules.as_ref()), (2, Some(&lanterns)));
    assert!(
        listed
            .iter()
            .find(|p| p.id == plain)
            .unwrap()
            .rules
            .is_none()
    );

    let titles = |library: &Library| -> Vec<String> {
        library
            .playlist_tracks_at(smart, 0, 100)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    };
    assert_eq!(titles(&library), ["Quiet Rooms", "Paper Boats"]);
    assert_eq!(library.playlist_track_ids(smart, 1, 10).unwrap().len(), 1);

    // Its songs are its rules'.
    let old = id_of(&library, "Old Song");
    assert_eq!(library.add_to_playlist(smart, &[old], NOW_MS).unwrap(), 0);
    assert!(!library.move_in_playlist(smart, 0, 1, NOW_MS).unwrap());
    assert!(!library.remove_from_playlist(smart, 0, NOW_MS).unwrap());
    assert_eq!(titles(&library), ["Quiet Rooms", "Paper Boats"]);

    // New rules; a playlist made by hand takes none.
    let liked = rules(json!({ "rules": [{ "field": "liked", "op": "is" }] }));
    assert!(!library.set_smart_rules(plain, &liked, NOW_MS).unwrap());
    assert!(library.set_smart_rules(smart, &liked, NOW_MS).unwrap());
    assert_eq!(titles(&library), ["Quiet Rooms", "Old Song"]);
}

#[test]
fn a_smart_playlist_goes_out_as_its_songs_with_its_rules_and_comes_back_as_rules() {
    let mut library = library();
    let lanterns = rules(one("artist", "contains", json!("lanterns")));
    let smart = library
        .create_smart_playlist("Lanterns", &lanterns, NOW_MS)
        .unwrap();
    let out = library.export(NOW_MS, "Stash desktop (test)").unwrap();
    let listed = out["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Lanterns")
        .unwrap()
        .clone();
    // Another Stash keeps the songs; this one, the rules.
    let songs: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect();
    assert_eq!(songs, ["Quiet Rooms", "Paper Boats"]);
    assert_eq!(Rules::read(&listed["rules"]), Ok(lanterns.clone()));

    // Into a library that has none of it: the playlist is smart again, and its songs are there for it to find.
    let bytes = serde_json::to_vec(&out).unwrap();
    let incoming = read(&bytes, NOW_MS).unwrap();
    let mut other = Library::open_in_memory().unwrap();
    other.import(&incoming).unwrap();
    let there = other.playlists().unwrap();
    let back = there.iter().find(|p| p.name == "Lanterns").unwrap();
    assert_eq!((back.rules.as_ref(), back.tracks), (Some(&lanterns), 2));

    // Into the library it came from: nothing changes, and no songs are written into it by hand.
    library.import(&incoming).unwrap();
    let rows = library.playlists().unwrap();
    assert_eq!(rows.iter().filter(|p| p.name == "Lanterns").count(), 1);
    assert_eq!(rows.iter().find(|p| p.id == smart).unwrap().tracks, 2);

    // The same playlist come back from a phone, without its rules (the phone has none): it stays smart.
    let mut from_phone = out.clone();
    for playlist in from_phone["playlists"].as_array_mut().unwrap() {
        playlist.as_object_mut().unwrap().remove("rules");
    }
    let phone = read(&serde_json::to_vec(&from_phone).unwrap(), NOW_MS).unwrap();
    library.import(&phone).unwrap();
    let row = library
        .playlists()
        .unwrap()
        .into_iter()
        .find(|p| p.id == smart)
        .unwrap();
    assert_eq!((row.rules, row.tracks), (Some(lanterns), 2));
}
