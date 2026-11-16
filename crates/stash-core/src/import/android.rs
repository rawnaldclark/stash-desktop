//! Reading the Stash Android app's own library: its database (`stash.db`), or the backup archive the app writes
//! (Settings › Library & Storage › Backup), which holds that database.
//!
//! What is taken is what the app's "Export for Stash on the web" takes, by the app's own rules: the liked songs (a
//! Stash like, or a place in a Liked Songs list mirrored from Spotify or YouTube Music), the listener's own playlists
//! in order (a followed shared mix as a follow), and the plays. On top of that, because a database holds more than
//! that export does: every play (not the newest 5,000), the downloaded songs, and the songs of saved albums. Those
//! come in as songs of the library, in no list.
//!
//! What is never taken: Daily Mixes and Stash Mixes (the app makes them anew), blocked songs, file paths, download
//! state, logins, settings. The database is opened as a copy; the file handed in is not written to.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{ImportError, Incoming, Playlist, Song};
use crate::identity::{clean_text, isrc};

const TEXT: usize = 300;
const NAME: usize = 100;
const MAX_PLAYLIST_ITEMS: usize = 10_000;
/// The largest database taken out of a backup archive.
const MAX_DATABASE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How every SQLite file begins.
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

fn not_a_library() -> ImportError {
    ImportError::Unreadable("That file is not a Stash library".into())
}

fn unreadable(e: impl std::fmt::Display) -> ImportError {
    ImportError::Unreadable(format!("That library could not be read ({e})"))
}

