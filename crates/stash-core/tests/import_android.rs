//! Reading the Android app's own library. The database here is made up in the app's shape (the tables and columns
//! the reader uses, as the app's schema 80 names them): no real library is part of the tests.

use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use stash_core::Library;
use stash_core::import::{ImportError, Incoming, read_bytes, read_file};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "stash-android-import-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The phone's database with a small library in it. Times are milliseconds, as the app keeps them.
fn phone_database(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA user_version = 80;
         CREATE TABLE tracks(id INTEGER PRIMARY KEY, title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT NOT NULL, duration_ms INTEGER NOT NULL,
             file_path TEXT, source TEXT NOT NULL, spotify_uri TEXT, youtube_id TEXT, album_art_url TEXT, date_added INTEGER NOT NULL,
             is_downloaded INTEGER NOT NULL DEFAULT 0, canonical_title TEXT NOT NULL DEFAULT '', canonical_artist TEXT NOT NULL DEFAULT '',
             isrc TEXT, explicit INTEGER, stash_liked_at INTEGER, is_streamable INTEGER NOT NULL DEFAULT 1, is_streamable_checked_at INTEGER);
         CREATE TABLE playlists(id INTEGER PRIMARY KEY, name TEXT NOT NULL, source TEXT NOT NULL, source_id TEXT NOT NULL, type TEXT NOT NULL,
             is_active INTEGER NOT NULL DEFAULT 1, sync_enabled INTEGER NOT NULL DEFAULT 1, date_added INTEGER NOT NULL, pinned_to_home_at INTEGER);
         CREATE TABLE playlist_tracks(playlist_id INTEGER NOT NULL, track_id INTEGER NOT NULL, position INTEGER NOT NULL, added_at INTEGER NOT NULL,
             removed_at INTEGER, PRIMARY KEY (playlist_id, track_id));
         CREATE TABLE listening_events(id INTEGER PRIMARY KEY, track_id INTEGER NOT NULL, started_at INTEGER NOT NULL);
         CREATE TABLE track_blocklist(canonical_key TEXT, artist TEXT, title TEXT, spotify_uri TEXT, youtube_id TEXT, blocked_at INTEGER);
         CREATE TABLE shared_mixes(playlist_id INTEGER PRIMARY KEY, share_id TEXT NOT NULL, role TEXT NOT NULL, version INTEGER NOT NULL,
             status TEXT NOT NULL, shared_by TEXT);

         INSERT INTO tracks(id, title, artist, album, duration_ms, file_path, source, spotify_uri, youtube_id, album_art_url, date_added, is_downloaded, canonical_title, canonical_artist, isrc, explicit, stash_liked_at) VALUES
           (1, 'Pink + White', 'Frank Ocean', 'Blonde', 184000, '/storage/emulated/0/Music/Stash/pink.opus', 'SPOTIFY', 'spotify:track:3xKsf9qdS1CyvXSMEid6g8', 'uxpDa-c-4Mc', 'https://i.scdn.co/image/cover1', 1000, 1, 'pink white', 'frank ocean', 'usum71607009', 1, 5000),
           (2, 'Nights', 'Frank Ocean', 'Blonde', 307000, NULL, 'SPOTIFY', 'spotify:track:7eqoqGkKwgOaWNNHx90uEZ', NULL, NULL, 1100, 0, 'nights', 'frank ocean', NULL, 0, NULL),
           (3, 'Ivy', 'Frank Ocean', 'Blonde', 249000, NULL, 'YOUTUBE', NULL, 'AE005nZeF-A', 'http://not-https.example/cover', 1200, 0, 'ivy', 'frank ocean', NULL, NULL, NULL),
           (4, 'Blocked Song', 'Nobody', '', 100000, NULL, 'SPOTIFY', 'spotify:track:0000000000000000000001', NULL, NULL, 1300, 1, 'blocked song', 'nobody', NULL, NULL, 6000),
           (5, 'Only Downloaded', 'Someone', 'An Album', 200000, '/storage/emulated/0/Music/Stash/only.opus', 'SPOTIFY', NULL, NULL, NULL, 1400, 1, 'only downloaded', 'someone', NULL, NULL, NULL),
           (6, 'Mix Filler', 'Whoever', '', 180000, NULL, 'SPOTIFY', NULL, NULL, NULL, 1500, 0, 'mix filler', 'whoever', NULL, NULL, NULL),
           (7, 'Album Track', 'Album Artist', 'Saved Album', 210000, NULL, 'SPOTIFY', NULL, NULL, NULL, 1600, 0, 'album track', 'album artist', NULL, NULL, NULL),
           (8, 'A Cloud Upload', 'Bedroom Producer', '', 90000, NULL, 'SOUNDCLOUD', NULL, NULL, NULL, 1700, 1, 'a cloud upload', 'bedroom producer', NULL, NULL, NULL),
           (9, '   ', 'No Title', '', 90000, NULL, 'SPOTIFY', NULL, NULL, NULL, 1800, 1, '', 'no title', NULL, NULL, NULL),
           (10, 'Shared Tune', 'Robin Band', '', 0, NULL, 'BOTH', NULL, NULL, NULL, 1900, 0, 'shared tune', 'robin band', NULL, NULL, NULL);
         INSERT INTO track_blocklist(canonical_key, artist, title, blocked_at) VALUES ('nobody|blocked song', 'Nobody', 'Blocked Song', 1);

         INSERT INTO playlists(id, name, source, source_id, type, is_active, sync_enabled, date_added) VALUES
           (1, 'Liked Songs', 'SPOTIFY', 'liked', 'LIKED_SONGS', 1, 0, 100),
           (2, '  Night drive ', 'SPOTIFY', '0wpFuhNdMabc', 'CUSTOM', 1, 1, 200),
           (3, 'Daily Mix 1', 'SPOTIFY', 'mix1', 'DAILY_MIX', 1, 1, 300),
           (4, 'Saved Album', 'SPOTIFY', 'album:spotify:abc', 'CUSTOM', 1, 1, 400),
           (5, 'Robin’s mix', 'BOTH', 'share:Fw12ab34', 'CUSTOM', 1, 0, 500),
           (6, 'From the web', 'BOTH', 'custom_web_web_1', 'CUSTOM', 1, 0, 600),
           (7, 'Gone', 'SPOTIFY', 'gone', 'CUSTOM', 0, 1, 700);
         INSERT INTO playlist_tracks(playlist_id, track_id, position, added_at, removed_at) VALUES
           (1, 2, 0, 4000, NULL), (1, 3, 1, 4500, 4600),
           (2, 3, 0, 2000, NULL), (2, 1, 1, 2500, NULL), (2, 4, 2, 2600, NULL), (2, 2, 3, 2700, 2800),
           (3, 6, 0, 3000, NULL),
           (4, 7, 0, 3100, NULL),
           (5, 10, 0, 3200, NULL),
           (6, 2, 0, 3300, NULL),
           (7, 5, 0, 3400, NULL);
         INSERT INTO shared_mixes(playlist_id, share_id, role, version, status, shared_by) VALUES (5, 'Fw12ab34', 'FOLLOWER', 3, 'ACTIVE', 'Robin');
         INSERT INTO listening_events(id, track_id, started_at) VALUES (1, 3, 9000), (2, 1, 9500), (3, 4, 9600), (4, 3, 0);",
    )
    .unwrap();
}

