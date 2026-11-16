//! Opening the library database and bringing its schema up to date.

use rusqlite::Connection;

use crate::{Error, Result};

/// The schema this build writes. A database with a higher number was written by a newer build and is refused:
/// a library is never opened by code that does not know its shape.
pub const SCHEMA_VERSION: i64 = 18;

/// Settings for a library on a desktop disk: WAL so readers (the UI, a remote) never wait for a writer (a scan, an
/// import), `synchronous = NORMAL` (safe with WAL: a power cut can lose the last transaction, never corrupt the file).
/// A scan writes through a connection of its own; a writer that finds the other one writing waits its turn.
const PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA foreign_keys = ON;
    PRAGMA temp_store = MEMORY;
    PRAGMA cache_size = -65536;
    PRAGMA mmap_size = 268435456;
    PRAGMA busy_timeout = 5000;
";

const V1: &str = "
    CREATE TABLE artists(
        id        INTEGER PRIMARY KEY,
        name      TEXT NOT NULL,
        sort_key  TEXT NOT NULL
    );
    CREATE UNIQUE INDEX artists_name ON artists(name);
    CREATE INDEX artists_sort ON artists(sort_key, id);

    CREATE TABLE albums(
        id        INTEGER PRIMARY KEY,
        title     TEXT NOT NULL,
        artist_id INTEGER NOT NULL REFERENCES artists(id),
        year      INTEGER,
        sort_key  TEXT NOT NULL
    );
    CREATE INDEX albums_artist ON albums(artist_id);
    CREATE INDEX albums_sort ON albums(sort_key, id);

    -- One row per song the listener has, wherever it plays from. `origin` says where: a file on this computer, or a
    -- track an add-on serves. `locator` is the path, or the add-on's own id for the track.
    CREATE TABLE tracks(
        id             INTEGER PRIMARY KEY,
        title          TEXT NOT NULL,
        artist_id      INTEGER NOT NULL REFERENCES artists(id),
        album_id       INTEGER REFERENCES albums(id),
        disc_no        INTEGER NOT NULL DEFAULT 1,
        track_no       INTEGER NOT NULL DEFAULT 0,
        duration_ms    INTEGER NOT NULL DEFAULT 0,
        isrc           TEXT,
        origin         TEXT NOT NULL CHECK (origin IN ('file', 'addon')),
        addon_id       TEXT,
        locator        TEXT NOT NULL,
        codec          TEXT,
        sample_rate    INTEGER,
        bit_depth      INTEGER,
        added_at       INTEGER NOT NULL,
        play_count     INTEGER NOT NULL DEFAULT 0,
        last_played_at INTEGER,
        sort_key       TEXT NOT NULL
    );
    CREATE INDEX tracks_album ON tracks(album_id, disc_no, track_no);
    CREATE INDEX tracks_artist ON tracks(artist_id);
    CREATE INDEX tracks_sort ON tracks(sort_key, id);
    CREATE INDEX tracks_added ON tracks(added_at DESC, id);

    -- Full-text search over what a listener types: title, artist, album. rowid = tracks.id. The prefix indexes make
    -- search-as-you-type cheap from the second letter on.
    CREATE VIRTUAL TABLE tracks_fts USING fts5(
        title, artist, album,
        tokenize = 'unicode61 remove_diacritics 2',
        prefix = '2 3 4'
    );
";

/// Version 2: what a folder scan needs. A file's size and time tell a scan whether to read its tags again; a file
/// that is not found is marked missing, never deleted; and one file (or one add-on's track) is one row.
const V2: &str = "
    ALTER TABLE tracks ADD COLUMN file_size INTEGER;
    ALTER TABLE tracks ADD COLUMN file_mtime INTEGER;
    ALTER TABLE tracks ADD COLUMN missing_since INTEGER;
    CREATE UNIQUE INDEX tracks_locator ON tracks(origin, IFNULL(addon_id, ''), locator);
";

/// Version 3: the add-ons the listener added. `link` is the address of the add-on's manifest (it can carry the
/// listener's own key, so it is never shown whole); `manifest` is the manifest as last read, so the add-on is there
/// when the computer is offline; `position` is the listener's order: the first add-on is asked first.
const V3: &str = "
    CREATE TABLE addons(
        id        TEXT PRIMARY KEY,
        link      TEXT NOT NULL,
        manifest  TEXT NOT NULL,
        position  INTEGER NOT NULL,
        enabled   INTEGER NOT NULL DEFAULT 1,
        added_at  INTEGER NOT NULL
    );
