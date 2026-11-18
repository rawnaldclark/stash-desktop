//! Stash's mixes, after Android's (`StashMixDefaults`, `LastFmRecommendationSource`, `StashMixRefreshWorker`):
//! **Daily Discover**, songs by artists like the ones played lately (Last.fm's similar artists, three top songs of each),
//! and **Recommended by Last.fm**, songs like the ones played lately and the listener's top songs on Last.fm (only with
//! a Last.fm account). Both are 40 songs the library does not have, new every day.
//!
//! What a mix found is kept (`discover_finds`, schema 24): a song once found is not looked for again for 30 days, and
//! each day's 40 are drawn from all of the mix's finds, the newest share always among them (Android's
//! `rotateSurvivorWindow`). Here: the seeds, the rotation, and the finds as the library keeps them. Asking Last.fm and
//! finding each song on YouTube Music is `stashd`'s.
use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::Result;
use crate::identity::words_key;
use crate::library::Library;
use crate::shuffle::Chance;

/// Daily Discover: similar artists' top songs.
pub const DAILY: &str = "daily";
/// Recommended by Last.fm: songs like the listener's.
pub const LASTFM: &str = "lastfm";
/// The mixes, in the order a song is offered to them: the narrower one first, so the other does not take its songs.
pub const MIXES: [&str; 2] = [LASTFM, DAILY];

/// A mix's songs (Android's `targetLength`).
pub const LENGTH: usize = 40;
/// The newest share of a mix's songs, always among the day's (Android's `FRESH_HEAD_RATIO`).
const FRESH_SHARE: f64 = 0.3;
/// The most finds a day's songs are drawn from (Android's `SURVIVOR_POOL_LIMIT`).
const POOL: usize = 500;
/// A song found (or not found) is not looked for again for this long: then it may come back.
pub const LOOK_AGAIN_MS: i64 = 30 * 86_400_000;
/// "Lately": the plays seeds start from, before the longer run fills in (Android's 48 hours).
pub const LATELY_MS: i64 = 48 * 3_600_000;
/// The longer run of plays, when there is no Last.fm account to say it.
pub const LONG_RUN_MS: i64 = 180 * 86_400_000;
/// Daily Discover's seed artists (Android's `TOP_ARTISTS_LIMIT`).
pub const SEED_ARTISTS: usize = 8;
/// Recommended by Last.fm's seed songs, and how many of them are asked about.
pub const SEED_SONGS: usize = 20;
pub const SONGS_ASKED: usize = 10;

/// A mix's name and words, as Android has them.
pub fn title(mix: &str) -> &'static str {
    if mix == LASTFM {
        "Recommended by Last.fm"
    } else {
        "Daily Discover"
    }
}
pub fn description(mix: &str) -> &'static str {
    if mix == LASTFM {
        "Songs like the ones you play, from Last.fm."
    } else {
        "Fresh finds via similar artists, new every day."
    }
}

/// What was played lately leads; what was played over a longer run fills in. No name twice, `limit` at most.
pub fn recent_first<T: PartialEq + Clone>(recent: &[T], longer: &[T], limit: usize) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in recent.iter().chain(longer) {
        if out.len() >= limit {
            break;
        }
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
    out
}

/// A day's songs from `pool` (newest first): the newest share of `cap` always, the rest drawn by `seed`, all in an order
/// drawn by `seed` too, so a new day (or Refresh) moves every place, the first ones and the cover too.
pub fn rotate<T: Clone>(pool: &[T], cap: usize, seed: u64) -> Vec<T> {
    let mut chance = Chance::new(seed);
    if cap == 0 {
        return Vec::new();
    }
    if pool.len() <= cap {
        let mut all = pool.to_vec();
        chance.mix(&mut all);
        return all;
    }
    let fresh = ((cap as f64) * FRESH_SHARE).round() as usize;
    let mut rest = pool[fresh..].to_vec();
    chance.mix(&mut rest);
    let mut out: Vec<T> = pool[..fresh].to_vec();
    out.extend(rest.into_iter().take(cap - fresh));
    chance.mix(&mut out);
    out
}

/// A song a mix found: what Last.fm named, and the YouTube Music song it was found as (None: not found there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Find {
    pub title: String,
    pub artist: String,
    /// The artist or song the find came from.
    pub seed: String,
    pub found_at_ms: i64,
    pub song: Option<FoundSong>,
}

/// The YouTube Music song a find plays as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundSong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub art: Option<String>,
    pub explicit: bool,
    pub artist_id: Option<String>,
    pub album_id: Option<String>,
}

