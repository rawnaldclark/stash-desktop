//! What the listener does to their own library: liking, making and editing playlists, and the plays that counted.
//! And the way back out: the library written as the file the Android app and the web player read
//! (`stash-web-library`, version 1), so a like made here reaches the phone the way a like made there reached here.

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};
use serde_json::{Map, Value, json};

use crate::identity::clean_text;
use crate::import::Song;
use crate::library::{Library, here};
use crate::{Error, Result};

/// Where a query's own columns start, after a track's.
const AFTER: usize = crate::library::TRACK_COLUMN_COUNT;

/// The setting that keeps the liked songs on this computer ("1"), as a playlist's `download` keeps it.
pub const DOWNLOAD_LIKED: &str = "download.liked";

/// What a radio's songs must leave room for (`Library::songs_by_artist`): songs already queued or had (`not`), songs
/// heard since `rested_since_ms` (Unix milliseconds), and with `here_only` (offline), songs not on this computer.
#[derive(Debug, Clone, Default)]
pub struct Room {
    pub not: HashSet<i64>,
    pub rested_since_ms: i64,
    pub here_only: bool,
}

/// How far a list kept on this computer is: its songs, how many play with no connection, how many wait to be saved
/// (or are coming in), and how many could not be saved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DownloadProgress {
    pub songs: u64,
    pub here: u64,
    pub waiting: u64,
    pub failed: u64,
    /// Songs the listener took off the downloads: a kept list leaves them be.
    pub declined: u64,
}

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

    // ---- Similar music (Last.fm)

    /// What Last.fm said to the question `key` (`similar::Ask::key`), and when (Unix seconds), if it was asked.
    pub fn kept_similar(&self, key: &str) -> Result<Option<(crate::similar::Answer, i64)>> {
        use crate::similar::{Answer, Like};
        Ok(self
            .conn
            .prepare_cached("SELECT body, at FROM similar_cache WHERE key = ?1")?
            .query_row([key], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))
            })
            .optional()?
            .and_then(|(body, at)| {
                let answer = match body {
                    None => Answer::NotFound,
                    // A body this program cannot read is as if nothing was kept: it is asked again.
                    Some(body) => Answer::Found(serde_json::from_str::<Vec<Like>>(&body).ok()?),
                };
                Some((answer, at))
            }))
    }

    /// Keeps what Last.fm said to the question `key`, at `at_s` (Unix seconds).
    pub fn keep_similar(
        &mut self,
        key: &str,
        answer: &crate::similar::Answer,
        at_s: i64,
    ) -> Result<()> {
        let body = match answer {
            crate::similar::Answer::Found(likes) => {
                Some(serde_json::to_string(likes).unwrap_or_default())
            }
            crate::similar::Answer::NotFound => None,
        };
        self.conn.execute(
            "INSERT INTO similar_cache(key, body, at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET body = excluded.body, at = excluded.at",
            params![key, body, at_s],
        )?;
        Ok(())
    }

    /// Lets go of everything Last.fm said (similar music switched off). Answers how much went.
    pub fn forget_similar(&mut self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM similar_cache", [])?)
    }

    /// Lets go of answers kept since before `older_than_s` (Unix seconds), and of the oldest past `cap_rows`. Answers
    /// how many went.
    pub fn prune_similar(&mut self, older_than_s: i64, cap_rows: usize) -> Result<usize> {
        let old = self
            .conn
            .execute("DELETE FROM similar_cache WHERE at < ?1", [older_than_s])?;
        let over = self.conn.execute(
            "DELETE FROM similar_cache WHERE key IN
               (SELECT key FROM similar_cache ORDER BY at DESC, key LIMIT -1 OFFSET ?1)",
            [cap_rows as i64],
        )?;
        Ok(old + over)
    }

    /// Up to `n` of the library's songs by the artist `name` (`key`: as songs are grouped, `shuffle::artist_key`, the
    /// first artist a credit names, folded), for a radio: those whose titles are among `titles` (what Last.fm named)
    /// first, in that order, then liked songs, then the most played, then any; one copy of each song ("Angel" and
    /// "Angel - 2006 Remaster" are one). Only songs that can play (with `room.here_only`, only those on this computer),
    /// none chosen not to be played, none heard since `room.rested_since_ms`, none of `room.not`.
    ///
    /// The artist is the library's artist of that very name when there is one ("The xx" and "the xx" are one), with
    /// the credits that surely name it first ("A feat. B"); only when there is none, any credit that names it first
    /// ("A & B"). So "Earth, Wind & Fire" is not "Earth"'s, though both are counted under "earth".
    pub fn songs_by_artist(
        &self,
        name: &str,
        key: &str,
        titles: &[String],
        room: &Room,
        n: usize,
    ) -> Result<Vec<i64>> {
        if key.is_empty() || n == 0 {
            return Ok(Vec::new());
        }
        // The artist by its whole name, and as a credit surely naming it first gives it ("A feat. B" is A).
        let fold = crate::identity::fold;
        let named = [fold(name), fold(crate::credits::surely_first(name))];
        let (mut exact, mut first): (Vec<i64>, Vec<i64>) = (Vec::new(), Vec::new());
        {
            let mut stmt = self.conn.prepare_cached("SELECT id, name FROM artists")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, artist) = row?;
                let own = [fold(&artist), fold(crate::credits::surely_first(&artist))];
                if own.iter().any(|n| named.contains(n)) {
                    exact.push(id);
                } else if crate::shuffle::artist_key(&artist) == key {
                    first.push(id);
                }
            }
        }
        let artists = if exact.is_empty() { first } else { exact };
        if artists.is_empty() {
            return Ok(Vec::new());
        }
        let playable = if room.here_only {
            here!()
        } else {
            "(t.origin = 'file' AND t.missing_since IS NULL
              OR t.origin = 'described' AND NOT EXISTS (SELECT 1 FROM unfound u WHERE u.track_id = t.id))"
        };
        let list = artists
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT t.id, t.title, t.liked_at_ms IS NOT NULL, t.play_count FROM tracks t
             WHERE t.artist_id IN ({list}) AND {playable} AND NOT {blocked}
               AND NOT EXISTS (SELECT 1 FROM plays p WHERE p.track_id = t.id AND p.played_at_ms >= ?1)
             ORDER BY random()",
            blocked = crate::blocks::blocked!()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map([room.rested_since_ms], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut songs: Vec<_> = rows
            .into_iter()
            .enumerate()
            .filter(|(_, (id, ..))| !room.not.contains(id))
            .map(|(by_chance, (id, title, liked, plays))| {
                let wanted = titles
                    .iter()
                    .position(|t| crate::matching::same_title(t, &title))
                    .unwrap_or(usize::MAX);
                (
                    (wanted, !liked, -plays, by_chance),
                    id,
                    crate::matching::title_key(&title),
                )
            })
            .collect();
        songs.sort_unstable_by_key(|s| s.0);
        // One copy of a song: the first of those with the same title, release labels aside.
        let mut seen_titles: HashSet<String> = HashSet::new();
        Ok(songs
            .into_iter()
            .filter(|(_, _, title)| seen_titles.insert(title.clone()))
            .map(|(_, id, _)| id)
            .take(n)
            .collect())
    }

    // ---- Autoplay

    /// Songs to play after `seeds` (the last songs heard; `seed_artists` names their artists, for songs that are not
    /// the library's): by those artists first, then liked songs, then the most played, then any. None of `queued`,
    /// none heard since `rested_since_ms`, only songs on this computer when `files_only` (files, and songs kept as they
    /// were played). At most `n`, the same artist never twice in a row when that can be helped (nor right after the
    /// last song heard, `seed_artists[0]`).
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

        // Songs no add-on could find are left out: they would only be passed over. Offline, only songs on this
        // computer: files, and songs kept as they were played.
        let playable = if files_only {
            here!()
        } else {
            "(t.origin = 'file' AND t.missing_since IS NULL
              OR t.origin = 'described' AND NOT EXISTS (SELECT 1 FROM unfound u WHERE u.track_id = t.id))"
        };
        let rested =
            "NOT EXISTS (SELECT 1 FROM plays p WHERE p.track_id = t.id AND p.played_at_ms >= ?1)";
        let candidates = |filter: &str, order: &str| -> Result<Vec<(i64, i64)>> {
            let sql = format!(
                "SELECT t.id, t.artist_id FROM tracks t WHERE {filter} AND {playable} AND {rested} AND NOT {blocked}
                 ORDER BY {order} LIMIT 60",
                blocked = crate::blocks::blocked!()
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

        // The same artist not twice in a row, when another can come between, starting from the song heard last.
        let last: Option<i64> = match seed_artists.first() {
            Some(name) => self
                .conn
                .prepare_cached("SELECT id FROM artists WHERE name = ?1 COLLATE NOCASE")?
                .query_row([name], |r| r.get::<_, i64>(0))
                .optional()?,
            None => None,
        };
        Ok(crate::shuffle::apart(picked, last))
    }

    // ---- Downloads

    /// Puts songs on the list to be saved as files: songs of the library that are not files already. A song that
    /// failed before is tried again; one queued, saving or saved stays as it is. Asked for by hand, a song stays on
    /// the list whatever the lists kept on this computer hold, and one the listener took off before is theirs to save
    /// again. Answers how many were put on it.
    pub fn ask_downloads(&mut self, ids: &[i64], now_ms: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut queued = 0;
        {
            let mut ask = tx.prepare(
                "INSERT INTO downloads(track_id, state, asked_at)
                 SELECT id, 'queued', ?2 FROM tracks WHERE id = ?1 AND origin <> 'file'
                 ON CONFLICT(track_id) DO UPDATE SET state = 'queued', error = NULL, asked_at = excluded.asked_at,
                                                     strikes = 0, asks = 0, unsure_since = NULL, tried_at = NULL
                 WHERE downloads.state = 'failed'",
            )?;
            let mut claim = tx.prepare("UPDATE downloads SET by_list = 0 WHERE track_id = ?1")?;
            let mut welcome = tx.prepare("DELETE FROM download_declined WHERE track_id = ?1")?;
            for id in ids {
                queued += ask.execute(params![id, now_ms])?;
                claim.execute([id])?;
                welcome.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(queued)
    }

    /// Brings the downloads into line with the lists kept on this computer (the liked songs when they are, each
    /// playlist that is; a smart playlist's songs as its rules pick them now). Their songs that are not files and have
    /// never been on the downloads are put on it; songs a list put there that still wait, but are on no kept list any
    /// more, are taken off. A song that could not be saved, or that the listener took off, is left alone, so a kept
    /// list does not spend an add-on's allowance on it again and again (the listener can ask for it again). A song
    /// asked for by hand stays whatever the lists hold. Answers how many were put on and how many taken off.
    pub fn reconcile_kept(&mut self, now_ms: i64) -> Result<(usize, usize)> {
        let smart: Vec<i64> = self
            .conn
            .prepare_cached("SELECT id FROM playlists WHERE download = 1 AND rules IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut picked = Vec::new();
        for list in smart {
            picked.extend(self.playlist_track_ids(list, 0, usize::MAX >> 1)?);
        }
        let liked = self.setting(DOWNLOAD_LIKED)?.as_deref() == Some("1");
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS kept_now(id INTEGER PRIMARY KEY);
             DELETE FROM temp.kept_now;",
        )?;
        if liked {
            tx.execute(
                "INSERT OR IGNORE INTO temp.kept_now(id) SELECT id FROM tracks WHERE liked_at_ms IS NOT NULL",
                [],
            )?;
        }
        tx.execute(
            "INSERT OR IGNORE INTO temp.kept_now(id)
             SELECT i.track_id FROM playlist_items i JOIN playlists p ON p.id = i.playlist_id
             WHERE p.download = 1 AND p.rules IS NULL",
            [],
        )?;
        {
            let mut put = tx.prepare("INSERT OR IGNORE INTO temp.kept_now(id) VALUES (?1)")?;
            for id in picked {
                put.execute([id])?;
            }
        }
        let added = tx.execute(
            "INSERT INTO downloads(track_id, state, asked_at, by_list)
             SELECT k.id, 'queued', ?1, 1 FROM temp.kept_now k JOIN tracks t ON t.id = k.id
             WHERE t.origin <> 'file'
               AND NOT EXISTS (SELECT 1 FROM downloads d WHERE d.track_id = k.id)
               AND NOT EXISTS (SELECT 1 FROM download_declined x WHERE x.track_id = k.id)",
            [now_ms],
        )?;
        let dropped = tx.execute(
            "DELETE FROM downloads WHERE state = 'queued' AND by_list = 1
               AND track_id NOT IN (SELECT id FROM temp.kept_now)",
            [],
        )?;
        tx.execute("DELETE FROM temp.kept_now", [])?;
        tx.commit()?;
        Ok((added, dropped))
    }

    /// Whether a smart playlist is kept on this computer. Its songs change as songs are played, with no edit to say
    /// so, and the kept lists are looked at now and then only for it.
    pub fn kept_smart_lists(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM playlists WHERE download = 1 AND rules IS NOT NULL)",
            [],
            |r| r.get(0),
        )?)
    }

    /// Keeps playlist `id` on this computer, or stops. False when there is no such playlist.
    pub fn set_playlist_download(&mut self, id: i64, on: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE playlists SET download = ?2 WHERE id = ?1",
            params![id, on],
        )? > 0)
    }

    /// Whether playlist `id` is kept on this computer; `None` when there is no such playlist.
    pub fn playlist_download(&self, id: i64) -> Result<Option<bool>> {
        Ok(self
            .conn
            .query_row("SELECT download FROM playlists WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// How far these songs are on this computer: how many play with no connection, how many wait to be saved (or are
    /// coming in), how many could not be saved, and how many the listener took off. One query, however long the list.
    pub fn download_progress(&self, ids: &[i64]) -> Result<DownloadProgress> {
        let list = serde_json::to_string(ids).unwrap_or_else(|_| "[]".into());
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT COALESCE(SUM(",
            here!(),
            "), 0),
                    COALESCE(SUM(d.state IN ('queued', 'saving')), 0),
                    COALESCE(SUM(d.state = 'failed'), 0),
                    COALESCE(SUM(x.track_id IS NOT NULL), 0)
             FROM json_each(?1) j JOIN tracks t ON t.id = j.value
             LEFT JOIN downloads d ON d.track_id = t.id
             LEFT JOIN download_declined x ON x.track_id = t.id"
        ))?;
        let (here, waiting, failed, declined): (i64, i64, i64, i64) =
            stmt.query_row([list], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(DownloadProgress {
            songs: ids.len() as u64,
            here: here as u64,
            waiting: waiting as u64,
            failed: failed as u64,
            declined: declined as u64,
        })
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

    /// The song coming in waits its turn again, at the back of the line (not its own trouble, it seems: the add-on's
    /// allowance is spent, it could not be reached, the downloads folder could not be written).
    pub fn download_held(&mut self, id: i64, now_ms: i64) -> Result<()> {
        // To the back of the line: the songs after it are tried first (the trouble may be its own after all).
        self.conn.execute(
            "UPDATE downloads SET state = 'queued', asked_at = ?2 WHERE track_id = ?1 AND state = 'saving'",
            params![id, now_ms],
        )?;
        Ok(())
    }

    /// Trouble that may be its own, for the song coming in (`id`). A try that asked something (`asked`) is noted, and
    /// counted against the song (a strike) when an add-on gave another song since its try before: the add-on gives
    /// other songs, so the trouble is likely this one's. A song saved from a copy kept from playing it does not count:
    /// no add-on was asked for it. Answers what the song has had so far; it is kept across starts, and
    /// a song saved or asked for again starts over.
    pub fn download_unsure(&mut self, id: i64, asked: bool, now_ms: i64) -> Result<UnsureTries> {
        let tx = self.conn.transaction()?;
        let strike: bool = asked
            && tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM downloads d JOIN downloads o
                                  ON o.track_id <> d.track_id AND o.state = 'saved' AND o.tried_at > d.tried_at
                                 WHERE d.track_id = ?1)",
                [id],
                |r| r.get(0),
            )?;
        tx.execute(
            "UPDATE downloads SET strikes = strikes + ?2, asks = asks + ?3, unsure_since = COALESCE(unsure_since, ?4),
                                  tried_at = CASE WHEN ?3 = 1 THEN ?4 ELSE tried_at END
             WHERE track_id = ?1",
            params![id, i64::from(strike), i64::from(asked), now_ms],
        )?;
        let tries = tx
            .query_row(
                "SELECT strikes, asks, COALESCE(unsure_since, ?2) FROM downloads WHERE track_id = ?1",
                params![id, now_ms],
                |r| {
                    Ok(UnsureTries {
                        strikes: r.get(0)?,
                        asks: r.get(1)?,
                        since_ms: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default();
        tx.commit()?;
        Ok(tries)
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
                                  saved_at = ?6, strikes = 0, asks = 0, unsure_since = NULL,
                                  tried_at = CASE WHEN ?7 THEN ?6 END
             WHERE track_id = ?1",
            params![
                id,
                file.locator,
                saved.addon,
                saved.tier,
                file.file_size.map(|s| s as i64),
                saved.at_ms,
                saved.fetched
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Takes songs off the list. A saved one goes back to being a song the add-ons play, and its file is answered
    /// so it can be deleted; one queued, saving or failed is just taken off. Either way the song is remembered as one
    /// the listener took off: a list kept on this computer does not put it back (asked for by hand, it comes back).
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
            let mut decline =
                tx.prepare("INSERT OR IGNORE INTO download_declined(track_id) VALUES (?1)")?;
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
                decline.execute([id])?;
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

    /// Failed songs back on the list (all of them when `ids` is empty). Asked for by hand: a song a kept list put
    /// there is the listener's own now. Every song named (all on the list when `ids` is empty) starts its tries over.
    pub fn retry_downloads(&mut self, ids: &[i64], now_ms: i64) -> Result<usize> {
        const AFRESH: &str = "strikes = 0, asks = 0, unsure_since = NULL, tried_at = NULL";
        if ids.is_empty() {
            // Asked for again by hand: no list's to take back (a list let go since would otherwise drop them).
            let n = self.conn.execute(
                "UPDATE downloads SET state = 'queued', error = NULL, asked_at = ?1, by_list = 0 WHERE state = 'failed'",
                [now_ms],
            )?;
            self.conn.execute(
                &format!("UPDATE downloads SET {AFRESH} WHERE state IN ('queued', 'saving')"),
                [],
            )?;
            return Ok(n);
        }
        let mut n = 0;
        for id in ids {
            n += self.conn.execute(
                "UPDATE downloads SET state = 'queued', error = NULL, asked_at = ?2, by_list = 0
                 WHERE track_id = ?1 AND state = 'failed'",
                params![id, now_ms],
            )?;
            self.conn.execute(
                &format!("UPDATE downloads SET {AFRESH} WHERE track_id = ?1 AND state IN ('queued', 'saving')"),
                [id],
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
                    state: r.get(AFTER)?,
                    error: r.get(AFTER + 1)?,
                    bytes: r.get::<_, Option<i64>>(AFTER + 2)?.map(|b| b as u64),
                    tier: r.get(AFTER + 3)?,
                    addon: r.get(AFTER + 4)?,
                    saved_at_ms: r.get(AFTER + 5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ---- Not found: songs every add-on that could be asked said it does not have. The list fills only from songs the
    // listener played or asked to save (no add-on is ever asked about the library in bulk), and empties as songs are
    // found: by the listener picking one of an add-on's results, or by an add-on finding it later.

    /// Notes that no add-on has the song `id`: a song described here, or a file that is gone (an unplugged drive's
    /// songs are not asked of the add-ons). Noted again, it keeps its place and stays set aside if it was. False when
    /// `id` is no such song.
    pub fn note_unfound(&mut self, id: i64, now_ms: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT INTO unfound(track_id, first_at, last_at)
             SELECT id, ?2, ?2 FROM tracks
             WHERE id = ?1 AND (origin = 'described' OR (origin = 'file' AND missing_since IS NOT NULL))
             ON CONFLICT(track_id) DO UPDATE SET last_at = excluded.last_at",
            params![id, now_ms],
        )? > 0)
    }

    /// The song was found after all: it leaves the list, set aside or not.
    pub fn found_again(&mut self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM unfound WHERE track_id = ?1", [id])?
            > 0)
    }

    /// The listener picked the add-on's track `theirs` as the song `id`: it is kept as that add-on's name for the
    /// song (so it plays from there from now on), the song leaves the list, and, with `save` (the add-on lets its
    /// songs be saved), a save of it that failed is asked again. `None` when there is no such song; `Some(true)` when
    /// a save was asked again.
    pub fn fix_unfound(
        &mut self,
        id: i64,
        addon: &str,
        theirs: &str,
        save: bool,
        now_ms: i64,
    ) -> Result<Option<bool>> {
        let tx = self.conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracks WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        tx.execute(
            "INSERT INTO track_refs(track_id, source, ref) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id, source) DO UPDATE SET ref = excluded.ref",
            params![id, addon, theirs],
        )?;
        tx.execute("DELETE FROM unfound WHERE track_id = ?1", [id])?;
        let saving = save
            && tx.execute(
                // Asked for again: its trouble starts over, as any song asked for again (`ask_downloads`).
                "UPDATE downloads SET state = 'queued', error = NULL, asked_at = ?2,
                                  strikes = 0, asks = 0, unsure_since = NULL, tried_at = NULL
             WHERE track_id = ?1 AND state = 'failed'",
                params![id, now_ms],
            )? > 0;
        tx.commit()?;
        Ok(Some(saving))
    }

    /// Sets songs aside (none of an add-on's results is the song): off the list, still known not to play. With
    /// `aside` false they are back on it. Answers how many changed.
    pub fn set_unfound_aside(&mut self, ids: &[i64], aside: bool, now_ms: i64) -> Result<usize> {
        // In one go: one write to the disk, however many songs.
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            let mut set = tx.prepare("UPDATE unfound SET aside_at = ?2 WHERE track_id = ?1")?;
            for id in ids {
                n += set.execute(params![id, aside.then_some(now_ms)])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// The page of songs on the list at `offset` (not set aside, not a file here now), the latest miss first.
    pub fn unfound_at(&self, offset: usize, limit: usize) -> Result<Vec<UnfoundRow>> {
        let sql = format!(
            "SELECT {}, u.first_at, u.last_at
             FROM (SELECT u.* FROM unfound u JOIN tracks t ON t.id = u.track_id
                   WHERE u.aside_at IS NULL AND NOT (t.origin = 'file' AND t.missing_since IS NULL)
                   ORDER BY u.last_at DESC, u.track_id LIMIT ?1 OFFSET ?2) u
             JOIN tracks t ON t.id = u.track_id {}
             ORDER BY u.last_at DESC, u.track_id",
            crate::library::TRACK_COLUMNS_FOR_LISTS,
            crate::library::TRACK_JOINS_FOR_LISTS,
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(UnfoundRow {
                    track: crate::library::track_row_for_lists(r)?,
                    first_at_ms: r.get(AFTER)?,
                    last_at_ms: r.get(AFTER + 1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// How many songs are on the list, and every song known not to play (on the list or set aside), for marking them
    /// wherever they are shown.
    pub fn unfound_counts(&self) -> Result<UnfoundCounts> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT u.track_id, u.aside_at IS NULL FROM unfound u JOIN tracks t ON t.id = u.track_id
             WHERE NOT (t.origin = 'file' AND t.missing_since IS NULL) ORDER BY u.track_id",
        )?;
        let mut counts = UnfoundCounts::default();
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)))? {
            let (id, open) = row?;
            counts.marked.push(id);
            if open {
                counts.open += 1;
            }
        }
        Ok(counts)
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

    /// The playlist that follows the list `list` of a source's ("youtube:VL…"), made the first time (named `name`, last
    /// in the list), with `tracks` as its songs, in their order: the list's songs as they now are. Answers its id and
    /// whether its songs changed. A name the listener gave it stays.
    pub fn follow_list(
        &mut self,
        list: &str,
        name: &str,
        tracks: &[i64],
        now_ms: i64,
    ) -> Result<(i64, bool)> {
        let tx = self.conn.transaction()?;
        let found: Option<i64> = tx
            .query_row(
                "SELECT id FROM playlists WHERE source_list = ?1",
                [list],
                |r| r.get(0),
            )
            .optional()?;
        let id = match found {
            Some(id) => id,
            None => {
                let name = named(name)?;
                let next: i64 =
                    tx.query_row("SELECT IFNULL(MAX(id), 0) + 1 FROM playlists", [], |r| {
                        r.get(0)
                    })?;
                tx.execute(
                    "INSERT INTO playlists(id, external_id, name, created_at_ms, updated_at_ms, position, source_list)
                     VALUES (?1, ?2, ?3, ?4, ?4, (SELECT IFNULL(MAX(position), 0) + 1 FROM playlists), ?5)",
                    params![next, format!("desk-{now_ms}-{next}"), name, now_ms, list],
                )?;
                next
            }
        };
        // Each song once, in the list's order; only songs the library has.
        let mut seen = HashSet::new();
        let mut wanted = Vec::new();
        {
            let mut known = tx.prepare("SELECT 1 FROM tracks WHERE id = ?1")?;
            for track in tracks {
                if seen.insert(*track) && known.exists([track])? {
                    wanted.push(*track);
                }
            }
        }
        let had: Vec<i64> = tx
            .prepare(
                "SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY position",
            )?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let changed = had != wanted;
        if changed {
            tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
            let mut add = tx.prepare(
                "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)",
            )?;
            for (position, track) in wanted.iter().enumerate() {
                add.execute(params![id, position as i64, track])?;
            }
            drop(add);
            tx.execute(
                "UPDATE playlists SET updated_at_ms = ?2 WHERE id = ?1",
                params![id, now_ms],
            )?;
        }
        tx.execute(
            "UPDATE playlists SET source_checked_ms = ?2 WHERE id = ?1",
            params![id, now_ms],
        )?;
        tx.commit()?;
        Ok((id, changed))
    }

    /// The lists of sources' that playlists follow: (playlist, list, when it was last read).
    pub fn followed_lists(&self) -> Result<Vec<(i64, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_list, IFNULL(source_checked_ms, 0) FROM playlists
             WHERE source_list IS NOT NULL ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The list of a source's the playlist `id` follows, when it follows one: its songs are that list's.
    pub fn follows(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT source_list FROM playlists WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
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
            // A smart playlist's songs are its rules': none are added by hand.
            let exists: Option<i64> = tx
                .query_row(
                    "SELECT id FROM playlists WHERE id = ?1 AND rules IS NULL",
                    [id],
                    |r| r.get(0),
                )
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
        let smart: bool = tx
            .query_row(
                "SELECT rules IS NOT NULL FROM playlists WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if smart {
            return Ok(false);
        }
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

    /// Takes several songs out of a playlist at once, each named by its place and the song there. Only while every place
    /// still holds that song (the playlist may have changed since they were chosen): otherwise nothing is taken out,
    /// and the answer is false.
    pub fn remove_many_from_playlist(
        &mut self,
        id: i64,
        places: &[(usize, i64)],
        now_ms: i64,
    ) -> Result<bool> {
        self.reorder(id, now_ms, |items| {
            if places.is_empty()
                || places
                    .iter()
                    .any(|&(place, track)| items.get(place) != Some(&track))
            {
                return false;
            }
            let out: std::collections::HashSet<usize> =
                places.iter().map(|&(place, _)| place).collect();
            let mut place = 0;
            items.retain(|_| {
                let keep = !out.contains(&place);
                place += 1;
                keep
            });
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

    /// Of the songs `ids`, the ones that can leave the library, and why the others cannot: a song in a playlist stays
    /// (taking it out would change the playlist), and so does a file in the listener's music folders (a scan would
    /// bring it back). A song saved to this computer can leave (its file goes with it).
    pub fn removable(&self, ids: &[i64]) -> Result<Removable> {
        let mut out = Removable::default();
        let mut read = self.conn.prepare_cached(
            "SELECT t.origin,
                    EXISTS (SELECT 1 FROM playlist_items i WHERE i.track_id = t.id),
                    EXISTS (SELECT 1 FROM downloads d WHERE d.track_id = t.id)
             FROM tracks t WHERE t.id = ?1",
        )?;
        for &id in ids {
            let Some((origin, listed, saved)) = read
                .query_row([id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, bool>(2)?,
                    ))
                })
                .optional()?
            else {
                continue;
            };
            if listed {
                out.in_playlists += 1;
            } else if origin == "file" && !saved {
                out.in_folders += 1;
            } else {
                out.ids.push(id);
            }
        }
        Ok(out)
    }

    /// Takes songs out of the library for good: the songs `ids` that [`Library::removable`] allows (call
    /// [`Library::forget_downloads`] first for a saved one, so it is no longer a file). What was played of each stays
    /// in the history as heard (by the source that knows it), its like goes with it, and an album or artist left with
    /// no song goes too. Answers how many left.
    pub fn remove_songs(&mut self, ids: &[i64]) -> Result<usize> {
        let ids = self.removable(ids)?.ids;
        let tx = self.conn.transaction()?;
        let mut removed = 0;
        {
            let mut song = tx.prepare(
                "SELECT t.title, a.name, al.title, t.duration_ms, t.art_url, t.origin,
                        (SELECT r.source || char(10) || r.ref FROM track_refs r WHERE r.track_id = t.id
                         ORDER BY r.source = 'library', r.source LIMIT 1)
                 FROM tracks t JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id
                 WHERE t.id = ?1",
            )?;
            let mut plays = tx.prepare("SELECT played_at_ms FROM plays WHERE track_id = ?1")?;
            let mut heard = tx.prepare(
                "INSERT OR IGNORE INTO heard(source, ref, title, artist, album, duration_ms, art_url, played_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for &id in &ids {
                type Song = (
                    String,
                    String,
                    Option<String>,
                    Option<i64>,
                    Option<String>,
                    String,
                    Option<String>,
                );
                let Some((title, artist, album, duration_ms, art, origin, from)): Option<Song> =
                    song.query_row([id], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    })
                    .optional()?
                else {
                    continue;
                };
                // Still a file (it was not taken off the list of saved songs first): it stays.
                if origin == "file" {
                    continue;
                }
                if let Some((source, theirs)) = from.as_deref().and_then(|f| f.split_once('\n')) {
                    let times = plays
                        .query_map([id], |r| r.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for at in times {
                        heard.execute(params![
                            source,
                            theirs,
                            title,
                            artist,
                            album,
                            duration_ms,
                            art,
                            at
                        ])?;
                    }
                }
                for sql in [
                    "DELETE FROM plays WHERE track_id = ?1",
                    "DELETE FROM download_declined WHERE track_id = ?1",
                    "DELETE FROM tracks_fts WHERE rowid = ?1",
                    "DELETE FROM tracks WHERE id = ?1",
                ] {
                    tx.execute(sql, [id])?;
                }
                removed += 1;
            }
        }
        if removed > 0 {
            tx.execute(
                "DELETE FROM albums WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = albums.id)",
                [],
            )?;
            tx.execute(
                "DELETE FROM artists WHERE NOT EXISTS (SELECT 1 FROM tracks t WHERE t.artist_id = artists.id)
                   AND NOT EXISTS (SELECT 1 FROM albums al WHERE al.artist_id = artists.id)",
                [],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// A play that counted of a song the library does not have (`heard.source` played it): kept with how it was shown,
    /// the newest [`HEARD_KEPT`] of them. False when that play is already kept.
    pub fn record_heard(&mut self, heard: &Heard) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let added = tx.execute(
            "INSERT OR IGNORE INTO heard(source, ref, title, artist, album, duration_ms, art_url, played_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                heard.source,
                heard.track,
                clean_text(&heard.title, 500),
                clean_text(&heard.artist, 500),
                heard.album.as_deref().map(|a| clean_text(a, 500)),
                heard.duration_ms.map(|ms| ms as i64),
                heard.art,
                heard.played_at_ms
            ],
        )?;
        if added > 0 {
            tx.execute(
                "DELETE FROM heard WHERE played_at_ms < (
                     SELECT played_at_ms FROM heard ORDER BY played_at_ms DESC LIMIT 1 OFFSET ?1)",
                [HEARD_KEPT as i64 - 1],
            )?;
        }
        tx.commit()?;
        Ok(added > 0)
    }

    /// Plays of songs the library does not have, newest first: `limit` of them from `offset`.
    pub fn heard_at(&self, offset: usize, limit: usize) -> Result<Vec<Heard>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT source, ref, title, artist, album, duration_ms, art_url, played_at_ms FROM heard
             ORDER BY played_at_ms DESC, source, ref LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(Heard {
                    source: r.get(0)?,
                    track: r.get(1)?,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    album: r.get(4)?,
                    duration_ms: r.get::<_, Option<i64>>(5)?.map(|ms| ms.max(0) as u64),
                    art: r.get(6)?,
                    played_at_ms: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
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
            let one = format!("SELECT {SONG} FROM tracks t {JOINS} WHERE t.id = ?1");
            let mut song_of = self.conn.prepare(&one)?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let mut items = Vec::new();
                let id: i64 = row.get(0)?;
                // A smart playlist travels as the songs it holds now, with its rules: other Stashes keep the songs,
                // this one the rules.
                let rules = self.smart_rules(id)?;
                if let Some(rules) = &rules {
                    for track in self.smart_track_ids(rules, 0, MAX_PLAYLIST_ITEMS, now_ms)? {
                        let mut songs = song_of.query([track])?;
                        if let Some(song_row) = songs.next()?
                            && let Some(item) = song(song_row)?
                        {
                            items.push(item);
                        }
                    }
                } else {
                    let mut songs = items_of.query([id])?;
                    while let Some(song_row) = songs.next()? {
                        if let Some(item) = song(song_row)? {
                            items.push(item);
                        }
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
                if let Some(rules) = rules {
                    playlist["rules"] = json!(rules);
                }
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

/// Of some songs, the ones that can leave the library ([`Library::removable`]), and how many cannot.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Removable {
    pub ids: Vec<i64>,
    /// In a playlist: taking them out would change it.
    pub in_playlists: usize,
    /// Files in the listener's music folders.
    pub in_folders: usize,
}

/// How many plays of songs the library does not have are kept.
pub const HEARD_KEPT: usize = 2_000;

/// A play of a song the library does not have: the source that played it and its name for the song, the song as it was
/// shown, and when it began.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Heard {
    pub source: String,
    pub track: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub art: Option<String>,
    pub played_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_leaves_the_library_and_its_plays_stay_heard() {
        let mut library = Library::open_in_memory().unwrap();
        let song = |title: &str, album: &str| Song {
            title: title.into(),
            artist: "Someone".into(),
            album: Some(album.into()),
            duration_ms: Some(200_000),
            refs: vec![("youtube".into(), format!("yt-{title}"))],
            ..Default::default()
        };
        let a = library.adopt(&song("A", "First")).unwrap();
        let b = library.adopt(&song("B", "Second")).unwrap();
        let c = library.adopt(&song("C", "Second")).unwrap();
        library.record_play(a, 1_000).unwrap();
        // C is in a playlist: it stays.
        let list = library.create_playlist("Mine", 5).unwrap();
        library.add_to_playlist(list, &[c], 6).unwrap();
        let can = library.removable(&[a, b, c, 999]).unwrap();
        assert_eq!(
            (can.ids.clone(), can.in_playlists, can.in_folders),
            (vec![a, b], 1, 0)
        );
        assert_eq!(library.remove_songs(&[a, b, c]).unwrap(), 2);
        assert!(library.describe(a).unwrap().is_none());
        assert!(library.describe(c).unwrap().is_some());
        // A's play is in the history still, as heard from YouTube Music.
        let heard = library.heard_at(0, 10).unwrap();
        assert_eq!(heard.len(), 1);
        assert_eq!(
            (
                heard[0].source.as_str(),
                heard[0].track.as_str(),
                heard[0].played_at_ms
            ),
            ("youtube", "yt-A", 1_000)
        );
        // "First" had only A: it is gone; "Second" keeps C.
        let albums: Vec<String> = library
            .conn
            .prepare("SELECT title FROM albums ORDER BY title")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(albums, vec!["Second".to_string()]);
    }

    #[test]
    fn plays_of_songs_the_library_does_not_have_are_kept_newest_first() {
        let mut library = Library::open_in_memory().unwrap();
        let heard = |track: &str, at: i64| Heard {
            source: "youtube".into(),
            track: track.into(),
            title: format!("Song {track}"),
            artist: "Someone".into(),
            album: None,
            duration_ms: Some(200_000),
            art: None,
            played_at_ms: at,
        };
        assert!(library.record_heard(&heard("a", 10)).unwrap());
        assert!(library.record_heard(&heard("b", 20)).unwrap());
        // The same play twice is kept once.
        assert!(!library.record_heard(&heard("a", 10)).unwrap());
        let kept = library.heard_at(0, 10).unwrap();
        assert_eq!(
            kept.iter()
                .map(|h| (h.track.as_str(), h.played_at_ms))
                .collect::<Vec<_>>(),
            vec![("b", 20), ("a", 10)]
        );
        assert_eq!(kept[0].title, "Song b");
        // Only the newest are kept.
        for n in 0..HEARD_KEPT as i64 {
            library.record_heard(&heard("c", 100 + n)).unwrap();
        }
        let all = library.heard_at(0, HEARD_KEPT + 10).unwrap();
        assert_eq!(all.len(), HEARD_KEPT);
        assert!(all.iter().all(|h| h.track == "c"));
    }

    #[test]
    fn a_followed_list_keeps_its_songs_as_the_list_has_them() {
        let mut library = Library::open_in_memory().unwrap();
        let song = |title: &str| crate::import::Song {
            title: title.into(),
            artist: "Someone".into(),
            ..Default::default()
        };
        let ids = library
            .adopt_songs(&[song("A"), song("B"), song("C")])
            .unwrap();
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let (id, changed) = library
            .follow_list("youtube:VLx", "Hits", &[a, b, a, 999], 10)
            .unwrap();
        assert!(changed);
        let items = |library: &Library| -> Vec<i64> {
            library
                .playlist_tracks_at(id, 0, 100)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect()
        };
        // Each song once, only songs the library has.
        assert_eq!(items(&library), vec![a, b]);
        let row = library
            .playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        assert!(row.followed);
        assert_eq!(
            (
                row.name.as_str(),
                row.follows.as_deref(),
                row.follows_checked_ms
            ),
            ("Hits", Some("youtube:VLx"), Some(10))
        );
        // Read again: the same list is the same playlist; the listener's name stays; the songs follow the list.
        library.rename_playlist(id, "My hits", 15).unwrap();
        let (again, changed) = library
            .follow_list("youtube:VLx", "Hits (new)", &[c, b], 20)
            .unwrap();
        assert_eq!((again, changed), (id, true));
        assert_eq!(items(&library), vec![c, b]);
        let (_, changed) = library
            .follow_list("youtube:VLx", "Hits", &[c, b], 30)
            .unwrap();
        assert!(!changed);
        let row = library
            .playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap();
        assert_eq!(
            (row.name.as_str(), row.follows_checked_ms),
            ("My hits", Some(30))
        );
        assert_eq!(
            library.followed_lists().unwrap(),
            vec![(id, "youtube:VLx".to_string(), 30)]
        );
        assert_eq!(library.follows(id).unwrap().as_deref(), Some("youtube:VLx"));
        // A playlist of the listener's own follows nothing.
        let own = library.create_playlist("Mine", 40).unwrap();
        assert_eq!(library.follows(own).unwrap(), None);
    }

    #[test]
    fn a_sources_songs_are_found_by_what_it_calls_them() {
        let mut library = Library::open_in_memory().unwrap();
        let kept = library
            .adopt(&crate::import::Song {
                title: "One More Time".into(),
                artist: "Daft Punk".into(),
                refs: vec![("youtube".into(), "FGBhQbmPwH8".into())],
                ..Default::default()
            })
            .unwrap();
        let asked = ["FGBhQbmPwH8".to_string(), "not-kept".to_string()];
        assert_eq!(
            library.rows_of_refs("youtube", &asked).unwrap(),
            vec![Some(kept), None]
        );
        // What another source calls a song is its own.
        assert_eq!(
            library.rows_of_refs("mornye.abc", &asked).unwrap(),
            vec![None, None]
        );
    }

    #[test]
    fn a_songs_unsure_tries_are_kept_and_strike_only_after_another_song_was_saved() {
        let mut library = Library::open_in_memory().unwrap();
        let song = |title: &str| crate::import::Song {
            title: title.into(),
            artist: "Someone".into(),
            ..Default::default()
        };
        let ids = library
            .adopt_songs(&[song("Alpha"), song("Bravo")])
            .unwrap();
        let (alpha, bravo) = (ids[0], ids[1]);
        library.ask_downloads(&ids, 1).unwrap();
        let had = |strikes, asks, since_ms| UnsureTries {
            strikes,
            asks,
            since_ms,
        };
        // The first try: noted, no strike (there is no try before it).
        assert_eq!(
            library.download_unsure(alpha, true, 100).unwrap(),
            had(0, 1, 100)
        );
        // A try that asked nothing is not noted; with nothing saved since, a try is no strike.
        assert_eq!(
            library.download_unsure(alpha, false, 150).unwrap(),
            had(0, 1, 100)
        );
        assert_eq!(
            library.download_unsure(alpha, true, 200).unwrap(),
            had(0, 2, 100)
        );
        // Another song saved since its try before, from a copy kept from playing it: no add-on gave it, so it says
        // nothing of them.
        let save = |library: &mut Library, id: i64, at_ms: i64, fetched: bool| {
            library
                .conn
                .execute(
                    "UPDATE downloads SET state = 'saving' WHERE track_id = ?1",
                    [id],
                )
                .unwrap();
            let file = crate::library::NewTrack {
                title: "Bravo".into(),
                artist: "Someone".into(),
                album_artist: None,
                album: None,
                year: None,
                disc_no: 0,
                track_no: 0,
                duration_ms: 1_000,
                isrc: None,
                origin: crate::library::Origin::File,
                addon_id: None,
                locator: format!("C:/Saved/{id}.flac"),
                codec: Some("flac".into()),
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                added_at: 0,
                file_size: Some(10),
                file_mtime: Some(0),
            };
            let saved = SavedDownload {
                addon: "demo".into(),
                tier: None,
                at_ms,
                fetched,
                cover_inside: false,
            };
            assert!(library.download_saved(id, &file, &saved).unwrap());
        };
        save(&mut library, bravo, 250, false);
        assert_eq!(
            library.download_unsure(alpha, true, 300).unwrap(),
            had(0, 3, 100)
        );
        // Given by an add-on since: a strike. The same save is not counted twice.
        save(&mut library, bravo, 350, true);
        assert_eq!(
            library.download_unsure(alpha, true, 400).unwrap(),
            had(1, 4, 100)
        );
        assert_eq!(
            library.download_unsure(alpha, true, 450).unwrap(),
            had(1, 5, 100)
        );
        // Try again starts every song on the list over.
        library.retry_downloads(&[], 500).unwrap();
        assert_eq!(
            library.download_unsure(alpha, true, 600).unwrap(),
            had(0, 1, 600)
        );
        // So does the song's own save; one off the list has had nothing.
        library
            .conn
            .execute(
                "UPDATE downloads SET state = 'saving' WHERE track_id = ?1",
                [alpha],
            )
            .unwrap();
        library.forget_downloads(&[alpha]).unwrap();
        assert_eq!(
            library.download_unsure(alpha, true, 700).unwrap(),
            UnsureTries::default()
        );
    }

    #[test]
    fn times_are_written_as_javascript_writes_them() {
        assert_eq!(iso_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_millis(1_759_838_405_000), "2025-10-07T12:00:05.000Z");
        assert_eq!(iso_millis(1_709_251_199_999), "2024-02-29T23:59:59.999Z");
        assert_eq!(iso_millis(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_millis(4_102_444_800_000), "2100-01-01T00:00:00.000Z");
    }
}

/// What a song on the downloads has had of trouble that may be its own (see [`Library::download_unsure`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnsureTries {
    /// Its tries after an add-on gave another song since its try before (a copy kept from playing one does not count).
    pub strikes: u32,
    /// Its tries that asked something.
    pub asks: u32,
    /// When it first had such trouble (since it was asked for, or last saved).
    pub since_ms: i64,
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
    /// An add-on gave it just now (not a copy kept from playing it): it tells that the add-on gives songs.
    pub fetched: bool,
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

/// The songs no add-on could find: how many are on the list, and every one known not to play.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct UnfoundCounts {
    pub open: u64,
    pub marked: Vec<i64>,
}

/// A song on the list of songs no add-on could find.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UnfoundRow {
    #[serde(flatten)]
    pub track: crate::library::TrackRow,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
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