";

/// Version 4: a library that holds more than files. A song can be here by its description alone (`origin` =
/// 'described': brought in from the phone or the web player, played through add-ons), so `tracks` is made again with
/// that third origin; the rule that one file is one row no longer covers described songs, which have no locator.
/// `words_key` is the song's identity by its words ([`crate::identity`]). `track_refs` holds what other sources call
/// a song. Likes, playlists and plays are the listener's own, with their times in milliseconds as the other Stash
/// apps keep them.
const V4: &str = "
    CREATE TABLE tracks_v4(
        id             INTEGER PRIMARY KEY,
        title          TEXT NOT NULL,
        artist_id      INTEGER NOT NULL REFERENCES artists(id),
        album_id       INTEGER REFERENCES albums(id),
        disc_no        INTEGER NOT NULL DEFAULT 1,
        track_no       INTEGER NOT NULL DEFAULT 0,
        duration_ms    INTEGER NOT NULL DEFAULT 0,
        isrc           TEXT,
        origin         TEXT NOT NULL CHECK (origin IN ('file', 'addon', 'described')),
        addon_id       TEXT,
        locator        TEXT NOT NULL,
        codec          TEXT,
        sample_rate    INTEGER,
        bit_depth      INTEGER,
        added_at       INTEGER NOT NULL,
        play_count     INTEGER NOT NULL DEFAULT 0,
        last_played_at INTEGER,
        sort_key       TEXT NOT NULL,
        file_size      INTEGER,
        file_mtime     INTEGER,
        missing_since  INTEGER,
        words_key      TEXT NOT NULL DEFAULT '',
        explicit       INTEGER,
        art_url        TEXT,
        liked_at_ms    INTEGER
    );
    INSERT INTO tracks_v4(id, title, artist_id, album_id, disc_no, track_no, duration_ms, isrc, origin, addon_id, locator,
                          codec, sample_rate, bit_depth, added_at, play_count, last_played_at, sort_key,
                          file_size, file_mtime, missing_since)
        SELECT id, title, artist_id, album_id, disc_no, track_no, duration_ms, isrc, origin, addon_id, locator,
               codec, sample_rate, bit_depth, added_at, play_count, last_played_at, sort_key,
               file_size, file_mtime, missing_since
        FROM tracks;
    DROP TABLE tracks;
    ALTER TABLE tracks_v4 RENAME TO tracks;
    CREATE INDEX tracks_album ON tracks(album_id, disc_no, track_no);
    CREATE INDEX tracks_artist ON tracks(artist_id);
    CREATE INDEX tracks_sort ON tracks(sort_key, id);
    CREATE INDEX tracks_added ON tracks(added_at DESC, id);
    CREATE UNIQUE INDEX tracks_locator ON tracks(origin, IFNULL(addon_id, ''), locator) WHERE origin <> 'described';
    CREATE INDEX tracks_words ON tracks(words_key);
    CREATE INDEX tracks_isrc ON tracks(isrc) WHERE isrc IS NOT NULL;
    CREATE INDEX tracks_liked ON tracks(liked_at_ms DESC, id) WHERE liked_at_ms IS NOT NULL;

    CREATE TABLE track_refs(
        track_id  INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        source    TEXT NOT NULL,
        ref       TEXT NOT NULL,
        PRIMARY KEY (track_id, source)
    ) WITHOUT ROWID;

    CREATE TABLE playlists(
        id             INTEGER PRIMARY KEY,
        external_id    TEXT UNIQUE,
        name           TEXT NOT NULL,
        created_at_ms  INTEGER,
        updated_at_ms  INTEGER,
        follow_id      TEXT,
        position       INTEGER NOT NULL
    );
    CREATE TABLE playlist_items(
        playlist_id  INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
        position     INTEGER NOT NULL,
        track_id     INTEGER NOT NULL REFERENCES tracks(id),
        PRIMARY KEY (playlist_id, position)
    ) WITHOUT ROWID;
    CREATE INDEX playlist_items_track ON playlist_items(track_id);

    CREATE TABLE plays(
        track_id      INTEGER NOT NULL REFERENCES tracks(id),
        played_at_ms  INTEGER NOT NULL,
        PRIMARY KEY (track_id, played_at_ms)
    ) WITHOUT ROWID;
    CREATE INDEX plays_time ON plays(played_at_ms DESC);