/// A folder of its own for the copy of a database, removed when the reading is done.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, ImportError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("stash-import-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(unreadable)?;
        Ok(Self(dir))
    }

    fn database(&self) -> PathBuf {
        self.0.join("stash.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Reads the app's database at `path`. A database in use has its newest writes in the files beside it (`-wal`,
/// `-shm`): they are copied along when they are there.
pub fn read_database(path: &Path, now_ms: i64) -> Result<Incoming, ImportError> {
    let scratch = Scratch::new()?;
    std::fs::copy(path, scratch.database()).map_err(unreadable)?;
    for suffix in ["-wal", "-shm"] {
        let beside = PathBuf::from(format!("{}{suffix}", path.display()));
        if beside.is_file() {
            std::fs::copy(&beside, format!("{}{suffix}", scratch.database().display()))
                .map_err(unreadable)?;
        }
    }
    read_copy(&scratch, now_ms, None)
}

/// Reads the app's database from its bytes.
pub fn read_database_bytes(bytes: &[u8], now_ms: i64) -> Result<Incoming, ImportError> {
    let scratch = Scratch::new()?;
    std::fs::write(scratch.database(), bytes).map_err(unreadable)?;
    read_copy(&scratch, now_ms, None)
}

/// Reads the app's backup archive: a zip holding `manifest.json` and `stash.db`. A backup made without the library
/// (settings only) has no database in it and is refused.
pub fn read_backup(archive: impl Read + Seek, now_ms: i64) -> Result<Incoming, ImportError> {
    let mut zip = zip::ZipArchive::new(archive).map_err(|_| not_a_library())?;
    let version = zip.by_name("manifest.json").ok().and_then(|mut manifest| {
        let mut text = String::new();
        (&mut manifest)
            .take(64 * 1024)
            .read_to_string(&mut text)
            .ok()?;
        let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
        manifest
            .get("appVersionName")?
            .as_str()
            .map(|v| clean_text(v, 40))
    });
    let scratch = Scratch::new()?;
    {
        let database = zip.by_name("stash.db").map_err(|_| {
            ImportError::Unreadable(
                "That backup holds no library (it was made with settings only)".into(),
            )
        })?;
        let mut out = std::fs::File::create(scratch.database()).map_err(unreadable)?;
        let copied = std::io::copy(&mut database.take(MAX_DATABASE_BYTES + 1), &mut out)
            .map_err(unreadable)?;
        if copied > MAX_DATABASE_BYTES {
            return Err(ImportError::TooBig);
        }
    }
    let generator = match version {
        Some(version) if !version.is_empty() => format!("a backup of Stash for Android {version}"),
        _ => "a backup of Stash for Android".to_string(),
    };
    read_copy(&scratch, now_ms, Some(generator))
}

/// What the database at hand has: an older app's database lacks columns and tables a newer one has.
struct Shape {
    columns: HashMap<&'static str, HashSet<String>>,
}

impl Shape {
    fn of(conn: &Connection) -> Result<Self, ImportError> {
        let mut columns = HashMap::new();
        for table in [
            "tracks",
            "playlists",
            "playlist_tracks",
            "listening_events",
            "track_blocklist",
            "shared_mixes",
        ] {
            let names: HashSet<String> = conn
                .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect())
                .map_err(|_| not_a_library())?;
            columns.insert(table, names);
        }
        Ok(Self { columns })
    }

    fn has(&self, table: &str, column: &str) -> bool {
        self.columns
            .get(table)
            .is_some_and(|names| names.contains(column))
    }

    fn has_all(&self, table: &str, columns: &[&str]) -> bool {
        columns.iter().all(|column| self.has(table, column))
    }

    /// The column, or NULL when this database does not have it.
    fn column(&self, table: &str, alias: &str, column: &str) -> String {
        if self.has(table, column) {
            format!("{alias}.{column}")
        } else {
            "NULL".to_string()
        }
    }
}

/// A song from a row that starts with the song's columns (see `song_columns`). `None` for a row the app's own export
/// leaves out too: no title or artist, or a SoundCloud upload.
fn song(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Song>> {
    let text = |index: usize| -> rusqlite::Result<Option<String>> {
        Ok(row
            .get::<_, Option<String>>(index)?
            .map(|t| clean_text(&t, TEXT))
            .filter(|t| !t.is_empty()))
    };
    let (Some(title), Some(artist)) = (text(0)?, text(1)?) else {
        return Ok(None);
    };
    if row.get::<_, Option<String>>(9)?.as_deref() == Some("SOUNDCLOUD") {
        return Ok(None);
    }
    let made_of = |id: &str, length: usize, also: &[u8]| {
        id.len() == length
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || also.contains(&b))
    };
    let mut refs = Vec::new();
    if let Some(youtube) = row
        .get::<_, Option<String>>(6)?
        .filter(|id| made_of(id, 11, b"_-"))
    {
        refs.push(("youtube".to_string(), youtube));
    }
    // `spotify:track:ID`, or the song's page on open.spotify.com.
    let spotify = row.get::<_, Option<String>>(5)?.and_then(|uri| {
        let uri = uri.trim();
        let id = uri
            .strip_prefix("spotify:track:")
            .or_else(|| uri.strip_prefix("https://open.spotify.com/track/"))?;
        Some(id.split(['?', '/']).next().unwrap_or_default().to_string())
    });
    if let Some(spotify) = spotify.filter(|id| made_of(id, 22, b"")) {
        refs.push(("spotify".to_string(), spotify));
    }
    let art_url = row.get::<_, Option<String>>(7)?.filter(|address| {
        address.len() <= 2_048
            && !address
                .chars()
                .any(|c| c.is_control() || c == ' ' || c == '\\')
            && url::Url::parse(address)
                .is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
    });
    Ok(Some(Song {
        title,
        artist,
        album: text(2)?,
        duration_ms: row
            .get::<_, Option<i64>>(3)?
            .filter(|ms| *ms > 0 && *ms <= 86_400_000)
            .map(|ms| ms as u64),
        isrc: row
            .get::<_, Option<String>>(4)?
            .and_then(|code| isrc(code.trim())),
        explicit: row.get::<_, Option<bool>>(10)?,
        refs,
        art_url,
        added_at_ms: row.get::<_, Option<i64>>(8)?.filter(|ms| *ms > 0),
    }))
}

fn read_copy(
    scratch: &Scratch,
    now_ms: i64,
    generator: Option<String>,
) -> Result<Incoming, ImportError> {
    let conn = Connection::open(scratch.database()).map_err(|_| not_a_library())?;
    let shape = Shape::of(&conn)?;
    let needed = shape.has_all(
        "tracks",
        &[
            "id",
            "title",
            "artist",
            "album",
            "duration_ms",
            "date_added",
            "source",
        ],
    ) && shape.has_all(
        "playlists",
        &["id", "name", "type", "source_id", "is_active", "date_added"],
    ) && shape.has_all(
        "playlist_tracks",
        &["playlist_id", "track_id", "position", "added_at"],
    );
    if !needed {
        return Err(not_a_library());
    }
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(unreadable)?;

    let track = |column: &str| shape.column("tracks", "t", column);
    // The eleven columns `song` reads, in its order.
    let song_columns = format!(
        "t.title, t.artist, t.album, t.duration_ms, {}, {}, {}, {}, t.date_added, t.source, {}",
        track("isrc"),
        track("spotify_uri"),
        track("youtube_id"),
        track("album_art_url"),
        track("explicit"),
    );
    // A song the listener blocked never leaves the phone: the app's three keys.
    let not_blocked = if shape.has_all(
        "track_blocklist",
        &["canonical_key", "spotify_uri", "youtube_id"],
    ) && shape.has_all(
        "tracks",
        &[
            "canonical_artist",
            "canonical_title",
            "spotify_uri",
            "youtube_id",
        ],
    ) {
        "NOT EXISTS (SELECT 1 FROM track_blocklist bl
                     WHERE bl.canonical_key = (t.canonical_artist || '|' || t.canonical_title)
                        OR (bl.spotify_uri IS NOT NULL AND bl.spotify_uri = t.spotify_uri)
                        OR (bl.youtube_id IS NOT NULL AND bl.youtube_id = t.youtube_id))"
    } else {
        "1"
    };
    let live = if shape.has("playlist_tracks", "removed_at") {
        "pt.removed_at IS NULL"
    } else {
        "1"
    };
    let in_liked_list = format!(
        "EXISTS (SELECT 1 FROM playlist_tracks pt JOIN playlists p ON p.id = pt.playlist_id
                 WHERE pt.track_id = t.id AND {live} AND p.type IN ('LIKED_SONGS', 'STASH_LIKED'))"
    );
    let stash_like = track("stash_liked_at");
    // A saved album is kept as a list in the app, and is not a playlist.
    let saved_album = "(p.type = 'CUSTOM' AND ((p.source_id LIKE 'album:%' AND p.source_id NOT LIKE 'album:%playlist:%')
                        OR (p.source = 'YOUTUBE' AND p.source_id LIKE 'MPREb%')))";
    let saved_album = if shape.has("playlists", "source") {
        saved_album
    } else {
        "0"
    };

    let mut incoming = Incoming {
        source: "android-library",
        generator: Some(generator.unwrap_or_else(|| {
            format!("the Stash app's library (version {version} of its database)")
        })),
        ..Incoming::default()
    };
    let read = |e: rusqlite::Error| unreadable(e);

    // Likes, newest first: a Stash like, else the earliest time the song entered a Liked Songs list.
    {
        let sql = format!(
            "SELECT {song_columns}, COALESCE({stash_like}, (
                 SELECT MIN(pt.added_at) FROM playlist_tracks pt JOIN playlists p ON p.id = pt.playlist_id
                 WHERE pt.track_id = t.id AND {live} AND p.type IN ('LIKED_SONGS', 'STASH_LIKED'))) AS liked_at
             FROM tracks t
             WHERE ({stash_like} IS NOT NULL OR {in_liked_list}) AND {not_blocked}
             ORDER BY liked_at DESC, t.id"
        );
        let mut stmt = conn.prepare(&sql).map_err(read)?;
        let mut rows = stmt.query([]).map_err(read)?;
        while let Some(row) = rows.next().map_err(read)? {
            if let Some(item) = song(row).map_err(read)? {
                let liked_at = row
                    .get::<_, Option<i64>>(11)
                    .map_err(read)?
                    .filter(|ms| *ms > 0);
                let liked_at = liked_at.or(item.added_at_ms).unwrap_or(now_ms);
                incoming.likes.push((item, liked_at));
            }
        }
    }

    // Followed shared mixes, by the playlist that holds them.
    let mut follows: HashMap<i64, (String, i64, Option<String>)> = HashMap::new();
    if shape.has_all(
        "shared_mixes",
        &["playlist_id", "share_id", "role", "status", "version"],
    ) {
        let by = shape.column("shared_mixes", "m", "shared_by");
        let sql = format!(
            "SELECT m.playlist_id, m.share_id, m.version, {by} FROM shared_mixes m
             WHERE m.role = 'FOLLOWER' AND m.status = 'ACTIVE'"
        );
        let mut stmt = conn.prepare(&sql).map_err(read)?;
        let mut rows = stmt.query([]).map_err(read)?;
        while let Some(row) = rows.next().map_err(read)? {
            let share: String = row.get(1).map_err(read)?;
            let version: i64 = row.get(2).map_err(read)?;
            if share.len() == 8 && share.bytes().all(|b| b.is_ascii_alphanumeric()) && version >= 1
            {
                let by = row
                    .get::<_, Option<String>>(3)
                    .map_err(read)?
                    .map(|b| clean_text(&b, 40))
                    .filter(|b| !b.is_empty());
                follows.insert(row.get(0).map_err(read)?, (share, version, by));
            }
        }
    }

    // The listener's own playlists, oldest first, as the app's export picks them: not a mix the app makes, not a
    // saved album, and (for a playlist that only mirrors another service) only one that is synced, pinned or holds
    // something that plays.
    {
        let kept = if shape.has_all("playlists", &["sync_enabled", "pinned_to_home_at"])
            && shape.has_all(
                "tracks",
                &["is_downloaded", "is_streamable", "is_streamable_checked_at"],
            ) {
            format!(
                "AND (p.sync_enabled = 1 OR p.pinned_to_home_at IS NOT NULL
                      OR p.source_id LIKE 'share:%' OR p.source_id LIKE 'custom_%'
                      OR EXISTS (SELECT 1 FROM playlist_tracks pt JOIN tracks t ON pt.track_id = t.id
                                 WHERE pt.playlist_id = p.id AND {live}
                                   AND (t.is_downloaded = 1 OR t.is_streamable = 1 OR t.is_streamable_checked_at IS NULL)))"
            )
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT p.id, p.name, p.source_id, p.date_added FROM playlists p
             WHERE p.type = 'CUSTOM' AND p.is_active = 1 AND NOT {saved_album} {kept}
             ORDER BY p.date_added, p.id"
        );
        let lists: Vec<(i64, String, String, i64)> = conn
            .prepare(&sql)
            .and_then(|mut stmt| {
                stmt.query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    ))
                })?
                .collect()
            })
            .map_err(read)?;
        let items_sql = format!(
            "SELECT {song_columns}, pt.added_at FROM playlist_tracks pt JOIN tracks t ON t.id = pt.track_id
             WHERE pt.playlist_id = ?1 AND {live} AND {not_blocked}
             ORDER BY pt.position, pt.added_at, t.id LIMIT {MAX_PLAYLIST_ITEMS}"
        );
        let mut items_of = conn.prepare(&items_sql).map_err(read)?;
        for (id, name, source_id, date_added) in lists {
            let mut items = Vec::new();
            let mut newest: Option<i64> = None;
            let mut rows = items_of.query([id]).map_err(read)?;
            while let Some(row) = rows.next().map_err(read)? {
                newest = newest.max(row.get::<_, Option<i64>>(11).map_err(read)?);
                if let Some(item) = song(row).map_err(read)? {
                    items.push(item);
                }
            }
            let created = if date_added > 0 { date_added } else { now_ms };
            // A playlist that came to the phone from Stash on the web goes on under the web's own id for it, so it
            // is one playlist everywhere; any other is `app-<its number>`, as in the app's own export.
            let from_web = source_id.strip_prefix("custom_web_").filter(|web| {
                (1..=64).contains(&web.len())
                    && web
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            });
            let name = clean_text(&name, NAME);
            let follow = follows.get(&id);
            incoming.playlists.push(Playlist {
                external_id: from_web
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("app-{id}")),
                name: if name.is_empty() {
                    "Playlist".to_string()
                } else {
                    name
                },
                items,
                created_at_ms: Some(created),
                updated_at_ms: Some(newest.unwrap_or(created).max(created)),
                follow_id: follow.map(|(share, _, _)| share.clone()),
                follow_version: follow.map(|(_, version, _)| *version),
                // The phone has no smart playlists.
                rules: None,
                follow_shared_by: follow.and_then(|(_, _, by)| by.clone()),
            });
        }
    }

    // Every play, newest first, stamped with when it started.
    if shape.has_all("listening_events", &["track_id", "started_at"]) {
        let sql = format!(
            "SELECT {song_columns}, le.started_at FROM listening_events le JOIN tracks t ON t.id = le.track_id
             WHERE le.started_at > 0 AND {not_blocked} ORDER BY le.started_at DESC, le.id DESC"
        );
        let mut stmt = conn.prepare(&sql).map_err(read)?;
        let mut rows = stmt.query([]).map_err(read)?;
        while let Some(row) = rows.next().map_err(read)? {
            if let Some(item) = song(row).map_err(read)? {
                incoming.plays.push((item, row.get(11).map_err(read)?));
            }
        }
    }

    // The rest of what the listener chose to have: downloaded songs, and the songs of saved albums.
    {
        let downloaded = if shape.has("tracks", "is_downloaded") {
            "t.is_downloaded = 1"
        } else {
            "0"
        };
        let sql = format!(
            "SELECT {song_columns} FROM tracks t
             WHERE ({downloaded} OR EXISTS (
                        SELECT 1 FROM playlist_tracks pt JOIN playlists p ON p.id = pt.playlist_id
                        WHERE pt.track_id = t.id AND {live} AND p.is_active = 1 AND {saved_album}))
               AND {not_blocked}
             ORDER BY t.date_added, t.id"
        );
        let mut stmt = conn.prepare(&sql).map_err(read)?;
        let mut rows = stmt.query([]).map_err(read)?;
        while let Some(row) = rows.next().map_err(read)? {
            if let Some(item) = song(row).map_err(read)? {
                incoming.songs.push(item);
            }
        }
    }
    // Songs the listener chose not to play on the phone.
    if shape.has_all("track_blocklist", &["artist", "title"]) {
        let at = shape.column("track_blocklist", "bl", "blocked_at");
        let sql = format!("SELECT bl.title, bl.artist, {at} FROM track_blocklist bl ORDER BY {at}");
        let mut stmt = conn.prepare(&sql).map_err(read)?;
        let mut rows = stmt.query([]).map_err(read)?;
        while let Some(row) = rows.next().map_err(read)? {
            let title: Option<String> = row.get(0).map_err(read)?;
            let artist: Option<String> = row.get(1).map_err(read)?;
            let at: Option<i64> = row.get(2).map_err(read)?;
            let (title, artist) = (
                clean_text(title.as_deref().unwrap_or(""), 300),
                clean_text(artist.as_deref().unwrap_or(""), 300),
            );
            if !title.is_empty() && !artist.is_empty() {
                incoming.blocked.push((title, artist, at.unwrap_or(now_ms)));
            }
        }
    }
    Ok(incoming)
}
