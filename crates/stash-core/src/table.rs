//! The library as a table: every song with what its file's tags say (genre, year, composer, bit rate), in any order
//! the listener asks for, a page at a time. This is the library manager's view of the songs; `library.rs` keeps the
//! plain title-ordered list the rest of the window uses.
//!
//! The extra tags are not read by a scan: [`read_more`] reads them from a file, and [`Library::files_without_tags`] /
//! [`Library::set_more_tags`] let a pass in the background fill them in for every file once (files scanned before the
//! library kept them, and files Stash saved).

use std::path::Path;

use lofty::prelude::*;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::Result;
use crate::library::Library;

/// What a file's tags say beyond what a scan keeps.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MoreTags {
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    /// Kilobits a second, as the file's audio says.
    pub bitrate: Option<u32>,
}

/// Reads [`MoreTags`] from the file at `path`.
pub fn read_more(path: &Path) -> std::result::Result<MoreTags, String> {
    let file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let tag = file.primary_tag().or_else(|| file.first_tag());
    let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Ok(MoreTags {
        genre: clean(tag.and_then(|t| t.genre()).map(|g| g.into_owned())),
        year: tag
            .and_then(|t| t.date())
            .map(|d| i32::from(d.year))
            .filter(|y| (1000..=9999).contains(y)),
        composer: clean(tag.and_then(|t| t.get_string(ItemKey::Composer)).map(str::to_string)),
        bitrate: file.properties().audio_bitrate().filter(|b| *b > 0),
    })
}

/// What the table can be put in order by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Title,
    Artist,
    Album,
    Genre,
    Year,
    Length,
    Format,
    Added,
    Plays,
}

impl SortBy {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "title" => Self::Title,
            "artist" => Self::Artist,
            "album" => Self::Album,
            "genre" => Self::Genre,
            "year" => Self::Year,
            "length" => Self::Length,
            "format" => Self::Format,
            "added" => Self::Added,
            "plays" => Self::Plays,
            _ => return None,
        })
    }

    /// The ORDER BY for this column going up; songs with nothing in it come last either way, and ties fall back to
    /// artist, album, disc and track so an album stays together.
    fn order(self, down: bool) -> String {
        let dir = if down { "DESC" } else { "ASC" };
        let then = "a.sort_key, COALESCE(al.title, ''), t.disc_no, t.track_no, t.sort_key, t.id";
        match self {
            Self::Title => format!("t.sort_key {dir}, t.id {dir}"),
            Self::Artist => format!("a.sort_key {dir}, COALESCE(al.title, ''), t.disc_no, t.track_no, t.sort_key, t.id"),
            Self::Album => format!("al.title IS NULL, al.title COLLATE NOCASE {dir}, t.disc_no, t.track_no, t.sort_key, t.id"),
            Self::Genre => format!("t.genre IS NULL, t.genre COLLATE NOCASE {dir}, {then}"),
            Self::Year => format!("COALESCE(t.year, al.year) IS NULL, COALESCE(t.year, al.year) {dir}, {then}"),
            Self::Length => format!("t.duration_ms {dir}, {then}"),
            Self::Format => format!("t.codec IS NULL, t.codec {dir}, t.bit_depth {dir}, t.sample_rate {dir}, {then}"),
            Self::Added => format!("t.added_at {dir}, {then}"),
            Self::Plays => format!("plays {dir}, {then}"),
        }
    }
}

/// What part of the library the table shows.
#[derive(Debug, Default, Clone)]
pub struct TableAsk {
    /// Only songs whose title, artist, album or genre holds this text (plain, any case).
    pub text: Option<String>,
    /// Only this genre, as written ("" asks for the songs with none).
    pub genre: Option<String>,
    /// Only songs that play with no connection: files that are here.
    pub here: bool,
}

/// A song as a row of the table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TableRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub artist_id: i64,
    pub album: Option<String>,
    pub album_id: Option<i64>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub track_no: u32,
    pub duration_ms: u64,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub bitrate: Option<u32>,
    pub added_at_ms: i64,
    pub plays: u32,
    pub liked: bool,
    /// A file that is on this computer now.
    pub here: bool,
}

