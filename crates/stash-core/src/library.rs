//! The library: every song the listener has, from files and from add-ons, in one database.
//!
//! Reads are shaped for a UI that shows a hundred thousand rows without loading them: a page at any scroll position
//! ([`Library::tracks_at`]), the next page after a row ([`Library::tracks_after`]), and search as the listener types
//! ([`Library::search`]).

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::{Result, db};

/// Where a track plays from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// A file on this computer; the locator is its path.
    File,
    /// A track an add-on serves; the locator is the add-on's own id for it.
    Addon,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::File => "file",
            Origin::Addon => "addon",
        }
    }
}

/// A track on its way into the library.
#[derive(Debug, Clone)]
pub struct NewTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    pub origin: Origin,
    pub addon_id: Option<String>,
    pub locator: String,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    /// Seconds since the Unix epoch.
    pub added_at: i64,
}

/// A track as a list row shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: u64,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub origin: String,
    /// The row's place in the title order: hand it back to [`Library::tracks_after`] for the next page.
    pub sort_key: String,
}

/// An album as a grid tile shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlbumRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub year: Option<i32>,
    pub track_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

pub struct Library {
    conn: Connection,
}

/// The most matches a search ranks. Above it the query is broad and answered from the title index instead
/// ([`Library::search`]). Ranking 2,000 rows takes a few milliseconds; ranking 50,000 took about 170 ms.
pub const RANK_CAP: usize = 2_000;

const TRACK_COLUMNS: &str = "t.id, t.title, a.name, al.title, t.duration_ms, t.codec, t.sample_rate, t.bit_depth, t.origin, t.sort_key";
const TRACK_JOINS: &str =
    "JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id";

fn track_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TrackRow> {
    Ok(TrackRow {
        id: r.get(0)?,
        title: r.get(1)?,
        artist: r.get(2)?,
        album: r.get(3)?,
        duration_ms: r.get::<_, i64>(4)? as u64,
        codec: r.get(5)?,
        sample_rate: r.get(6)?,
        bit_depth: r.get(7)?,
        origin: r.get(8)?,
        sort_key: r.get(9)?,
    })
}

