//! Opening the library database and bringing its schema up to date.

use rusqlite::Connection;

use crate::{Error, Result};

/// The schema this build writes. A database with a higher number was written by a newer build and is refused:
/// a library is never opened by code that does not know its shape.
pub const SCHEMA_VERSION: i64 = 1;

/// Settings for a library on a desktop disk: WAL so readers (the UI, a remote) never wait for a writer (a scan, an
/// import), `synchronous = NORMAL` (safe with WAL: a power cut can lose the last transaction, never corrupt the file).
const PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA foreign_keys = ON;
    PRAGMA temp_store = MEMORY;
    PRAGMA cache_size = -65536;
    PRAGMA mmap_size = 268435456;
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