const FROM: &str = "FROM tracks t JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id";

impl Library {
    fn table_where(ask: &TableAsk, binds: &mut Vec<String>) -> String {
        let mut parts = Vec::new();
        if ask.here {
            parts.push("(t.origin = 'file' AND t.missing_since IS NULL)".to_string());
        }
        if let Some(genre) = &ask.genre {
            if genre.is_empty() {
                parts.push("t.genre IS NULL".to_string());
            } else {
                binds.push(genre.clone());
                parts.push(format!("t.genre = ?{} COLLATE NOCASE", binds.len()));
            }
        }
        if let Some(text) = ask.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            // LIKE, with its own wildcards taken as plain characters.
            let like = format!("%{}%", text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            binds.push(like);
            let n = binds.len();
            parts.push(format!(
                "(t.title LIKE ?{n} ESCAPE '\\' OR a.name LIKE ?{n} ESCAPE '\\' OR al.title LIKE ?{n} ESCAPE '\\' OR t.genre LIKE ?{n} ESCAPE '\\')"
            ));
        }
        if parts.is_empty() { String::new() } else { format!(" WHERE {}", parts.join(" AND ")) }
    }

    /// How many songs the table has for `ask`.
    pub fn table_count(&self, ask: &TableAsk) -> Result<usize> {
        let mut binds = Vec::new();
        let filter = Self::table_where(ask, &mut binds);
        let n: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) {FROM}{filter}"),
            rusqlite::params_from_iter(binds.iter()),
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// A page of the table for `ask`, in the order asked.
    pub fn table_at(&self, ask: &TableAsk, by: SortBy, down: bool, offset: usize, limit: usize) -> Result<Vec<TableRow>> {
        let mut binds = Vec::new();
        let filter = Self::table_where(ask, &mut binds);
        let sql = format!(
            "SELECT t.id, t.title, a.name, t.artist_id, al.title, t.album_id, t.genre, COALESCE(t.year, al.year),
                    t.track_no, t.duration_ms, t.codec, t.sample_rate, t.bit_depth, t.bitrate, t.added_at * 1000,
                    (SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id) AS plays,
                    t.liked_at_ms IS NOT NULL,
                    t.origin = 'file' AND t.missing_since IS NULL
             {FROM}{filter} ORDER BY {} LIMIT {} OFFSET {}",
            by.order(down),
            limit.min(2_000),
            offset
        );
        let mut read = self.conn.prepare(&sql)?;
        let rows = read
            .query_map(rusqlite::params_from_iter(binds.iter()), |r| {
                Ok(TableRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    artist_id: r.get(3)?,
                    album: r.get(4)?,
                    album_id: r.get(5)?,
                    genre: r.get(6)?,
                    year: r.get(7)?,
                    track_no: r.get::<_, i64>(8)?.max(0) as u32,
                    duration_ms: r.get::<_, i64>(9)?.max(0) as u64,
                    codec: r.get(10)?,
                    sample_rate: r.get(11)?,
                    bit_depth: r.get(12)?,
                    bitrate: r.get(13)?,
                    added_at_ms: r.get(14)?,
                    plays: r.get::<_, i64>(15)?.max(0) as u32,
                    liked: r.get(16)?,
                    here: r.get(17)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The ids of the table's songs from `offset` on, in the order asked: what plays when a row is played.
    pub fn table_ids(&self, ask: &TableAsk, by: SortBy, down: bool, offset: usize, limit: usize) -> Result<Vec<i64>> {
        let mut binds = Vec::new();
        let filter = Self::table_where(ask, &mut binds);
        let sql = format!(
            "SELECT t.id, (SELECT COUNT(*) FROM plays p WHERE p.track_id = t.id) AS plays {FROM}{filter} ORDER BY {} LIMIT {} OFFSET {}",
            by.order(down),
            limit.min(5_000),
            offset
        );
        let mut read = self.conn.prepare(&sql)?;
        let ids = read
            .query_map(rusqlite::params_from_iter(binds.iter()), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    /// Every genre the library's songs name, with how many songs each has, the biggest first; `None` counts the songs
    /// with no genre.
    pub fn genres(&self, here: bool) -> Result<Vec<(Option<String>, usize)>> {
        let only = if here { " WHERE origin = 'file' AND missing_since IS NULL" } else { "" };
        let mut read = self.conn.prepare(&format!(
            "SELECT MIN(genre), COUNT(*) FROM tracks{only} GROUP BY genre COLLATE NOCASE ORDER BY genre IS NULL, COUNT(*) DESC, genre COLLATE NOCASE"
        ))?;
        let rows = read
            .query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Files whose extra tags have not been read yet (at most `limit`): each with where it is.
    pub fn files_without_tags(&self, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut read = self.conn.prepare_cached(
            "SELECT id, locator FROM tracks
             WHERE origin = 'file' AND tags_read = 0 AND locator != '' AND missing_since IS NULL
             ORDER BY id LIMIT ?1",
        )?;
        let rows = read
            .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Keeps what was read of the files' extra tags (`None`: the file could not be read; it is not asked again until
    /// a scan finds it changed).
    pub fn set_more_tags(&mut self, read: &[(i64, Option<MoreTags>)]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            let mut set = tx.prepare(
                "UPDATE tracks SET genre = ?2, year = ?3, composer = ?4, bitrate = ?5, tags_read = 1 WHERE id = ?1",
            )?;
            let mut skip = tx.prepare("UPDATE tracks SET tags_read = 1 WHERE id = ?1")?;
            for (id, tags) in read {
                n += match tags {
                    Some(t) => set.execute(params![id, t.genre, t.year, t.composer, t.bitrate])?,
                    None => skip.execute([id])?,
                };
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// How many files still wait to have their extra tags read.
    pub fn tags_waiting(&self) -> Result<usize> {
        let n: Option<i64> = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE origin = 'file' AND tags_read = 0 AND locator != '' AND missing_since IS NULL",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.unwrap_or(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{NewTrack, Origin};

    fn file(title: &str, artist: &str, album: &str, track_no: u32, ms: u64) -> NewTrack {
        NewTrack {
            title: title.into(),
            artist: artist.into(),
            album_artist: None,
            album: Some(album.into()),
            year: None,
            disc_no: 1,
            track_no,
            duration_ms: ms,
            isrc: None,
            origin: Origin::File,
            addon_id: None,
            locator: format!("C:/music/{artist}/{album}/{title}.flac"),
            codec: Some("flac".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            added_at: 100 + track_no as i64,
            file_size: Some(1),
            file_mtime: Some(1),
        }
    }

    fn library() -> (Library, Vec<i64>) {
        let mut library = Library::open_in_memory().unwrap();
        library
            .add_tracks([
                file("Nude", "Radiohead", "In Rainbows", 3, 255_000),
                file("Reckoner", "Radiohead", "In Rainbows", 7, 290_000),
                file("Xtal", "Aphex Twin", "Selected Ambient Works", 1, 294_000),
                file("So What", "Miles Davis", "Kind of Blue", 1, 562_000),
            ])
            .unwrap();
        let ids: Vec<i64> = library.files_without_tags(10).unwrap().into_iter().map(|(id, _)| id).collect();
        (library, ids)
    }

    fn titles(rows: &[TableRow]) -> Vec<&str> {
        rows.iter().map(|r| r.title.as_str()).collect()
    }

    #[test]
    fn the_tags_wait_to_be_read_once_and_are_kept() {
        let (mut library, ids) = library();
        assert_eq!(ids.len(), 4);
        assert_eq!(library.tags_waiting().unwrap(), 4);
        let tags = |genre: &str, year: i32| Some(MoreTags { genre: Some(genre.into()), year: Some(year), composer: None, bitrate: Some(900) });
        library
            .set_more_tags(&[(ids[0], tags("Alternative", 2007)), (ids[1], tags("alternative", 2007)), (ids[2], tags("Electronic", 1992)), (ids[3], None)])
            .unwrap();
        // The one that could not be read is not asked for again.
        assert_eq!(library.tags_waiting().unwrap(), 0);
        // One genre however it was capitalised, the biggest first, and the songs with none last.
        assert_eq!(
            library.genres(false).unwrap(),
            vec![(Some("Alternative".to_string()), 2), (Some("Electronic".to_string()), 1), (None, 1)]
        );
    }

    #[test]
    fn the_table_sorts_by_any_column_and_keeps_an_album_together() {
        let (mut library, ids) = library();
        let tags = |genre: &str, year: i32| Some(MoreTags { genre: Some(genre.into()), year: Some(year), composer: None, bitrate: None });
        library
            .set_more_tags(&[(ids[0], tags("Alternative", 2007)), (ids[1], tags("Alternative", 2007)), (ids[2], tags("Electronic", 1992)), (ids[3], None)])
            .unwrap();
        let all = TableAsk::default();
        let by = |library: &Library, by: SortBy, down: bool| library.table_at(&all, by, down, 0, 50).unwrap();

        assert_eq!(titles(&by(&library, SortBy::Title, false)), ["Nude", "Reckoner", "So What", "Xtal"]);
        assert_eq!(titles(&by(&library, SortBy::Title, true)), ["Xtal", "So What", "Reckoner", "Nude"]);
        // By genre: the album's songs stay in track order, and the song with no genre is last, up or down.
        assert_eq!(titles(&by(&library, SortBy::Genre, false)), ["Nude", "Reckoner", "Xtal", "So What"]);
        assert_eq!(titles(&by(&library, SortBy::Genre, true)), ["Xtal", "Nude", "Reckoner", "So What"]);
        assert_eq!(titles(&by(&library, SortBy::Year, false)), ["Xtal", "Nude", "Reckoner", "So What"]);
        assert_eq!(titles(&by(&library, SortBy::Length, true))[0], "So What");
        assert_eq!(titles(&by(&library, SortBy::Artist, false)), ["Xtal", "So What", "Nude", "Reckoner"]);

        let row = &by(&library, SortBy::Title, false)[0];
        assert_eq!((row.genre.as_deref(), row.year, row.track_no, row.here), (Some("Alternative"), Some(2007), 3, true));
    }

    #[test]
    fn the_table_filters_by_text_and_genre_and_pages() {
        let (mut library, ids) = library();
        let tags = |genre: &str| Some(MoreTags { genre: Some(genre.into()), ..Default::default() });
        library
            .set_more_tags(&[(ids[0], tags("Alternative")), (ids[1], tags("Alternative")), (ids[2], tags("Electronic")), (ids[3], None)])
            .unwrap();
        let ask = |text: Option<&str>, genre: Option<&str>| TableAsk { text: text.map(String::from), genre: genre.map(String::from), here: false };

        assert_eq!(library.table_count(&ask(None, None)).unwrap(), 4);
        assert_eq!(library.table_count(&ask(Some("rainbows"), None)).unwrap(), 2);
        assert_eq!(library.table_count(&ask(Some("electro"), None)).unwrap(), 1); // by genre too
        assert_eq!(library.table_count(&ask(Some("100%"), None)).unwrap(), 0); // its own wildcard is plain text
        assert_eq!(library.table_count(&ask(None, Some("alternative"))).unwrap(), 2);
        assert_eq!(library.table_count(&ask(None, Some(""))).unwrap(), 1); // no genre
        assert_eq!(library.table_count(&ask(Some("nude"), Some("Alternative"))).unwrap(), 1);

        let page = library.table_at(&ask(None, None), SortBy::Title, false, 1, 2).unwrap();
        assert_eq!(titles(&page), ["Reckoner", "So What"]);
        // What plays from the third row on, in the same order.
        let from_third = library.table_ids(&ask(None, None), SortBy::Title, false, 2, 100).unwrap();
        assert_eq!(from_third.len(), 2);
        assert_eq!(from_third[0], library.table_at(&ask(None, None), SortBy::Title, false, 2, 1).unwrap()[0].id);
    }
}