impl Library {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: db::open(path)?,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            conn: db::open_in_memory()?,
        })
    }

    pub fn counts(&self) -> Result<Counts> {
        let n = |table: &str| -> Result<u64> {
            Ok(self
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
                    r.get::<_, i64>(0)
                })? as u64)
        };
        Ok(Counts {
            tracks: n("tracks")?,
            albums: n("albums")?,
            artists: n("artists")?,
        })
    }

    /// Adds tracks in one transaction and returns how many went in. Artists and albums are made as they are first
    /// named; an album is its title under one artist.
    pub fn add_tracks(&mut self, tracks: impl IntoIterator<Item = NewTrack>) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut added = 0usize;
        {
            let mut artist_ids: HashMap<String, i64> = HashMap::new();
            let mut album_ids: HashMap<(i64, String), i64> = HashMap::new();
            let mut find_artist = tx.prepare("SELECT id FROM artists WHERE name = ?1")?;
            let mut add_artist =
                tx.prepare("INSERT INTO artists(name, sort_key) VALUES (?1, ?2)")?;
            let mut find_album =
                tx.prepare("SELECT id FROM albums WHERE artist_id = ?1 AND title = ?2")?;
            let mut add_album = tx.prepare(
                "INSERT INTO albums(title, artist_id, year, sort_key) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut add_track = tx.prepare(
                "INSERT INTO tracks(title, artist_id, album_id, disc_no, track_no, duration_ms, isrc, origin, addon_id, locator,
                                    codec, sample_rate, bit_depth, added_at, sort_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            )?;
            let mut add_fts = tx.prepare(
                "INSERT INTO tracks_fts(rowid, title, artist, album) VALUES (?1, ?2, ?3, ?4)",
            )?;

            for t in tracks {
                let artist_id = match artist_ids.get(&t.artist) {
                    Some(id) => *id,
                    None => {
                        let found: Option<i64> =
                            find_artist.query_row([&t.artist], |r| r.get(0)).ok();
                        let id = match found {
                            Some(id) => id,
                            None => {
                                add_artist.execute(params![t.artist, sort_key(&t.artist)])?;
                                tx.last_insert_rowid()
                            }
                        };
                        artist_ids.insert(t.artist.clone(), id);
                        id
                    }
                };
                let album_id = match &t.album {
                    None => None,
                    Some(title) => {
                        let key = (artist_id, title.clone());
                        Some(match album_ids.get(&key) {
                            Some(id) => *id,
                            None => {
                                let found: Option<i64> = find_album
                                    .query_row(params![artist_id, title], |r| r.get(0))
                                    .ok();
                                let id = match found {
                                    Some(id) => id,
                                    None => {
                                        add_album.execute(params![
                                            title,
                                            artist_id,
                                            t.year,
                                            sort_key(title)
                                        ])?;
                                        tx.last_insert_rowid()
                                    }
                                };
                                album_ids.insert(key, id);
                                id
                            }
                        })
                    }
                };
                add_track.execute(params![
                    t.title,
                    artist_id,
                    album_id,
                    t.disc_no,
                    t.track_no,
                    t.duration_ms as i64,
                    t.isrc,
                    t.origin.as_str(),
                    t.addon_id,
                    t.locator,
                    t.codec,
                    t.sample_rate,
                    t.bit_depth,
                    t.added_at,
                    sort_key(&t.title),
                ])?;
                let id = tx.last_insert_rowid();
                add_fts.execute(params![id, t.title, t.artist, t.album])?;
                added += 1;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// Search as the listener types: every word must start a word in the title, artist or album ("radioh cre" finds
    /// Radiohead's Creep). Title matches rank above artist, artist above album.
    ///
    /// Ranking costs time for every match, and a broad query ("lo", "love") matches tens of thousands of rows that
    /// no order could tell apart usefully. So the matches are first counted up to [`RANK_CAP`], which stops early:
    /// - few enough: they are ranked, best first;
    /// - more: titles that start with what was typed come first, in title order, then other matches. Typing on
    ///   narrows it into the ranked case.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<TrackRow>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut probe = self
            .conn
            .prepare_cached("SELECT rowid FROM tracks_fts WHERE tracks_fts MATCH ?1 LIMIT ?2")?;
        let matched: Vec<i64> = probe
            .query_map(params![fts, RANK_CAP as i64 + 1], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        if matched.len() <= RANK_CAP {
            // The hits and their order come from the index alone; only the rows shown are then read.
            let sql = format!(
                "WITH hits AS (
                     SELECT rowid, rank FROM tracks_fts
                     WHERE tracks_fts MATCH ?1 AND rank MATCH 'bm25(10.0, 5.0, 2.0)'
                     ORDER BY rank LIMIT ?2)
                 SELECT {TRACK_COLUMNS} FROM hits JOIN tracks t ON t.id = hits.rowid {TRACK_JOINS}
                 ORDER BY hits.rank"
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let rows = stmt
                .query_map(params![fts, limit as i64], track_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(rows);
        }

        // Broad: titles starting with the typed text (a range of the title index), then the first other matches.
        let typed = sort_key(query);
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM tracks t {TRACK_JOINS}
             WHERE t.sort_key >= ?1 AND t.sort_key < ?2
             ORDER BY t.sort_key, t.id LIMIT ?3"
        );
        let mut by_title = self.conn.prepare_cached(&sql)?;
        let mut rows = by_title
            .query_map(
                params![typed, format!("{typed}\u{10FFFF}"), limit as i64],
                track_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() < limit {
            let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks t {TRACK_JOINS} WHERE t.id = ?1");
            let mut by_id = self.conn.prepare_cached(&sql)?;
            for id in matched {
                if rows.len() >= limit {
                    break;
                }
                if rows.iter().any(|r| r.id == id) {
                    continue;
                }
                rows.push(by_id.query_row([id], track_row)?);
            }
        }
        Ok(rows)
    }

    /// The page of tracks at `offset` in title order: what a list shows after the scrollbar is dragged anywhere.
    pub fn tracks_at(&self, offset: usize, limit: usize) -> Result<Vec<TrackRow>> {
        // The inner query walks the (sort_key, id) index only; the rows are read for the page alone.
        let sql = format!(
            "SELECT {TRACK_COLUMNS}
             FROM (SELECT id FROM tracks ORDER BY sort_key, id LIMIT ?1 OFFSET ?2) p
             JOIN tracks t ON t.id = p.id {TRACK_JOINS}
             ORDER BY t.sort_key, t.id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], track_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The page after a row (`sort_key`, `id`), or the first page: what a list asks for while it scrolls on. The cost
    /// does not grow with the position.
    pub fn tracks_after(&self, after: Option<(&str, i64)>, limit: usize) -> Result<Vec<TrackRow>> {
        let (key, id) = after.unwrap_or(("", 0));
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM tracks t {TRACK_JOINS}
             WHERE (t.sort_key, t.id) > (?1, ?2)
             ORDER BY t.sort_key, t.id LIMIT ?3"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt
            .query_map(params![key, id, limit as i64], track_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The page of albums at `offset` in title order, each with its track count: an album grid at any scroll position.
    pub fn albums_at(&self, offset: usize, limit: usize) -> Result<Vec<AlbumRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT al.id, al.title, a.name, al.year,
                    (SELECT COUNT(*) FROM tracks t WHERE t.album_id = al.id)
             FROM (SELECT id FROM albums ORDER BY sort_key, id LIMIT ?1 OFFSET ?2) p
             JOIN albums al ON al.id = p.id JOIN artists a ON a.id = al.artist_id
             ORDER BY al.sort_key, al.id",
        )?;
        let rows = stmt
            .query_map(params![limit as i64, offset as i64], |r| {
                Ok(AlbumRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    year: r.get(3)?,
                    track_count: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

/// How a name sorts: lower case, without a leading article, so "The Beatles" files under B.
pub fn sort_key(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    lower
}

/// The listener's words as an FTS5 query: each word quoted (so punctuation and FTS operators in a title are just
/// text), all required, and each one the start of a word in the library, since people stop typing a word as soon as
/// it is enough ("radioh cre"). `None` when there is no word to search for.
fn fts_query(input: &str) -> Option<String> {
    let parts: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, album: &str, no: u32) -> NewTrack {
        NewTrack {
            title: title.into(),
            artist: artist.into(),
            album: Some(album.into()),
            year: Some(1993),
            disc_no: 1,
            track_no: no,
            duration_ms: 200_000,
            isrc: None,
            origin: Origin::File,
            addon_id: None,
            locator: format!("C:/music/{artist}/{album}/{no:02} {title}.flac"),
            codec: Some("flac".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            added_at: 1_700_000_000,
        }
    }

    fn sample() -> Library {
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_tracks([
            track("Creep", "Radiohead", "Pablo Honey", 2),
            track("You", "Radiohead", "Pablo Honey", 1),
            track("Creep", "Stone Temple Pilots", "Core", 9),
            track("The Bends", "Radiohead", "The Bends", 2),
            track("Björk's Song (Live) AND more", "Björk", "Debut", 1),
        ])
        .unwrap();
        lib
    }

    #[test]
    fn artists_and_albums_are_made_once() {
        let lib = sample();
        assert_eq!(
            lib.counts().unwrap(),
            Counts {
                tracks: 5,
                albums: 4,
                artists: 3
            }
        );
    }

    #[test]
    fn search_needs_every_word_and_finishes_the_last_one() {
        let lib = sample();
        let titles = |q: &str| {
            lib.search(q, 10)
                .unwrap()
                .into_iter()
                .map(|t| format!("{} / {}", t.title, t.artist))
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("radioh cre"), ["Creep / Radiohead"]);
        assert_eq!(titles("creep").len(), 2);
        assert_eq!(titles("pablo").len(), 2, "an album's name finds its tracks");
        assert!(titles("zzz").is_empty());
        assert!(titles("   ").is_empty());
    }

    #[test]
    fn a_broad_search_answers_from_the_title_index_titles_that_start_with_it_first() {
        let mut lib = Library::open_in_memory().unwrap();
        // More matches than a search ranks: "love" is in every album name, and in four titles.
        let mut tracks: Vec<NewTrack> = (0..RANK_CAP as u32 + 50)
            .map(|i| track(&format!("Song {i}"), "Somebody", "Love Songs", i))
            .collect();
        tracks.push(track("Lovely Day", "Bill Withers", "Menagerie", 1));
        tracks.push(track("Love Me Do", "The Beatles", "Please Please Me", 8));
        tracks.push(track(
            "A Love Supreme",
            "John Coltrane",
            "A Love Supreme",
            1,
        ));
        tracks.push(track("Crazy in Love", "Beyoncé", "Dangerously in Love", 1));
        lib.add_tracks(tracks).unwrap();

        let hits = lib.search("love", 10).unwrap();
        let titles: Vec<&str> = hits.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(hits.len(), 10, "a full page");
        // Titles that start with "love" (articles aside), in title order; then other matches.
        assert_eq!(&titles[..3], ["Love Me Do", "A Love Supreme", "Lovely Day"]);
        assert!(titles[3..].iter().all(|t| t.starts_with("Song ")));
        // No row twice, and typing on narrows it into a ranked answer.
        let mut ids: Vec<i64> = hits.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 10);
        assert_eq!(
            lib.search("love supr", 10).unwrap()[0].title,
            "A Love Supreme"
        );
        assert_eq!(
            lib.search("crazy love", 10).unwrap()[0].title,
            "Crazy in Love"
        );
    }

    #[test]
    fn a_title_match_ranks_above_an_album_match() {
        let lib = sample();
        // "bends" is a title and an album for one track, only an album for none other; "the bends" finds it first.
        let hits = lib.search("bends", 10).unwrap();
        assert_eq!(hits[0].title, "The Bends");
    }

    #[test]
    fn accents_punctuation_and_fts_operators_are_just_text() {
        let lib = sample();
        assert_eq!(
            lib.search("bjork", 10).unwrap().len(),
            1,
            "an unaccented search finds an accented name"
        );
        assert_eq!(lib.search("song (live) AND", 10).unwrap().len(), 1);
        assert_eq!(
            lib.search("\"quoted\" OR creep", 10).unwrap().len(),
            0,
            "OR is a word here, not an operator"
        );
    }

    #[test]
    fn pages_are_in_title_order_without_the_article_and_join_up() {
        let lib = sample();
        let all = lib.tracks_at(0, 10).unwrap();
        let titles: Vec<&str> = all.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "The Bends",
                "Björk's Song (Live) AND more",
                "Creep",
                "Creep",
                "You"
            ]
        );
        // The page at an offset and the page after a row are the same rows.
        let by_offset = lib.tracks_at(2, 2).unwrap();
        let by_cursor = lib
            .tracks_after(Some((&all[1].sort_key, all[1].id)), 2)
            .unwrap();
        assert_eq!(by_offset, by_cursor);
        assert_eq!(lib.tracks_after(None, 2).unwrap(), all[..2]);
    }

    #[test]
    fn albums_come_with_their_track_count() {
        let lib = sample();
        let albums = lib.albums_at(0, 10).unwrap();
        let pablo = albums.iter().find(|a| a.title == "Pablo Honey").unwrap();
        assert_eq!(
            (pablo.artist.as_str(), pablo.track_count, pablo.year),
            ("Radiohead", 2, Some(1993))
        );
        assert_eq!(
            albums.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            ["The Bends", "Core", "Debut", "Pablo Honey"]
        );
    }
}