";

/// Version 5: a shared mix someone else owns keeps its version and who shared it, so it can be written out again as
/// the follow it is.
const V5: &str = "
    ALTER TABLE playlists ADD COLUMN follow_version INTEGER;
    ALTER TABLE playlists ADD COLUMN follow_shared_by TEXT;
";

/// Version 6: what the listener set (the volume, whether Stash takes the sound card for itself), and the music
/// folders they added, so "scan again" knows where to look.
const V6: &str = "
    CREATE TABLE settings(
        key    TEXT PRIMARY KEY,
        value  TEXT NOT NULL
    ) WITHOUT ROWID;
    CREATE TABLE folders(
        path           TEXT PRIMARY KEY,
        added_at       INTEGER NOT NULL,
        last_scan_at   INTEGER
    ) WITHOUT ROWID;
";

/// Whether a file has a picture (inside it or beside it): unknown until its picture is first asked for.
const V7: &str = "
    ALTER TABLE tracks ADD COLUMN cover INTEGER;
";

/// Lyrics found on the network, by song (its ISRC, else its words): the lyrics as JSON, or NULL for "none there",
/// where they came from, and when (Unix seconds), so a song is asked for once and a miss is asked again later.
const V8: &str = "
    CREATE TABLE lyrics_cache(
        key    TEXT PRIMARY KEY,
        body   TEXT,
        via    TEXT,
        at     INTEGER NOT NULL
    ) WITHOUT ROWID;
";

/// Songs to save as files, and saved: queued, saving, saved (`path`, from `addon` at `tier`, `bytes` long) or
/// failed (`error` in words). Times in Unix milliseconds.
const V9: &str = "
    CREATE TABLE downloads(
        track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        state      TEXT NOT NULL CHECK (state IN ('queued', 'saving', 'saved', 'failed')),
        path       TEXT,
        addon      TEXT,
        tier       TEXT,
        bytes      INTEGER,
        error      TEXT,
        asked_at   INTEGER NOT NULL,
        saved_at   INTEGER
    );
    CREATE INDEX downloads_state ON downloads(state, asked_at);
";

/// Songs no add-on could find: every add-on that could be asked said it does not have the song. `aside_at` is when the
/// listener said none of an add-on's results is it: the song leaves the list and is still known not to play.
const V10: &str = "
    CREATE TABLE unfound(
        track_id  INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        first_at  INTEGER NOT NULL,
        last_at   INTEGER NOT NULL,
        aside_at  INTEGER
    );
    CREATE INDEX unfound_last ON unfound(last_at);
";

/// Copies of songs as they were played from an add-on (`played.rs`): a file in the copies' folder, its size, where it
/// came from, and when it was kept and last played (the least recently played go first when room is short).
const V11: &str = "
    CREATE TABLE played_copies(
        track_id    INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        file        TEXT NOT NULL,
        bytes       INTEGER NOT NULL,
        tier        TEXT,
        addon       TEXT,
        kept_at_ms  INTEGER NOT NULL,
        used_at_ms  INTEGER NOT NULL
    );
    CREATE INDEX played_copies_used ON played_copies(used_at_ms);
";

/// Each stream and download an add-on gave, with its time (`usage.rs`), kept for a week.
const V12: &str = "
    CREATE TABLE addon_events(
        addon  TEXT NOT NULL,
        kind   TEXT NOT NULL,
        at_ms  INTEGER NOT NULL
    );
    CREATE INDEX addon_events_at ON addon_events(at_ms);
";

/// Version 13: a smart playlist keeps its rules (`smart.rs`) and no songs of its own.
const V13: &str = "
    ALTER TABLE playlists ADD COLUMN rules TEXT;
";

/// Version 17: songs and artists the listener chose not to play (`blocks.rs`), each by its words: a song by its
/// `words_key`, an artist by its folded name.
const V17: &str = "
    CREATE TABLE blocked(
        kind   TEXT NOT NULL CHECK (kind IN ('song', 'artist')),
        key    TEXT NOT NULL,
        title  TEXT NOT NULL DEFAULT '',
        artist TEXT NOT NULL DEFAULT '',
        at_ms  INTEGER NOT NULL,
        PRIMARY KEY (kind, key)
    ) WITHOUT ROWID;
