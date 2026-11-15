//! What the listener does to their own library: liking, making and editing playlists, and the plays that counted.
//! And the way back out: the library written as the file the Android app and the web player read
//! (`stash-web-library`, version 1), so a like made here reaches the phone the way a like made there reached here.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::identity::clean_text;
use crate::import::Song;
use crate::library::Library;
use crate::{Error, Result};

/// The file's own limits, kept by this writer too.
const MAX_LIKES: usize = 50_000;
const MAX_PLAYLIST_ITEMS: usize = 10_000;
const MAX_PLAYS: usize = 5_000;
const NAME: usize = 100;

fn named(name: &str) -> Result<String> {
    let name = clean_text(name, NAME);
    if name.is_empty() {
        return Err(Error::Refused("A playlist needs a name".into()));
    }
    Ok(name)
}

/// `ms` since 1970 as JavaScript's `toISOString()` writes it: "2026-11-15T09:00:00.000Z".
fn iso_millis(ms: i64) -> String {
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days to a date: Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

impl Library {
    /// What the listener set for `key`, if anything.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// The music folders the listener added, in the order they were added.
    pub fn folders(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT path FROM folders ORDER BY added_at, path")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }

    /// Remembers a music folder, and when it was last scanned.
    pub fn remember_folder(&mut self, path: &str, now_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO folders(path, added_at, last_scan_at) VALUES (?1, ?2, ?2)
             ON CONFLICT(path) DO UPDATE SET last_scan_at = excluded.last_scan_at",
            params![path, now_ms],
        )?;
        Ok(())
    }

    /// Likes the song `id`, or takes the like back. False when there is no such song.
    pub fn set_liked(&mut self, id: i64, liked: bool, now_ms: i64) -> Result<bool> {
        let changed = if liked {
            // A song that is liked keeps the time it was first liked.
            self.conn.execute(
                "UPDATE tracks SET liked_at_ms = COALESCE(liked_at_ms, ?2) WHERE id = ?1",
                params![id, now_ms],
            )?
        } else {
            self.conn
                .execute("UPDATE tracks SET liked_at_ms = NULL WHERE id = ?1", [id])?
        };
        Ok(changed > 0)
    }

    /// Makes an empty playlist, last in the list. Its id for other Stashes is made here and never changes.
    pub fn create_playlist(&mut self, name: &str, now_ms: i64) -> Result<i64> {
        let name = named(name)?;
        let tx = self.conn.transaction()?;
        let next: i64 = tx.query_row("SELECT IFNULL(MAX(id), 0) + 1 FROM playlists", [], |r| {
            r.get(0)
        })?;
        tx.execute(
            "INSERT INTO playlists(id, external_id, name, created_at_ms, updated_at_ms, position)
             VALUES (?1, ?2, ?3, ?4, ?4, (SELECT IFNULL(MAX(position), 0) + 1 FROM playlists))",
            params![next, format!("desk-{now_ms}-{next}"), name, now_ms],
        )?;
        tx.commit()?;
        Ok(next)
    }

    pub fn rename_playlist(&mut self, id: i64, name: &str, now_ms: i64) -> Result<bool> {
        let name = named(name)?;
        Ok(self.conn.execute(
            "UPDATE playlists SET name = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![id, name, now_ms],
        )? > 0)
    }

    /// Deletes the playlist. Its songs stay in the library.
    pub fn delete_playlist(&mut self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM playlists WHERE id = ?1", [id])?
            > 0)
    }

    /// Adds songs at the end of a playlist, leaving out any it already has. Says how many were added.
    pub fn add_to_playlist(&mut self, id: i64, tracks: &[i64], now_ms: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut added = 0;
        {
            let exists: Option<i64> = tx
                .query_row("SELECT id FROM playlists WHERE id = ?1", [id], |r| r.get(0))
                .optional()?;
            if exists.is_none() {
                return Ok(0);
            }
            let mut next: i64 = tx.query_row(
                "SELECT IFNULL(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
                [id],
                |r| r.get(0),
            )?;
            let mut has = tx
                .prepare("SELECT 1 FROM playlist_items WHERE playlist_id = ?1 AND track_id = ?2")?;
            let mut known = tx.prepare("SELECT 1 FROM tracks WHERE id = ?1")?;
            let mut add = tx.prepare(
                "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for track in tracks {
                if has.exists(params![id, track])? || !known.exists([track])? {
                    continue;
                }
                add.execute(params![id, next, track])?;
                next += 1;
                added += 1;
            }
            if added > 0 {
                tx.execute(
                    "UPDATE playlists SET updated_at_ms = ?2 WHERE id = ?1",
                    params![id, now_ms],
                )?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Rewrites a playlist's order through `change`, which is handed its songs in order. False when `change` says
    /// there is nothing to do.
    fn reorder(
        &mut self,
        id: i64,
        now_ms: i64,
        change: impl FnOnce(&mut Vec<i64>) -> bool,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut items: Vec<i64> = tx
            .prepare(
                "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
            )?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if !change(&mut items) {
            return Ok(false);
        }
        tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
        {
            let mut add = tx.prepare(
                "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for (position, track) in items.iter().enumerate() {
                add.execute(params![id, position as i64, track])?;
            }
        }
        tx.execute(
            "UPDATE playlists SET updated_at_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Takes the song at place `index` (0 is the top) out of a playlist.
    pub fn remove_from_playlist(&mut self, id: i64, index: usize, now_ms: i64) -> Result<bool> {
        self.reorder(id, now_ms, |items| {
            if index >= items.len() {
                return false;
            }
            items.remove(index);
            true
        })
    }

    /// Moves the song at place `from` to place `to`.
    pub fn move_in_playlist(
        &mut self,
        id: i64,
        from: usize,
        to: usize,
        now_ms: i64,
    ) -> Result<bool> {
        self.reorder(id, now_ms, |items| {
            if from >= items.len() || to >= items.len() || from == to {
                return false;
            }
            let track = items.remove(from);
            items.insert(to, track);
            true
        })
    }

    /// A play that counted: the song `id`, started at `started_at_ms`. False when that play is already kept.
    pub fn record_play(&mut self, id: i64, started_at_ms: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO plays(track_id, played_at_ms)
             SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM tracks WHERE id = ?1)",
            params![id, started_at_ms],
        )?;
        if added > 0 {
            tx.execute(
                "UPDATE tracks SET play_count = play_count + 1, last_played_at = MAX(IFNULL(last_played_at, 0), ?2 / 1000)
                 WHERE id = ?1",
                params![id, started_at_ms],
            )?;
        }
        tx.commit()?;
        Ok(added > 0)
    }

    /// The library's row for a song met somewhere else (an add-on's search result the listener liked): the row it
    /// has for that song, or a new one known by its description.
    pub fn adopt(&mut self, song: &Song) -> Result<i64> {
        self.adopt_songs(std::slice::from_ref(song))
            .map(|ids| ids[0])
    }

    /// The library as the file other Stashes read: likes, playlists in order, and the newest plays, each song as its
    /// description. No file paths, no stream addresses, nothing that only works on this computer.
    pub fn export(&self, now_ms: i64, generator: &str) -> Result<Value> {
        let mut refs: HashMap<i64, Vec<(String, String)>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT track_id, source, ref FROM track_refs ORDER BY track_id, source",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                refs.entry(row.get(0)?)
                    .or_default()
                    .push((row.get(1)?, row.get(2)?));
            }
        }
        // A song as the file describes it, or `None` for one another Stash could not find: a file without tags.
        let song = |row: &rusqlite::Row<'_>| -> rusqlite::Result<Option<Value>> {
            let (id, title, artist): (i64, String, String) =
                (row.get(0)?, row.get(1)?, row.get(2)?);
            let origin: String = row.get(8)?;
            if origin == "file" && artist == crate::scan::UNKNOWN_ARTIST {
                return Ok(None);
            }
            let mut item = Map::new();
            item.insert("title".into(), json!(title));
            item.insert("artist".into(), json!(artist));
            if let Some(album) = row.get::<_, Option<String>>(3)? {
                item.insert("album".into(), json!(album));
            }
            let duration_ms: i64 = row.get(4)?;
            if duration_ms > 0 {
                item.insert("durationMs".into(), json!(duration_ms));
            }
            if let Some(isrc) = row.get::<_, Option<String>>(5)? {
                item.insert("isrc".into(), json!(isrc));
            }
            let mut named = Map::new();
            for (source, theirs) in refs.get(&id).map(Vec::as_slice).unwrap_or_default() {
                if source == "spotify" {
                    item.insert("spotifyId".into(), json!(theirs));
                } else {
                    named.insert(source.clone(), json!(theirs));
                }
            }
            if !named.is_empty() {
                item.insert("refs".into(), Value::Object(named));
            }
            if let Some(cover) = row.get::<_, Option<String>>(6)? {
                item.insert("artwork".into(), json!([{ "url": cover }]));
            }
            item.insert("addedAt".into(), json!(row.get::<_, i64>(7)? * 1000));
            Ok(Some(Value::Object(item)))
        };
        const SONG: &str = "t.id, t.title, a.name, al.title, t.duration_ms, t.isrc, t.art_url, t.added_at, t.origin";
        const JOINS: &str =
            "JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id";

        let mut likes = Vec::new();
        {
            let sql = format!(
                "SELECT {SONG}, t.liked_at_ms FROM tracks t {JOINS}
                 WHERE t.liked_at_ms IS NOT NULL ORDER BY t.liked_at_ms DESC, t.id LIMIT {MAX_LIKES}"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if let Some(item) = song(row)? {
                    likes.push(json!({ "item": item, "likedAt": row.get::<_, i64>(9)? }));
                }
            }
        }

        let mut playlists = Vec::new();
        {
            let sql = format!(
                "SELECT {SONG} FROM playlist_items i JOIN tracks t ON t.id = i.track_id {JOINS}
                 WHERE i.playlist_id = ?1 ORDER BY i.position LIMIT {MAX_PLAYLIST_ITEMS}"
            );
            let mut items_of = self.conn.prepare(&sql)?;
            let mut stmt = self.conn.prepare(
                "SELECT id, external_id, name, created_at_ms, updated_at_ms, follow_id, follow_version, follow_shared_by
                 FROM playlists WHERE external_id IS NOT NULL ORDER BY position, id",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let mut items = Vec::new();
                let mut songs = items_of.query([row.get::<_, i64>(0)?])?;
                while let Some(song_row) = songs.next()? {
                    if let Some(item) = song(song_row)? {
                        items.push(item);
                    }
                }
                let created: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(now_ms);
                let mut playlist = json!({
                    "id": row.get::<_, String>(1)?,
                    "name": row.get::<_, String>(2)?,
                    "items": items,
                    "createdAt": created,
                    "updatedAt": row.get::<_, Option<i64>>(4)?.unwrap_or(created),
                });
                // A shared mix someone else owns travels as a follow: the other side follows it again.
                if let Some(follow) = row.get::<_, Option<String>>(5)? {
                    let mut told = json!({ "id": follow, "version": row.get::<_, Option<i64>>(6)?.unwrap_or(1).max(1), "checkedAt": 0 });
                    if let Some(by) = row.get::<_, Option<String>>(7)? {
                        told["sharedBy"] = json!(by);
                    }
                    playlist["follow"] = told;
                }
                playlists.push(playlist);
            }
        }

        let mut history = Vec::new();
        {
            let sql = format!(
                "SELECT {SONG}, p.played_at_ms FROM plays p JOIN tracks t ON t.id = p.track_id {JOINS}
                 ORDER BY p.played_at_ms DESC, t.id LIMIT {MAX_PLAYS}"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                if let Some(item) = song(row)? {
                    history.push(json!({ "item": item, "playedAt": row.get::<_, i64>(9)? }));
                }
            }
        }

        Ok(json!({
            "kind": "stash-web-library",
            "v": 1,
            "exportedAt": iso_millis(now_ms),
            "generator": generator,
            "likes": likes,
            "playlists": playlists,
            "history": history,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_written_as_javascript_writes_them() {
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_millis(1_759_838_405_000), "2025-10-07T12:00:05.000Z");
        assert_eq!(iso_millis(1_709_251_199_999), "2024-02-29T23:59:59.999Z");
        assert_eq!(iso_millis(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_millis(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
    }
}
