//! The library as it is served to other devices (`stashd`'s Subsonic API): songs, albums and artists with what a
//! player on another device shows of them (track numbers, years, how often played, when added, liked), read in the
//! orders such players ask for.

use rusqlite::{OptionalExtension, params};

use crate::Result;
use crate::library::{Library, art};

/// A song as served.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub artist_id: i64,
    pub album: Option<String>,
    pub album_id: Option<i64>,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: Option<i32>,
    pub duration_ms: u64,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    /// The file's size, for a file here.
    pub size: Option<u64>,
    pub play_count: u64,
    pub added_at_s: i64,
    pub liked_at_ms: Option<i64>,
    /// Its picture's address, as the window has it.
    pub art: Option<String>,
    /// The file's name ending, for a file here (".flac" -> "flac").
    pub ending: Option<String>,
}

/// An album as served.
#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub artist_id: i64,
    pub year: Option<i32>,
    pub songs: u32,
    pub duration_ms: u64,
    pub added_at_s: i64,
    pub play_count: u64,
    pub art: Option<String>,
}

/// An artist as served.
#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub albums: u32,
    pub art: Option<String>,
}

/// The orders albums are asked for in (Subsonic's `getAlbumList2` types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumOrder {
    Newest,
    Random,
    ByName,
    ByArtist,
    /// Most played first (only those played).
    Frequent,
    /// Last played first (only those played).
    Recent,
    /// Released from one year to another (backwards when the first is the later).
    Years(i32, i32),
}

const SONG: &str = concat!(
    "SELECT t.id, t.title, a.name, t.artist_id, al.title, t.album_id, t.track_no, t.disc_no, al.year, t.duration_ms,
            t.codec, t.sample_rate, t.bit_depth, CASE WHEN t.origin = 'file' THEN t.file_size END, t.play_count,
            t.added_at, t.liked_at_ms, ",
    art!(),
    ", CASE WHEN t.origin = 'file' THEN t.locator END
     FROM tracks t JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id"
);

fn song(r: &rusqlite::Row<'_>) -> rusqlite::Result<Song> {
    let locator: Option<String> = r.get(18)?;
    Ok(Song {
        id: r.get(0)?,
        title: r.get(1)?,
        artist: r.get(2)?,
        artist_id: r.get(3)?,
        album: r.get(4)?,
        album_id: r.get(5)?,
        track_no: r.get(6)?,
        disc_no: r.get(7)?,
        year: r.get(8)?,
        duration_ms: r.get::<_, i64>(9)?.max(0) as u64,
        codec: r.get(10)?,
        sample_rate: r.get(11)?,
        bit_depth: r.get(12)?,
        size: r.get::<_, Option<i64>>(13)?.map(|s| s.max(0) as u64),
        play_count: r.get::<_, i64>(14)?.max(0) as u64,
        added_at_s: r.get(15)?,
        liked_at_ms: r.get(16)?,
        art: r.get(17)?,
        ending: locator.and_then(|path| {
            std::path::Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
        }),
    })
}

const ALBUM: &str = concat!(
    "SELECT al.id, al.title, a.name, al.artist_id, al.year, COUNT(t.id), SUM(t.duration_ms), MAX(t.added_at),
            SUM(t.play_count),
            (SELECT ",
    art!(),
    " FROM tracks t WHERE t.album_id = al.id AND ",
    art!(),
    " IS NOT NULL ORDER BY t.art_url IS NULL, t.disc_no, t.track_no LIMIT 1)
     FROM albums al JOIN artists a ON a.id = al.artist_id JOIN tracks t ON t.album_id = al.id"
);

fn album(r: &rusqlite::Row<'_>) -> rusqlite::Result<Album> {
    Ok(Album {
        id: r.get(0)?,
        title: r.get(1)?,
        artist: r.get(2)?,
        artist_id: r.get(3)?,
        year: r.get(4)?,
        songs: r.get(5)?,
        duration_ms: r.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64,
        added_at_s: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
        play_count: r.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0) as u64,
        art: r.get(9)?,
    })
}

const ARTIST: &str = concat!(
    "SELECT a.id, a.name, (SELECT COUNT(*) FROM albums al WHERE al.artist_id = a.id),
            (SELECT ",
    art!(),
    " FROM tracks t WHERE t.artist_id = a.id AND ",
    art!(),
    " IS NOT NULL ORDER BY t.art_url IS NULL LIMIT 1)
     FROM artists a"
);

fn artist(r: &rusqlite::Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: r.get(0)?,
        name: r.get(1)?,
        albums: r.get(2)?,
        art: r.get(3)?,
    })
}