";

/// Version 18: playlist folders (`folders.rs`), this computer's own grouping of playlists. A folder deleted leaves its
/// playlists out of any.
const V18: &str = "
    CREATE TABLE playlist_folders(
        id        INTEGER PRIMARY KEY,
        name      TEXT NOT NULL,
        position  INTEGER NOT NULL,
        collapsed INTEGER NOT NULL DEFAULT 0
    );
    ALTER TABLE playlists ADD COLUMN folder_id INTEGER REFERENCES playlist_folders(id) ON DELETE SET NULL;
";

/// Version 16: what a list kept on this computer leaves alone. A song the listener took off the downloads is
/// remembered (`download_declined`), so a kept list does not put it back; and a song a list put on the downloads says
/// so (`by_list`), so it is taken off again when no kept list holds it, while one asked for by hand stays.
const V16: &str = "
    CREATE TABLE download_declined(track_id INTEGER PRIMARY KEY);
    ALTER TABLE downloads ADD COLUMN by_list INTEGER NOT NULL DEFAULT 0;
";

/// Version 15: a playlist can be kept on this computer (`download`): its songs are saved as files, and so are songs
/// that come onto it later. The liked songs' choice is a setting (`download.liked`).
const V15: &str = "
    ALTER TABLE playlists ADD COLUMN download INTEGER NOT NULL DEFAULT 0;
";

/// Version 14: the services the listener's plays go to (ListenBrainz, or a server that speaks its language), and the
/// plays waiting for each (`scrobble.rs`). A service's token is kept here, as an add-on's link is.
const V14: &str = "
    CREATE TABLE scrobble_targets(
        id            INTEGER PRIMARY KEY,
        kind          TEXT NOT NULL,
        url           TEXT NOT NULL,
        token         TEXT NOT NULL,
        user          TEXT,
        enabled       INTEGER NOT NULL DEFAULT 1,
        refused       INTEGER NOT NULL DEFAULT 0,
        added_at_ms   INTEGER NOT NULL,
        sent          INTEGER NOT NULL DEFAULT 0,
        sent_at_ms    INTEGER,
        dropped       INTEGER NOT NULL DEFAULT 0,
        trouble       TEXT,
        trouble_at_ms INTEGER,
        UNIQUE(kind, url)
    );
    CREATE TABLE scrobbles(
        id          INTEGER PRIMARY KEY,
        target      INTEGER NOT NULL REFERENCES scrobble_targets(id) ON DELETE CASCADE,
        listened_at INTEGER NOT NULL,
        song        TEXT NOT NULL,
        UNIQUE(target, listened_at, song)
    );
";

/// Opens (or creates) the library at `path` and migrates it to [`SCHEMA_VERSION`].
pub fn open(path: &std::path::Path) -> Result<Connection> {
    prepare(Connection::open(path)?)
}

/// A library that lives only in memory: for tests.
pub fn open_in_memory() -> Result<Connection> {
    prepare(Connection::open_in_memory()?)
}