fn check(file: &Incoming) {
    assert_eq!(file.source, "android-library");
    // Liked: the Stash like and the song in the Liked Songs list, newest first. Not the blocked song, though it is
    // liked; not the song that was taken out of the Liked Songs list.
    assert_eq!(
        file.likes
            .iter()
            .map(|(s, at)| (s.title.as_str(), *at))
            .collect::<Vec<_>>(),
        [("Pink + White", 5000), ("Nights", 4000)]
    );
    let pink = &file.likes[0].0;
    assert_eq!(
        (
            pink.album.as_deref(),
            pink.duration_ms,
            pink.isrc.as_deref(),
            pink.explicit,
            pink.added_at_ms
        ),
        (
            Some("Blonde"),
            Some(184_000),
            Some("USUM71607009"),
            Some(true),
            Some(1000)
        )
    );
    assert_eq!(
        pink.refs,
        [
            ("youtube".to_string(), "uxpDa-c-4Mc".to_string()),
            ("spotify".to_string(), "3xKsf9qdS1CyvXSMEid6g8".to_string())
        ]
    );
    assert_eq!(
        pink.art_url.as_deref(),
        Some("https://i.scdn.co/image/cover1")
    );

    // The listener's own playlists, oldest first. Not the Daily Mix, not the saved album, not the one that is gone.
    let lists: Vec<(&str, &str, Vec<&str>)> = file
        .playlists
        .iter()
        .map(|p| {
            (
                p.external_id.as_str(),
                p.name.as_str(),
                p.items.iter().map(|s| s.title.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        lists,
        [
            // In its order, without the blocked song and the song that was taken out.
            ("app-2", "Night drive", vec!["Ivy", "Pink + White"]),
            ("app-5", "Robin’s mix", vec!["Shared Tune"]),
            // A playlist that came from Stash on the web keeps the web's id for it.
            ("web_1", "From the web", vec!["Nights"]),
        ]
    );
    let (drive, mix) = (&file.playlists[0], &file.playlists[1]);
    assert_eq!(
        (
            drive.created_at_ms,
            drive.updated_at_ms,
            drive.follow_id.as_deref()
        ),
        (Some(200), Some(2500), None)
    );
    assert_eq!(
        (
            mix.follow_id.as_deref(),
            mix.follow_version,
            mix.follow_shared_by.as_deref()
        ),
        (Some("Fw12ab34"), Some(3), Some("Robin"))
    );
    // A cover that is not https stays behind; a song without a length has none.
    assert_eq!(drive.items[0].art_url, None);
    assert_eq!(mix.items[0].duration_ms, None);

    // Every play with a time, newest first; not the blocked song's.
    assert_eq!(
        file.plays
            .iter()
            .map(|(s, at)| (s.title.as_str(), *at))
            .collect::<Vec<_>>(),
        [("Pink + White", 9500), ("Ivy", 9000)]
    );

    // The rest of what the listener chose to have: downloads and the songs of saved albums. Not the mix's filler,
    // not the blocked song, not a SoundCloud upload, not a song without a title.
    assert_eq!(
        file.songs
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        ["Pink + White", "Only Downloaded", "Album Track"]
    );
}

#[test]
fn the_phones_database_is_read_by_the_apps_own_rules() {
    let dir = scratch("database");
    let db = dir.join("stash.db");
    phone_database(&db);
    let before = std::fs::read(&db).unwrap();

    let file = read_file(&db).unwrap();
    check(&file);
    assert_eq!(
        file.generator.as_deref(),
        Some("the Stash app's library (version 80 of its database)")
    );
    // The file handed in was read as a copy: it is as it was, with nothing left beside it.
    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    // Into a library: nothing of the phone's own places comes along.
    let mut library = Library::open_in_memory().unwrap();
    let summary = library.import(&file).unwrap();
    // Pink + White, Nights, Ivy, Shared Tune, Only Downloaded, Album Track.
    assert_eq!(
        (
            summary.songs_added,
            summary.likes_added,
            summary.playlists_added,
            summary.plays_added
        ),
        (6, 2, 3, 2)
    );
    let out = library.export(1, "test").unwrap().to_string();
    assert!(
        !out.contains("/storage/") && !out.contains("Blocked Song") && !out.contains("Mix Filler")
    );
    let again = library.import(&file).unwrap();
    assert_eq!(
        (
            again.songs_added,
            again.likes_added,
            again.playlists_added,
            again.plays_added
        ),
        (0, 0, 0, 0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_apps_backup_archive_is_read_and_what_is_not_a_library_is_refused() {
    let dir = scratch("backup");
    let db = dir.join("stash.db");
    phone_database(&db);
    let zipped = |entries: &[(&str, Vec<u8>)]| {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    };
    let manifest = br#"{"dbSchemaVersion":80,"exportTimestamp":1,"appVersionName":"0.9.125","scope":"EVERYTHING"}"#.to_vec();

    let backup = zipped(&[
        ("manifest.json", manifest.clone()),
        ("stash.db", std::fs::read(&db).unwrap()),
        ("datastore/settings.preferences_pb", vec![1, 2, 3]),
    ]);
    let file = read_bytes(&backup).unwrap();
    check(&file);
    assert_eq!(
        file.generator.as_deref(),
        Some("a backup of Stash for Android 0.9.125")
    );
    // The same archive as a file on disk.
    let on_disk = dir.join("stash-backup.zip");
    std::fs::write(&on_disk, &backup).unwrap();
    check(&read_file(&on_disk).unwrap());
    // The database's own bytes, as the window would hand them over.
    check(&read_bytes(&std::fs::read(&db).unwrap()).unwrap());

    // A backup of the settings alone has no library in it.
    let settings_only = zipped(&[
        ("manifest.json", manifest),
        ("datastore/settings.preferences_pb", vec![1, 2, 3]),
    ]);
    assert!(
        matches!(read_bytes(&settings_only), Err(ImportError::Unreadable(why)) if why.contains("holds no library"))
    );
    // Some other zip, some other database, and something that only starts like one.
    assert!(matches!(
        read_bytes(&zipped(&[("readme.txt", b"hello".to_vec())])),
        Err(ImportError::Unreadable(_))
    ));
    let other = dir.join("other.db");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT);")
        .unwrap();
    assert!(
        matches!(read_file(&other), Err(ImportError::Unreadable(why)) if why == "That file is not a Stash library")
    );
    assert!(matches!(
        read_bytes(b"PK\x03\x04 not really a zip"),
        Err(ImportError::Unreadable(_))
    ));
    assert!(matches!(
        read_bytes(b"SQLite format 3\0 and then nothing of the kind"),
        Err(ImportError::Unreadable(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
