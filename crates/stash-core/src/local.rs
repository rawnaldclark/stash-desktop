//! The library as it is on this computer: what the listener's own files hold, for Stash run for local files only.
//! Albums and artists that have a file here, and Home's shelves (what was added lately, what is played most, the
//! genres, and how much there is in all).

use rusqlite::params;
use serde::Serialize;

use crate::Result;
use crate::library::{ALBUM_COLUMNS, ARTIST_COLUMNS, AlbumRow, ArtistRow, Library, TRACK_COLUMNS, TRACK_JOINS, TrackRow, album_row, artist_row, track_row};

/// A song that is a file here.
const HERE: &str = "t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL";

/// How much music is on this computer.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct LocalStats {
    pub songs: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub duration_ms: u64,
    pub bytes: u64,
}

/// A genre and how many of the files have it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenreCount {
    pub genre: String,
    pub songs: u64,
}

/// Home, for local files only.
#[derive(Debug, Clone, Serialize)]
pub struct LocalHome {
    pub stats: LocalStats,
    /// The albums whose files came most lately, newest first.
    pub added: Vec<AlbumRow>,
    /// The files played most, most first: only ones played at all.
    pub played: Vec<TrackRow>,
    /// The genres with the most files, most first.
    pub genres: Vec<GenreCount>,
}

/// An album as the grid groups it: the album, and what it is grouped by.
#[derive(Debug, Clone, Serialize)]
pub struct AlbumCard {
    #[serde(flatten)]
    pub album: AlbumRow,
    /// The genre most of its songs have.
    pub genre: Option<String>,
    /// When its newest song came into the library, in milliseconds.
    pub added_ms: i64,
    /// How many of its songs are files here.
    pub songs_here: u32,
}

impl Library {
    /// The library's row for a song another device describes (the phone's remote, `crate::link::remote`), by the identity
    /// rule the web player and the app share: its ISRC first; else its words (title and artist, folded) with no ISRC that
    /// says otherwise. A file on this computer is preferred, then a liked song, then the oldest row.
    pub fn same_song(&self, title: &str, artist: &str, isrc: Option<&str>) -> Result<Option<i64>> {
        let here = "(t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL) DESC, (t.liked_at_ms IS NOT NULL) DESC, t.id";
        if let Some(isrc) = isrc.and_then(crate::identity::isrc) {
            let by_isrc: Option<i64> = self
                .conn
                .prepare_cached(&format!("SELECT t.id FROM tracks t WHERE t.isrc = ?1 ORDER BY {here} LIMIT 1"))?
                .query_map(params![isrc], |r| r.get(0))?
                .next()
                .transpose()?;
            if by_isrc.is_some() {
                return Ok(by_isrc);
            }
        }
        let key = crate::identity::words_key(title, artist);
        let wanted = isrc.and_then(crate::identity::isrc);
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT t.id, t.isrc FROM tracks t WHERE t.words_key = ?1 ORDER BY {here} LIMIT 20"))?;
        let rows: Vec<(i64, Option<String>)> = stmt.query_map(params![key], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows
            .into_iter()
            .find(|(_, theirs)| crate::identity::isrcs_agree(wanted.as_deref(), theirs.as_deref()))
            .map(|(id, _)| id))
    }