impl Library {
    /// The songs `ids`, in their order; one the library no longer has is left out.
    pub fn served_songs(&self, ids: &[i64]) -> Result<Vec<Song>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{SONG} WHERE t.id = ?1"))?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(s) = stmt.query_row([id], song).optional()? {
                out.push(s);
            }
        }
        Ok(out)
    }

    /// Every song, a page at a time, in the order they were added to the library (a device copying the whole list).
    pub fn served_songs_at(&self, offset: usize, limit: usize) -> Result<Vec<Song>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{SONG} ORDER BY t.id LIMIT ?1 OFFSET ?2"))?;
        Ok(stmt
            .query_map(params![limit as i64, offset as i64], song)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `count` songs drawn at random.
    pub fn served_random_songs(&self, count: usize) -> Result<Vec<Song>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{SONG} ORDER BY RANDOM() LIMIT ?1"))?;
        Ok(stmt
            .query_map([count as i64], song)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The liked songs, the last liked first.
    pub fn served_liked(&self, limit: usize) -> Result<Vec<Song>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{SONG} WHERE t.liked_at_ms IS NOT NULL ORDER BY t.liked_at_ms DESC, t.id LIMIT ?1"
        ))?;
        Ok(stmt
            .query_map([limit as i64], song)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// An album's songs, in its order.
    pub fn served_album_songs(&self, album: i64) -> Result<Vec<Song>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{SONG} WHERE t.album_id = ?1 ORDER BY t.disc_no, t.track_no, t.sort_key, t.id"
        ))?;
        Ok(stmt
            .query_map([album], song)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn served_album(&self, id: i64) -> Result<Option<Album>> {
        Ok(self
            .conn
            .prepare_cached(&format!("{ALBUM} WHERE al.id = ?1 GROUP BY al.id"))?
            .query_row([id], album)
            .optional()?)
    }

    /// The albums `ids`, in their order.
    pub fn served_albums_of(&self, ids: &[i64]) -> Result<Vec<Album>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(a) = self.served_album(*id)? {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// A page of albums in `order`.
    pub fn served_albums(
        &self,
        order: AlbumOrder,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Album>> {
        let (filter, sort) = match order {
            AlbumOrder::Newest => ("", "MAX(t.added_at) DESC, al.id DESC".to_string()),
            AlbumOrder::Random => ("", "RANDOM()".to_string()),
            AlbumOrder::ByName => ("", "al.sort_key, al.id".to_string()),
            AlbumOrder::ByArtist => ("", "a.sort_key, al.year, al.sort_key, al.id".to_string()),
            AlbumOrder::Frequent => (
                " HAVING SUM(t.play_count) > 0",
                "SUM(t.play_count) DESC, al.id".to_string(),
            ),
            AlbumOrder::Recent => (
                " HAVING MAX(t.last_played_at) IS NOT NULL",
                "MAX(t.last_played_at) DESC, al.id".to_string(),
            ),
            AlbumOrder::Years(from, to) => {
                let (low, high) = (from.min(to), from.max(to));
                let way = if from > to { "DESC" } else { "ASC" };
                let sql = format!(
                    "{ALBUM} WHERE al.year BETWEEN ?3 AND ?4 GROUP BY al.id ORDER BY al.year {way}, al.sort_key LIMIT ?1 OFFSET ?2"
                );
                let mut stmt = self.conn.prepare_cached(&sql)?;
                return Ok(stmt
                    .query_map(params![limit as i64, offset as i64, low, high], album)?
                    .collect::<rusqlite::Result<Vec<_>>>()?);
            }
        };
        let sql = format!("{ALBUM} GROUP BY al.id{filter} ORDER BY {sort} LIMIT ?1 OFFSET ?2");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(stmt
            .query_map(params![limit as i64, offset as i64], album)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every artist with a song or an album here, in name order.
    pub fn served_artists(&self) -> Result<Vec<Artist>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{ARTIST} WHERE EXISTS (SELECT 1 FROM tracks t WHERE t.artist_id = a.id)
                         OR EXISTS (SELECT 1 FROM albums al WHERE al.artist_id = a.id)
             ORDER BY a.sort_key, a.id"
        ))?;
        Ok(stmt
            .query_map([], artist)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn served_artist(&self, id: i64) -> Result<Option<Artist>> {
        Ok(self
            .conn
            .prepare_cached(&format!("{ARTIST} WHERE a.id = ?1"))?
            .query_row([id], artist)
            .optional()?)
    }

    /// The artists `ids`, in their order.
    pub fn served_artists_of(&self, ids: &[i64]) -> Result<Vec<Artist>> {
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(a) = self.served_artist(*id)? {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// The artist's albums, oldest first.
    pub fn served_artist_albums(&self, id: i64) -> Result<Vec<Album>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{ALBUM} WHERE al.artist_id = ?1 GROUP BY al.id ORDER BY al.year IS NULL, al.year, al.sort_key"
        ))?;
        Ok(stmt
            .query_map([id], album)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A page of albums or artists, in name order (a device copying the whole list).
    pub fn served_albums_at(&self, offset: usize, limit: usize) -> Result<Vec<Album>> {
        self.served_albums(AlbumOrder::ByName, offset, limit)
    }

    pub fn served_artists_at(&self, offset: usize, limit: usize) -> Result<Vec<Artist>> {
        Ok(self
            .served_artists()?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{NewTrack, Origin};

    fn track(title: &str, artist: &str, album: &str, n: u32, path: &str) -> NewTrack {
        NewTrack {
            title: title.into(),
            artist: artist.into(),
            album_artist: None,
            album: Some(album.into()),
            year: Some(2001),
            disc_no: 1,
            track_no: n,
            duration_ms: 1000 * n as u64,
            isrc: None,
            origin: Origin::File,
            addon_id: None,
            locator: path.into(),
            codec: Some("flac".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            added_at: 100 + n as i64,
            file_size: Some(500),
            file_mtime: Some(1),
        }
    }

    #[test]
    fn the_library_is_served_by_album_artist_and_song() {
        let mut library = Library::open_in_memory().unwrap();
        library
            .add_tracks([
                track("Two", "Band", "Record", 2, "C:/m/b/02.FLAC"),
                track("One", "Band", "Record", 1, "C:/m/b/01.flac"),
                track("Solo", "Other", "Alone", 1, "C:/m/o/01.mp3"),
            ])
            .unwrap();
        let albums = library.served_albums(AlbumOrder::ByName, 0, 10).unwrap();
        assert_eq!(
            albums.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            vec!["Alone", "Record"]
        );
        let record = albums.iter().find(|a| a.title == "Record").unwrap();
        assert_eq!(
            (record.songs, record.duration_ms, record.year),
            (2, 3000, Some(2001))
        );

        let songs = library.served_album_songs(record.id).unwrap();
        assert_eq!(
            songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            vec!["One", "Two"]
        );
        assert_eq!(songs[1].ending.as_deref(), Some("flac"));
        assert_eq!(
            (songs[0].size, songs[0].track_no, songs[0].album_id),
            (Some(500), 1, Some(record.id))
        );

        let artists = library.served_artists().unwrap();
        assert_eq!(
            artists
                .iter()
                .map(|a| (a.name.as_str(), a.albums))
                .collect::<Vec<_>>(),
            vec![("Band", 1), ("Other", 1)]
        );
        assert_eq!(
            library.served_artist_albums(artists[0].id).unwrap().len(),
            1
        );

        assert_eq!(library.served_songs_at(1, 10).unwrap().len(), 2);
        assert!(
            library
                .served_albums(AlbumOrder::Frequent, 0, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            library
                .served_albums(AlbumOrder::Years(2005, 2000), 0, 10)
                .unwrap()
                .len(),
            2
        );
        assert!(
            library
                .served_albums(AlbumOrder::Years(1990, 1995), 0, 10)
                .unwrap()
                .is_empty()
        );
        library.set_liked(songs[0].id, true, 5).unwrap();
        assert_eq!(library.served_liked(10).unwrap()[0].id, songs[0].id);
        assert_eq!(library.served_random_songs(2).unwrap().len(), 2);
        let ids = [songs[1].id, 999, songs[0].id];
        assert_eq!(
            library
                .served_songs(&ids)
                .unwrap()
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![songs[1].id, songs[0].id]
        );
    }
}