fn prepare(mut conn: Connection) -> Result<Connection> {
    conn.execute_batch(PRAGMAS)?;
    // Text as smart playlists compare it: no case, no accents (`smart::plain`).
    conn.create_scalar_function(
        "stash_plain",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            Ok(ctx
                .get::<Option<String>>(0)?
                .map(|text| crate::smart::plain(&text)))
        },
    )?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(Error::NewerSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(V1)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    if found < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(V2)?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    if found < 3 {
        let tx = conn.transaction()?;
        tx.execute_batch(V3)?;
        tx.pragma_update(None, "user_version", 3)?;
        tx.commit()?;
    }
    if found < 4 {
        let tx = conn.transaction()?;
        tx.execute_batch(V4)?;
        // Every song the library already has gets its identity by words.
        let rows: Vec<(i64, String, String)> = tx
            .prepare(
                "SELECT t.id, t.title, a.name FROM tracks t JOIN artists a ON a.id = t.artist_id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        {
            let mut set = tx.prepare("UPDATE tracks SET words_key = ?2 WHERE id = ?1")?;
            for (id, title, artist) in rows {
                set.execute(rusqlite::params![
                    id,
                    crate::identity::words_key(&title, &artist)
                ])?;
            }
        }
        tx.pragma_update(None, "user_version", 4)?;
        tx.commit()?;
    }
    if found < 5 {
        let tx = conn.transaction()?;
        tx.execute_batch(V5)?;
        tx.pragma_update(None, "user_version", 5)?;
        tx.commit()?;
    }
    if found < 6 {
        let tx = conn.transaction()?;
        tx.execute_batch(V6)?;
        tx.pragma_update(None, "user_version", 6)?;
        tx.commit()?;
    }
    if found < 7 {
        let tx = conn.transaction()?;
        tx.execute_batch(V7)?;
        tx.pragma_update(None, "user_version", 7)?;
        tx.commit()?;
    }
    if found < 8 {
        let tx = conn.transaction()?;
        tx.execute_batch(V8)?;
        tx.pragma_update(None, "user_version", 8)?;
        tx.commit()?;
    }
    if found < 9 {
        let tx = conn.transaction()?;
        tx.execute_batch(V9)?;
        tx.pragma_update(None, "user_version", 9)?;
        tx.commit()?;
    }
    if found < 10 {
        let tx = conn.transaction()?;
        tx.execute_batch(V10)?;
        tx.pragma_update(None, "user_version", 10)?;
        tx.commit()?;
    }
    if found < 11 {
        let tx = conn.transaction()?;
        tx.execute_batch(V11)?;
        tx.pragma_update(None, "user_version", 11)?;
        tx.commit()?;
    }
    if found < 12 {
        let tx = conn.transaction()?;
        tx.execute_batch(V12)?;
        tx.pragma_update(None, "user_version", 12)?;
        tx.commit()?;
    }
    if found < 13 {
        let tx = conn.transaction()?;
        tx.execute_batch(V13)?;
        tx.pragma_update(None, "user_version", 13)?;
        tx.commit()?;
    }
    if found < 14 {
        let tx = conn.transaction()?;
        tx.execute_batch(V14)?;
        tx.pragma_update(None, "user_version", 14)?;
        tx.commit()?;
    }
    if found < 15 {
        let tx = conn.transaction()?;
        tx.execute_batch(V15)?;
        tx.pragma_update(None, "user_version", 15)?;
        tx.commit()?;
    }
    if found < 16 {
        let tx = conn.transaction()?;
        tx.execute_batch(V16)?;
        tx.pragma_update(None, "user_version", 16)?;
        tx.commit()?;
    }
    if found < 17 {
        let tx = conn.transaction()?;
        tx.execute_batch(V17)?;
        tx.pragma_update(None, "user_version", 17)?;
        tx.commit()?;
    }
    if found < 18 {
        let tx = conn.transaction()?;
        tx.execute_batch(V18)?;
        tx.pragma_update(None, "user_version", 18)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_library_is_at_the_current_schema_and_has_full_text_search() {
        let conn = open_in_memory().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        // FTS5 is compiled in: the table exists and answers a MATCH.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tracks_fts WHERE tracks_fts MATCH 'x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn a_version_1_library_is_brought_up_with_its_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO artists(id, name, sort_key) VALUES (1, 'A', 'a');
             INSERT INTO tracks(title, artist_id, origin, locator, added_at, sort_key, play_count)
             VALUES ('Old', 1, 'file', 'C:/music/old.flac', 1, 'old', 7);",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let (version, plays, size): (i64, i64, Option<i64>) = conn
            .query_row("SELECT (SELECT user_version FROM pragma_user_version), play_count, file_size FROM tracks", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((version, plays, size), (SCHEMA_VERSION, 7, None));
        // The table was made again for version 4: the row kept its id, and has its identity by words.
        let (id, words): (i64, String) = conn
            .query_row("SELECT id, words_key FROM tracks", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((id, words.as_str()), (1, "old|a"));
        // One file is one row from now on.
        let again = conn.execute("INSERT INTO tracks(title, artist_id, origin, locator, added_at, sort_key) VALUES ('Dup', 1, 'file', 'C:/music/old.flac', 2, 'dup')", []);
        assert!(again.is_err());
    }

    #[test]
    fn a_library_from_a_newer_build_is_refused_not_opened() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        match migrate(&mut conn) {
            Err(Error::NewerSchema { found, supported }) => {
                assert_eq!((found, supported), (SCHEMA_VERSION + 1, SCHEMA_VERSION))
            }
            other => panic!("expected NewerSchema, got {other:?}"),
        }
    }
}