    /// Every album (`here`: only those with a file here), in the library's order, each with its main genre, the newest
    /// of its songs' arrival and how many of its songs are files here, for a grid grouped by any of them. An album that
    /// names no year takes its songs' latest.
    pub fn album_cards(&self, here: bool) -> Result<Vec<AlbumCard>> {
        let only = if here { format!("WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = al.id AND {HERE})") } else { String::new() };
        let sql = format!(
            "SELECT {ALBUM_COLUMNS},
                    (SELECT t.genre FROM tracks t WHERE t.album_id = al.id AND COALESCE(t.genre, '') != ''
                     GROUP BY t.genre ORDER BY COUNT(*) DESC, t.genre LIMIT 1),
                    (SELECT MAX(t.year) FROM tracks t WHERE t.album_id = al.id),
                    (SELECT MAX(t.added_at) FROM tracks t WHERE t.album_id = al.id),
                    (SELECT COUNT(*) FROM tracks t WHERE t.album_id = al.id AND {HERE})
             FROM albums al JOIN artists a ON a.id = al.artist_id {only}
             ORDER BY al.sort_key, al.id"
        );
        let rows = self
            .conn
            .prepare(&sql)?
            .query_map([], |r| {
                let mut album = album_row(r)?;
                if album.year.is_none() {
                    album.year = r.get::<_, Option<i32>>(8)?;
                }
                Ok(AlbumCard {
                    album,
                    genre: r.get(7)?,
                    added_ms: r.get::<_, Option<i64>>(9)?.unwrap_or(0) * 1000,
                    songs_here: r.get::<_, i64>(10)?.max(0) as u32,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The albums with a file here, in the library's order.
    pub fn albums_here_at(&self, offset: usize, limit: usize) -> Result<Vec<AlbumRow>> {
        let sql = format!(
            "SELECT {ALBUM_COLUMNS}
             FROM (SELECT x.id FROM albums x
                   WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.album_id = x.id AND {HERE})
                   ORDER BY x.sort_key, x.id LIMIT ?1 OFFSET ?2) p
             JOIN albums al ON al.id = p.id JOIN artists a ON a.id = al.artist_id
             ORDER BY al.sort_key, al.id"
        );
        let rows = self
            .conn
            .prepare_cached(&sql)?
            .query_map(params![limit as i64, offset as i64], album_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The artists with a file here (as a song's artist, or as the artist of an album that has one), in the library's
    /// order.
    pub fn artists_here_at(&self, offset: usize, limit: usize) -> Result<Vec<ArtistRow>> {
        let sql = format!(
            "SELECT {ARTIST_COLUMNS}
             FROM (SELECT x.id FROM artists x
                   WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.artist_id = x.id AND {HERE})
                      OR EXISTS (SELECT 1 FROM albums al JOIN tracks t ON t.album_id = al.id WHERE al.artist_id = x.id AND {HERE})
                   ORDER BY x.sort_key, x.id LIMIT ?1 OFFSET ?2) p
             JOIN artists a ON a.id = p.id ORDER BY a.sort_key, a.id"
        );
        let rows = self
            .conn
            .prepare_cached(&sql)?
            .query_map(params![limit as i64, offset as i64], artist_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Home's shelves for the files on this computer.
    pub fn local_home(&self, shelf: usize) -> Result<LocalHome> {
        let stats = self.conn.query_row(
            &format!(
                "SELECT COUNT(*), COUNT(DISTINCT t.album_id), COUNT(DISTINCT t.artist_id), COUNT(DISTINCT NULLIF(t.genre, '')),
                        COALESCE(SUM(t.duration_ms), 0), COALESCE(SUM(t.file_size), 0)
                 FROM tracks t WHERE {HERE}"
            ),
            [],
            |r| {
                let n = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
                Ok(LocalStats { songs: n(0)?, albums: n(1)?, artists: n(2)?, genres: n(3)?, duration_ms: n(4)?, bytes: n(5)? })
            },
        )?;
        let added = self
            .conn
            .prepare_cached(&format!(
                "SELECT {ALBUM_COLUMNS}
                 FROM (SELECT t.album_id AS id, MAX(t.added_at) AS at FROM tracks t
                       WHERE {HERE} AND t.album_id IS NOT NULL GROUP BY t.album_id ORDER BY at DESC, id DESC LIMIT ?1) p
                 JOIN albums al ON al.id = p.id JOIN artists a ON a.id = al.artist_id
                 ORDER BY p.at DESC, al.id DESC"
            ))?
            .query_map([shelf as i64], album_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let played = self
            .conn
            .prepare_cached(&format!(
                "SELECT {TRACK_COLUMNS} FROM tracks t {TRACK_JOINS}
                 JOIN (SELECT p.track_id AS id, COUNT(*) AS n FROM plays p GROUP BY p.track_id) c ON c.id = t.id
                 WHERE {HERE} ORDER BY c.n DESC, t.id LIMIT ?1"
            ))?
            .query_map([shelf as i64], track_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let genres = self
            .conn
            .prepare_cached(&format!(
                "SELECT t.genre, COUNT(*) AS n FROM tracks t WHERE {HERE} AND COALESCE(t.genre, '') != ''
                 GROUP BY t.genre ORDER BY n DESC, t.genre COLLATE NOCASE LIMIT ?1"
            ))?
            .query_map([shelf as i64], |r| Ok(GenreCount { genre: r.get(0)?, songs: r.get::<_, i64>(1)? as u64 }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(LocalHome { stats, added, played, genres })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Song;

    fn song(library: &mut Library, title: &str, artist: &str, album: &str, file: Option<&str>) -> i64 {
        let id = library.adopt(&Song { title: title.into(), artist: artist.into(), album: Some(album.into()), duration_ms: Some(200_000), ..Default::default() }).unwrap();
        if let Some(file) = file {
            library.conn.execute("UPDATE tracks SET origin = 'file', locator = ?2, file_size = 1000 WHERE id = ?1", params![id, file]).unwrap();
        }
        id
    }

    #[test]
    fn only_what_has_a_file_here_is_listed_and_counted() {
        let mut library = Library::open_in_memory().unwrap();
        let nude = song(&mut library, "Nude", "Radiohead", "In Rainbows", Some("C:/m/nude.flac"));
        let reckoner = song(&mut library, "Reckoner", "Radiohead", "In Rainbows", Some("C:/m/reckoner.flac"));
        let so_what = song(&mut library, "So What", "Miles Davis", "Kind of Blue", Some("C:/m/so.flac"));
        // Only an entry: liked from a streaming source, no file.
        song(&mut library, "Humble", "Kendrick Lamar", "DAMN.", None);
        // A file that has gone missing is not here either.
        let gone = song(&mut library, "Dreams", "Fleetwood Mac", "Rumours", Some("C:/m/dreams.flac"));
        library.conn.execute("UPDATE tracks SET missing_since = 5 WHERE id = ?1", [gone]).unwrap();
        library.conn.execute("UPDATE tracks SET genre = 'Alternative' WHERE id IN (?1, ?2)", [nude, reckoner]).unwrap();
        library.conn.execute("UPDATE tracks SET genre = 'Jazz', added_at = added_at + 100 WHERE id = ?1", [so_what]).unwrap();

        let albums: Vec<_> = library.albums_here_at(0, 50).unwrap().into_iter().map(|a| a.title).collect();
        assert_eq!(albums, vec!["In Rainbows", "Kind of Blue"]);
        assert_eq!(library.albums_at(0, 50).unwrap().len(), 4);
        let artists: Vec<_> = library.artists_here_at(0, 50).unwrap().into_iter().map(|a| a.name).collect();
        assert_eq!(artists, vec!["Miles Davis", "Radiohead"]);
        assert_eq!(library.artists_here_at(1, 50).unwrap().len(), 1);

        let home = library.local_home(12).unwrap();
        assert_eq!(home.stats, LocalStats { songs: 3, albums: 2, artists: 2, genres: 2, duration_ms: 600_000, bytes: 3000 });
        // The album whose file came last is first.
        assert_eq!(home.added.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["Kind of Blue", "In Rainbows"]);
        assert_eq!(home.genres, vec![GenreCount { genre: "Alternative".into(), songs: 2 }, GenreCount { genre: "Jazz".into(), songs: 1 }]);
        // Nothing played yet: the shelf is empty, not a list of unplayed songs.
        assert!(home.played.is_empty());

        // The grid's cards: every album, or only those with a file here; each with its main genre, a year taken from
        // its songs when the album names none, and how many of its songs are here.
        library.conn.execute("UPDATE tracks SET year = 2007 WHERE id = ?1", [nude]).unwrap();
        assert_eq!(library.album_cards(false).unwrap().len(), 4);
        let cards = library.album_cards(true).unwrap();
        let rainbows = cards.iter().find(|c| c.album.title == "In Rainbows").unwrap();
        assert_eq!((rainbows.genre.as_deref(), rainbows.album.year, rainbows.songs_here), (Some("Alternative"), Some(2007), 2));
        let blue = cards.iter().find(|c| c.album.title == "Kind of Blue").unwrap();
        assert_eq!((blue.genre.as_deref(), blue.album.year, blue.songs_here), (Some("Jazz"), None, 1));
        assert!(blue.added_ms > rainbows.added_ms);
        assert_eq!(cards.len(), 2);
    }
}
