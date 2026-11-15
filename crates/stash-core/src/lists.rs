//! What the listener does to their own library: liking, making and editing playlists, and the plays that counted.
//! And the way back out: the library written as the file the Android app and the web player read
//! (`stash-web-library`, version 1), so a like made here reaches the phone the way a like made there reached here.

use std::collections::{HashMap, HashSet};

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
    /// Lyrics kept for the song `key`: what was found (`None`: the song had none), where, and when.
    pub fn cached_lyrics(&self, key: &str) -> Result<Option<KeptLyrics>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .prepare_cached("SELECT body, via, at FROM lyrics_cache WHERE key = ?1")?
            .query_row([key], |r| {
                let body: Option<String> = r.get(0)?;
                Ok((body, r.get(1)?, r.get(2)?))
            })
            .optional()?
            .map(|(body, via, at)| KeptLyrics {
                lyrics: body.and_then(|b: String| serde_json::from_str(&b).ok()),
                via,
                at,
            }))
    }

    /// Keeps what was found for the song `key` (or that nothing was), with where and when (Unix seconds).
    pub fn keep_lyrics(
        &mut self,
        key: &str,
        lyrics: Option<&crate::lyrics::Lyrics>,
        via: Option<&str>,
        at: i64,
    ) -> Result<()> {
        let body = lyrics.and_then(|l| serde_json::to_string(l).ok());
        self.conn.execute(
            "INSERT INTO lyrics_cache(key, body, via, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET body = excluded.body, via = excluded.via, at = excluded.at",
            params![key, body, via, at],
        )?;
        Ok(())
    }

    // ---- Autoplay

    /// Songs to play after `seeds` (the last songs heard; `seed_artists` names their artists, for songs that are not
    /// the library's): by those artists first, then liked songs, then the most played, then any. None of `queued`,
    /// none heard since `rested_since_ms`, only files here when `files_only`. At most `n`, the same artist never twice
    /// in a row when that can be helped.
    pub fn autoplay_picks(
        &self,
        seeds: &[i64],
        seed_artists: &[String],
        queued: &HashSet<i64>,
        rested_since_ms: i64,
        files_only: bool,
        n: usize,
    ) -> Result<Vec<i64>> {
        let mut artists: Vec<i64> = Vec::new();
        for id in seeds {
            if let Some(artist) = self
                .conn
                .prepare_cached("SELECT artist_id FROM tracks WHERE id = ?1")?
                .query_row([id], |r| r.get::<_, i64>(0))
                .optional()?
            {
                artists.push(artist);
            }
        }
        for name in seed_artists {
            if let Some(artist) = self
                .conn
                .prepare_cached("SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE")?
                .query_row([name], |r| r.get::<_, i64>(0))
                .optional()?
            {
                artists.push(artist);
            }
        }
        artists.sort_unstable();
        artists.dedup();

        let playable = if files_only {
            "t.origin = 'file' AND t.missing_since IS NULL"
        } else {
            "(t.origin = 'described' OR (t.origin = 'file' AND t.missing_since IS NULL))"
        };
        let rested =
            "NOT EXISTS (SELECT 1 FROM plays p WHERE p.track_id = t.id AND p.played_at_ms >= ?1)";
        let candidates = |filter: &str, order: &str| -> Result<Vec<(i64, i64)>> {
            let sql = format!(
                "SELECT t.id, t.artist_id FROM tracks t WHERE {filter} AND {playable} AND {rested} ORDER BY {order} LIMIT 60"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map([rested_since_ms], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        };
        let same_artists = if artists.is_empty() {
            Vec::new()
        } else {
            let list = artists
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            candidates(&format!("t.artist_id IN ({list})"), "random()")?
        };
        let liked = candidates("t.liked_at_ms IS NOT NULL", "random()")?;
        let most_played = candidates("t.play_count > 0", "t.play_count DESC, random()")?;
        let any = candidates("1", "random()")?;

        let mut picked: Vec<(i64, i64)> = Vec::new();
        let mut seen: HashSet<i64> = queued.clone();
        let mut take = |from: &[(i64, i64)], up_to: usize, picked: &mut Vec<(i64, i64)>| {
            for &(id, artist) in from {
                if picked.len() >= up_to {
                    break;
                }
                if seen.insert(id) {
                    picked.push((id, artist));
                }
            }
        };
        // About half by the artists just heard, the rest from what the listener likes and plays most.
        take(&same_artists, n.div_ceil(2), &mut picked);
        take(&liked, n, &mut picked);
        take(&most_played, n, &mut picked);
        take(&same_artists, n, &mut picked);
        take(&any, n, &mut picked);

        // The same artist not twice in a row, when another can come between.
        let mut ordered: Vec<i64> = Vec::with_capacity(picked.len());
        let mut last: Option<i64> = None;
        while !picked.is_empty() {
            let at = picked
                .iter()
                .position(|(_, a)| Some(*a) != last)
                .unwrap_or(0);
            let (id, artist) = picked.remove(at);
            ordered.push(id);
            last = Some(artist);
        }
        Ok(ordered)
    }

    // ---- Downloads

    /// Puts songs on the list to be saved as files: songs of the library that are not files already. A song that
    /// failed before is tried again; one queued, saving or saved stays as it is. Answers how many were put on it.
    pub fn ask_downloads(&mut self, ids: &[i64], now_ms: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut queued = 0;
        {
            let mut ask = tx.prepare(
                "INSERT INTO downloads(track_id, state, asked_at)
                 SELECT id, 'queued', ?2 FROM tracks WHERE id = ?1 AND origin <> 'file'
                 ON CONFLICT(track_id) DO UPDATE SET state = 'queued', error = NULL, asked_at = excluded.asked_at
                 WHERE downloads.state = 'failed'",
            )?;
            for id in ids {
                queued += ask.execute(params![id, now_ms])?;
            }
        }
        tx.commit()?;
        Ok(queued)
    }

    /// The next song to save, now marked as saving; the first asked for first.
    pub fn next_download(&mut self) -> Result<Option<i64>> {
        let next: Option<i64> = self
            .conn
            .query_row(
                "SELECT track_id FROM downloads WHERE state = 'queued' ORDER BY asked_at, track_id LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = next {
            self.conn.execute(
                "UPDATE downloads SET state = 'saving' WHERE track_id = ?1",
                [id],
            )?;
        }
        Ok(next)
    }

    /// Songs that were coming in when the program last ended are queued again (what came in is thrown away).
    pub fn requeue_downloads(&mut self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE downloads SET state = 'queued' WHERE state = 'saving'",
            [],
        )?)
    }

    pub fn download_failed(&mut self, id: i64, why: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET state = 'failed', error = ?2 WHERE track_id = ?1 AND state = 'saving'",
            params![id, clean_text(why, 300)],
        )?;
        Ok(())
    }

    /// The song `id` is a file now: its row becomes the file's (its likes, playlists, plays and ISRC stay), its
    /// download is saved. With `cover_inside` the file's own picture is shown before the add-on's cover address, so
    /// it shows with no network (the address is kept for when the file is given back). False (and nothing changed) when the song was taken off the list meanwhile.
    pub fn download_saved(
        &mut self,
        id: i64,
        file: &crate::library::NewTrack,
        saved: &SavedDownload,
    ) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let still: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM downloads WHERE track_id = ?1 AND state = 'saving'",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)?;
        if !still {
            return Ok(false);
        }
        tx.execute(
            "UPDATE tracks SET origin = 'file', addon_id = NULL, locator = ?2, codec = ?3, sample_rate = ?4,
                               bit_depth = ?5, file_size = ?6, file_mtime = ?7, missing_since = NULL,
                               cover = CASE WHEN ?9 THEN 1 END,
                               duration_ms = CASE WHEN duration_ms > 0 THEN duration_ms ELSE ?8 END
             WHERE id = ?1",
            params![
                id,
                file.locator,
                file.codec,
                file.sample_rate,
                file.bit_depth,
                file.file_size.map(|s| s as i64),
                file.file_mtime,
                file.duration_ms as i64,
                saved.cover_inside,
            ],
        )?;
        tx.execute(
            "UPDATE downloads SET state = 'saved', path = ?2, addon = ?3, tier = ?4, bytes = ?5, error = NULL,
                                  saved_at = ?6
             WHERE track_id = ?1",
            params![
                id,
                file.locator,
                saved.addon,
                saved.tier,
                file.file_size.map(|s| s as i64),
                saved.at_ms
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Takes songs off the list. A saved one goes back to being a song the add-ons play, and its file is answered
    /// so it can be deleted; one queued, saving or failed is just taken off.
    pub fn forget_downloads(&mut self, ids: &[i64]) -> Result<Vec<String>> {
        let tx = self.conn.transaction()?;
        let mut files = Vec::new();
        {
            let mut read = tx.prepare("SELECT state, path FROM downloads WHERE track_id = ?1")?;
            let mut back = tx.prepare(
                "UPDATE tracks SET origin = 'described', locator = '', codec = NULL, sample_rate = NULL,
                                   bit_depth = NULL, file_size = NULL, file_mtime = NULL, missing_since = NULL,
                                   cover = NULL
                 WHERE id = ?1 AND origin = 'file' AND locator = ?2",
            )?;
            let mut drop = tx.prepare("DELETE FROM downloads WHERE track_id = ?1")?;
            for id in ids {
                let found: Option<(String, Option<String>)> = read
                    .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                let Some((state, path)) = found else { continue };
                if state == "saved"
                    && let Some(path) = path
                {
                    back.execute(params![id, path])?;
                    files.push(path);
                }
                drop.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(files)
    }

    /// The cover address another Stash or an add-on gave for the song `id`.
    pub fn art_url_of(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT art_url FROM tracks WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .flatten())
    }

    /// The song's place on its album, when the library knows it.
    pub fn track_number(&self, id: i64) -> Result<Option<u32>> {
        Ok(self
            .conn
            .prepare_cached("SELECT track_no FROM tracks WHERE id = ?1 AND track_no > 0")?
            .query_row([id], |r| r.get(0))
            .optional()?)
    }

    /// Whether the song `id` is still to be saved (the one coming in checks this, to stop when taken off).
    pub fn download_wanted(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT COUNT(*) FROM downloads WHERE track_id = ?1 AND state = 'saving'",
            )?
            .query_row([id], |r| r.get::<_, i64>(0))?
            > 0)
    }

    /// Failed songs back on the list (all of them when `ids` is empty).
    pub fn retry_downloads(&mut self, ids: &[i64], now_ms: i64) -> Result<usize> {
        if ids.is_empty() {
            return Ok(self.conn.execute(
                "UPDATE downloads SET state = 'queued', error = NULL, asked_at = ?1 WHERE state = 'failed'",
                [now_ms],
            )?);
        }
        let mut n = 0;
        for id in ids {
            n += self.conn.execute(
                "UPDATE downloads SET state = 'queued', error = NULL, asked_at = ?2 WHERE track_id = ?1 AND state = 'failed'",
                params![id, now_ms],
            )?;
        }
        Ok(n)
    }

    /// How many songs are queued, saving, saved and failed.
    pub fn download_counts(&self) -> Result<DownloadCounts> {
        let mut counts = DownloadCounts::default();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT state, COUNT(*) FROM downloads GROUP BY state")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        for row in rows {
            let (state, n) = row?;
            match state.as_str() {
                "queued" => counts.queued = n,
                "saving" => counts.saving = n,
                "saved" => counts.saved = n,
                _ => counts.failed = n,
            }
        }
        Ok(counts)
    }

    /// A page of the list: the song coming in first, then those waiting (first asked first), those that failed,
    /// and the saved ones (the newest first).
    pub fn downloads_at(&self, offset: usize, limit: usize) -> Result<Vec<DownloadRow>> {
        let sql = format!(
            "SELECT {}, d.state, d.error, d.bytes, d.tier, d.addon, d.saved_at
             FROM (SELECT * FROM downloads
                   ORDER BY CASE state WHEN 'saving' THEN 0 WHEN 'queued' THEN 1 WHEN 'failed' THEN 2 ELSE 3 END,
                            CASE WHEN state = 'saved' THEN -saved_at ELSE asked_at END, track_id
                   LIMIT ?1 OFFSET ?2) d
             JOIN tracks t ON t.id = d.track_id {}
             ORDER BY CASE d.state WHEN 'saving' THEN 0 WHEN 'queued' THEN 1 WHEN 'failed' THEN 2 ELSE 3 END,
                      CASE WHEN d.state = 'saved' THEN -d.saved_at ELSE d.asked_at END, d.track_id",
            crate::library::TRACK_COLUMNS_FOR_LISTS,
            crate::library::TRACK_JOINS_FOR_LISTS,
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(DownloadRow {
                    track: crate::library::track_row_for_lists(r)?,
                    state: r.get(17)?,
                    error: r.get(18)?,
                    bytes: r.get::<_, Option<i64>>(19)?.map(|b| b as u64),
                    tier: r.get(20)?,
                    addon: r.get(21)?,
                    saved_at_ms: r.get(22)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

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

/// Lyrics kept from the network for a song: what was found (`None`: it had none), from where, and when (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptLyrics {
    pub lyrics: Option<crate::lyrics::Lyrics>,
    pub via: Option<String>,
    pub at: i64,
}

/// What a finished download came from.
#[derive(Debug, Clone)]
pub struct SavedDownload {
    pub addon: String,
    pub tier: Option<String>,
    pub at_ms: i64,
    /// The file carries its cover now.
    pub cover_inside: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DownloadCounts {
    pub queued: u64,
    pub saving: u64,
    pub saved: u64,
    pub failed: u64,
}

/// A song on the downloads list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DownloadRow {
    #[serde(flatten)]
    pub track: crate::library::TrackRow,
    /// "queued", "saving", "saved" or "failed".
    pub state: String,
    pub error: Option<String>,
    pub bytes: Option<u64>,
    pub tier: Option<String>,
    pub addon: Option<String>,
    pub saved_at_ms: Option<i64>,
}