/// A mix as it stands: when its songs were last drawn, and the day they were drawn for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MixState {
    pub mix: String,
    pub drawn_ms: Option<i64>,
    pub day: Option<i64>,
}

impl Library {
    /// The artists played most since `since_ms` (the library's own plays and plays of songs it does not have), most
    /// played first.
    pub fn artists_played_since(&self, since_ms: i64, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT artist, COUNT(*) AS n FROM (
                SELECT ar.name AS artist FROM plays p JOIN tracks t ON t.id = p.track_id JOIN artists ar ON ar.id = t.artist_id
                 WHERE p.played_at_ms >= ?1
                UNION ALL
                SELECT h.artist FROM heard h WHERE h.played_at_ms >= ?1
             ) GROUP BY lower(artist) ORDER BY n DESC, artist LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since_ms, limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The songs played most since `since_ms`, as (artist, title), most played first.
    pub fn songs_played_since(&self, since_ms: i64, limit: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT artist, title, COUNT(*) AS n FROM (
                SELECT ar.name AS artist, t.title AS title FROM plays p JOIN tracks t ON t.id = p.track_id
                  JOIN artists ar ON ar.id = t.artist_id WHERE p.played_at_ms >= ?1
                UNION ALL
                SELECT h.artist, h.title FROM heard h WHERE h.played_at_ms >= ?1
             ) GROUP BY lower(artist), lower(title) ORDER BY n DESC, artist, title LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since_ms, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The artists with the most songs in the library, for a listener who has played nothing yet.
    pub fn artists_with_most_songs(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT ar.name FROM tracks t JOIN artists ar ON ar.id = t.artist_id
              GROUP BY ar.id ORDER BY COUNT(*) DESC, ar.name LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The words of every song the library has, and of every song and artist the listener chose not to play: a mix
    /// offers none of them.
    pub fn not_for_mixes(&self) -> Result<(HashSet<String>, HashSet<String>)> {
        let mut songs: HashSet<String> = HashSet::new();
        let mut stmt = self.conn.prepare("SELECT words_key FROM tracks")?;
        for key in stmt.query_map([], |r| r.get::<_, String>(0))? {
            songs.insert(key?);
        }
        let mut artists: HashSet<String> = HashSet::new();
        let mut stmt = self.conn.prepare("SELECT kind, key FROM blocked")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (kind, key) = row?;
            if kind == "artist" {
                artists.insert(key);
            } else {
                songs.insert(key);
            }
        }
        Ok((songs, artists))
    }

    /// Whether `mix` looked for this song since `since_ms` (found or not).
    pub fn looked_for(&self, mix: &str, title: &str, artist: &str, since_ms: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM discover_finds WHERE mix = ?1 AND key = ?2 AND found_at_ms >= ?3",
                params![mix, words_key(title, artist), since_ms],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Keeps what `mix` found (or did not find) for a song; a song looked for again replaces what was kept.
    pub fn note_find(&self, mix: &str, find: &Find) -> Result<()> {
        let s = find.song.as_ref();
        self.conn.execute(
            "INSERT OR REPLACE INTO discover_finds(mix, key, title, artist, seed, found_at_ms, song_id, song_title,
                song_artist, album, duration_ms, art_url, explicit, artist_id, album_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                mix,
                words_key(&find.title, &find.artist),
                find.title,
                find.artist,
                find.seed,
                find.found_at_ms,
                s.map(|s| &s.id),
                s.map(|s| &s.title),
                s.map(|s| &s.artist),
                s.and_then(|s| s.album.as_ref()),
                s.and_then(|s| s.duration_ms).map(|d| d as i64),
                s.and_then(|s| s.art.as_ref()),
                s.is_some_and(|s| s.explicit),
                s.and_then(|s| s.artist_id.as_ref()),
                s.and_then(|s| s.album_id.as_ref()),
            ],
        )?;
        Ok(())
    }

    /// What `mix` found on YouTube Music, newest first, without songs the library has now or the listener chose not
    /// to play (by Last.fm's words or the song's own), nor a song twice.
    pub fn finds(&self, mix: &str) -> Result<Vec<Find>> {
        let (not_songs, not_artists) = self.not_for_mixes()?;
        let mut stmt = self.conn.prepare(
            "SELECT title, artist, seed, found_at_ms, song_id, song_title, song_artist, album, duration_ms, art_url,
                    explicit, artist_id, album_id
               FROM discover_finds WHERE mix = ?1 AND song_id IS NOT NULL ORDER BY found_at_ms DESC, key LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![mix, POOL as i64], |r| {
            Ok(Find {
                title: r.get(0)?,
                artist: r.get(1)?,
                seed: r.get(2)?,
                found_at_ms: r.get(3)?,
                song: Some(FoundSong {
                    id: r.get(4)?,
                    title: r.get(5)?,
                    artist: r.get(6)?,
                    album: r.get(7)?,
                    duration_ms: r.get::<_, Option<i64>>(8)?.map(|d| d.max(0) as u64),
                    art: r.get(9)?,
                    explicit: r.get(10)?,
                    artist_id: r.get(11)?,
                    album_id: r.get(12)?,
                }),
            })
        })?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for find in rows {
            let find = find?;
            let Some(song) = &find.song else { continue };
            let keys = [
                words_key(&find.title, &find.artist),
                words_key(&song.title, &song.artist),
            ];
            let artists = [
                crate::identity::fold(&find.artist),
                crate::identity::fold(&song.artist),
            ];
            if keys.iter().any(|k| not_songs.contains(k))
                || artists.iter().any(|a| not_artists.contains(a))
                || !seen.insert(song.id.clone())
            {
                continue;
            }
            out.push(find);
        }
        Ok(out)
    }

    /// The day's songs of `mix`, as drawn (`draw_mix`), still in the order drawn; songs the library got since leave.
    pub fn mix_songs(&self, mix: &str) -> Result<Vec<Find>> {
        let drawn: Option<String> = self
            .conn
            .query_row(
                "SELECT songs FROM discover_mixes WHERE mix = ?1",
                params![mix],
                |r| r.get(0),
            )
            .optional()?;
        let Some(drawn) = drawn else {
            return Ok(Vec::new());
        };
        let order: Vec<String> = serde_json::from_str(&drawn).unwrap_or_default();
        let finds = self.finds(mix)?;
        Ok(order
            .iter()
            .filter_map(|id| {
                finds
                    .iter()
                    .find(|f| f.song.as_ref().is_some_and(|s| &s.id == id))
                    .cloned()
            })
            .collect())
    }

    /// Draws the day's songs of `mix` from its finds (`rotate`, by `seed`) and keeps them, for `day`.
    pub fn draw_mix(&self, mix: &str, day: i64, seed: u64, now_ms: i64) -> Result<usize> {
        let finds = self.finds(mix)?;
        let ids: Vec<String> = finds
            .iter()
            .filter_map(|f| f.song.as_ref().map(|s| s.id.clone()))
            .collect();
        let drawn = rotate(&ids, LENGTH, seed);
        self.conn.execute(
            "INSERT OR REPLACE INTO discover_mixes(mix, songs, day, drawn_ms) VALUES (?1, ?2, ?3, ?4)",
            params![mix, serde_json::to_string(&drawn).unwrap_or_else(|_| "[]".into()), day, now_ms],
        )?;
        Ok(drawn.len())
    }

    /// When `mix` was drawn last, and for which day.
    pub fn mix_state(&self, mix: &str) -> Result<MixState> {
        let row = self
            .conn
            .query_row(
                "SELECT day, drawn_ms FROM discover_mixes WHERE mix = ?1",
                params![mix],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(MixState {
            mix: mix.to_string(),
            day: row.map(|r| r.0),
            drawn_ms: row.map(|r| r.1),
        })
    }

    /// Forgets a mix (its finds and its songs): Recommended by Last.fm when the account goes.
    pub fn forget_mix(&self, mix: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM discover_finds WHERE mix = ?1", params![mix])?;
        self.conn
            .execute("DELETE FROM discover_mixes WHERE mix = ?1", params![mix])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_played_lately_leads_and_nothing_comes_twice() {
        let recent = vec!["A", "B"];
        let longer = vec!["B", "C", "D"];
        assert_eq!(recent_first(&recent, &longer, 3), vec!["A", "B", "C"]);
        assert_eq!(recent_first::<&str>(&[], &longer, 8), vec!["B", "C", "D"]);
    }

    #[test]
    fn a_day_keeps_the_newest_finds_and_draws_the_rest() {
        let pool: Vec<u32> = (0..200).collect();
        let a = rotate(&pool, 40, 7);
        assert_eq!(a.len(), 40);
        // The newest twelve (30 % of 40) are always there.
        assert!((0..12).all(|n| a.contains(&n)));
        // Another day draws others, in another order; the same day draws the same.
        let b = rotate(&pool, 40, 8);
        assert_ne!(a, b);
        assert_eq!(a, rotate(&pool, 40, 7));
        let unique: HashSet<_> = a.iter().collect();
        assert_eq!(unique.len(), 40);
        // A small pool is all of it, in a drawn order.
        let small: Vec<u32> = (0..5).collect();
        let mut drawn = rotate(&small, 40, 3);
        drawn.sort();
        assert_eq!(drawn, small);
    }
}
