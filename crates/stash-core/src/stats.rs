//! The library in numbers: what the files on this computer are (formats, quality, decades, genres, artists), how
//! complete their tags are, and what has been listened to (most played, plays a month).

use serde::Serialize;

use crate::Result;
use crate::library::Library;

/// A song that is a file here.
const HERE: &str = "t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL";
/// Codecs that lose nothing.
const LOSSLESS: &str = "LOWER(COALESCE(t.codec, '')) IN ('flac', 'alac', 'wav', 'pcm', 'aiff', 'ape', 'wavpack', 'wv', 'tta')";
/// Lossless and past CD quality.
const HI_RES: &str = "(COALESCE(t.bit_depth, 0) > 16 OR COALESCE(t.sample_rate, 0) > 48000)";

/// One row of a ranked or ordered list: what it is, how many, and (for formats) how much room it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bar {
    pub label: String,
    pub value: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// What the row opens, when it opens something: a genre, an artist's id, a decade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A second line: a song's artist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// How many of the files have a tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Share {
    pub label: String,
    pub have: u64,
    pub of: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub songs: u64,
    pub albums: u64,
    pub artists: u64,
    pub genres: u64,
    pub duration_ms: u64,
    pub bytes: u64,
    pub lossless: u64,
    pub hi_res: u64,
    /// Every play Stash has counted, of any song.
    pub plays: u64,
    /// Listening time those plays add up to.
    pub played_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryStats {
    pub totals: Totals,
    pub formats: Vec<Bar>,
    /// Hi-res lossless, CD-quality lossless, lossy: in that order.
    pub quality: Vec<Bar>,
    /// Oldest first; songs with no year last.
    pub decades: Vec<Bar>,
    /// The most common first; the rest folded into "Other genres".
    pub genres: Vec<Bar>,
    /// Files with no genre at all (not a genre, so not in [`Self::genres`]).
    pub no_genre: u64,
    pub artists_by_songs: Vec<Bar>,
    pub artists_by_plays: Vec<Bar>,
    pub most_played: Vec<Bar>,
    /// The last twelve months, oldest first, as "2026-11".
    pub months: Vec<Bar>,
    pub tags: Vec<Share>,
}

/// A codec as people name it.
fn format_name(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "" => "Unknown".into(),
        "flac" => "FLAC".into(),
        "alac" => "ALAC".into(),
        "aac" => "AAC".into(),
        "mp3" => "MP3".into(),
        "opus" => "Opus".into(),
        "vorbis" => "Ogg Vorbis".into(),
        "wav" | "pcm" => "WAV".into(),
        "aiff" => "AIFF".into(),
        other => other.to_uppercase(),
    }
}

impl Library {
    /// The library in numbers. Lists are cut at `top` rows.
    pub fn library_stats(&self, top: usize) -> Result<LibraryStats> {
        let n = |v: i64| v.max(0) as u64;
        let mut totals = self.conn.query_row(
            &format!(
                "SELECT COUNT(*), COUNT(DISTINCT t.album_id), COUNT(DISTINCT t.artist_id), COUNT(DISTINCT NULLIF(t.genre, '')),
                        COALESCE(SUM(t.duration_ms), 0), COALESCE(SUM(t.file_size), 0),
                        COALESCE(SUM({LOSSLESS}), 0), COALESCE(SUM({LOSSLESS} AND {HI_RES}), 0)
                 FROM tracks t WHERE {HERE}"
            ),
            [],
            |r| {
                Ok(Totals {
                    songs: n(r.get(0)?),
                    albums: n(r.get(1)?),
                    artists: n(r.get(2)?),
                    genres: n(r.get(3)?),
                    duration_ms: n(r.get(4)?),
                    bytes: n(r.get(5)?),
                    lossless: n(r.get(6)?),
                    hi_res: n(r.get(7)?),
                    ..Default::default()
                })
            },
        )?;
        (totals.plays, totals.played_ms) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(t.duration_ms), 0) FROM plays p JOIN tracks t ON t.id = p.track_id",
            [],
            |r| Ok((n(r.get(0)?), n(r.get(1)?))),
        )?;

        let bars = |sql: &str, top: usize| -> Result<Vec<Bar>> {
            let rows = self
                .conn
                .prepare(sql)?
                .query_map([top as i64], |r| {
                    let label: String = r.get(0)?;
                    // A song's row is its title and artist joined by a unit separator: the artist goes on its own line.
                    let (label, detail) = match label.split_once('\u{1f}') {
                        Some((title, artist)) => (title.to_string(), Some(artist.to_string())),
                        None => (label, None),
                    };
                    Ok(Bar { label, value: n(r.get(1)?), bytes: r.get::<_, Option<i64>>(2)?.map(n), key: r.get(3)?, detail })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        };

        let mut formats = bars(
            &format!(
                "SELECT LOWER(COALESCE(t.codec, '')) AS c, COUNT(*) AS n, COALESCE(SUM(t.file_size), 0), NULL
                 FROM tracks t WHERE {HERE} GROUP BY c ORDER BY n DESC, c LIMIT ?1"
            ),
            top,
        )?;
        for f in &mut formats {
            f.label = format_name(&f.label);
        }

        let quality = {
            let (hi, cd, lossy) = self.conn.query_row(
                &format!(
                    "SELECT COALESCE(SUM({LOSSLESS} AND {HI_RES}), 0), COALESCE(SUM({LOSSLESS} AND NOT {HI_RES}), 0),
                            COALESCE(SUM(NOT {LOSSLESS}), 0)
                     FROM tracks t WHERE {HERE}"
                ),
                [],
                |r| Ok((n(r.get(0)?), n(r.get(1)?), n(r.get(2)?))),
            )?;
            vec![
                Bar { label: "Hi-res lossless".into(), value: hi, bytes: None, key: None, detail: None },
                Bar { label: "CD-quality lossless".into(), value: cd, bytes: None, key: None, detail: None },
                Bar { label: "Lossy".into(), value: lossy, bytes: None, key: None, detail: None },
            ]
        };

        let decades = bars(
            &format!(
                "SELECT CASE WHEN d IS NULL THEN 'No year' ELSE d || 's' END, n, NULL, CAST(d AS TEXT)
                 FROM (SELECT COALESCE(t.year, al.year) / 10 * 10 AS d, COUNT(*) AS n
                       FROM tracks t LEFT JOIN albums al ON al.id = t.album_id WHERE {HERE} GROUP BY d)
                 ORDER BY d IS NULL, d LIMIT ?1"
            ),
            100,
        )?;

        let mut genres = bars(
            &format!(
                "SELECT t.genre, COUNT(*) AS n, NULL, t.genre FROM tracks t
                 WHERE {HERE} AND COALESCE(t.genre, '') != '' GROUP BY t.genre ORDER BY n DESC, t.genre COLLATE NOCASE LIMIT ?1"
            ),
            top,
        )?;
        let (tagged, untagged): (i64, i64) = self.conn.query_row(
            &format!("SELECT COALESCE(SUM(COALESCE(t.genre, '') != ''), 0), COALESCE(SUM(COALESCE(t.genre, '') = ''), 0) FROM tracks t WHERE {HERE}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let shown: u64 = genres.iter().map(|g| g.value).sum();
        if n(tagged) > shown {
            genres.push(Bar { label: "Other genres".into(), value: n(tagged) - shown, bytes: None, key: None, detail: None });
        }

        let artists_by_songs = bars(
            &format!(
                "SELECT a.name, COUNT(*) AS n, NULL, CAST(a.id AS TEXT) FROM tracks t JOIN artists a ON a.id = t.artist_id
                 WHERE {HERE} GROUP BY a.id ORDER BY n DESC, a.sort_key LIMIT ?1"
            ),
            top,
        )?;
        let artists_by_plays = bars(
            "SELECT a.name, COUNT(*) AS n, NULL, CAST(a.id AS TEXT) FROM plays p JOIN tracks t ON t.id = p.track_id
             JOIN artists a ON a.id = t.artist_id GROUP BY a.id ORDER BY n DESC, a.sort_key LIMIT ?1",
            top,
        )?;
        let most_played = bars(
            "SELECT t.title || char(31) || a.name, COUNT(*) AS n, NULL, CAST(t.id AS TEXT) FROM plays p JOIN tracks t ON t.id = p.track_id
             JOIN artists a ON a.id = t.artist_id GROUP BY t.id ORDER BY n DESC, t.sort_key LIMIT ?1",
            top,
        )?;
        let months = bars(
            "WITH RECURSIVE m(i, month) AS (
                 SELECT 0, strftime('%Y-%m', 'now', 'localtime', 'start of month')
                 UNION ALL SELECT i + 1, strftime('%Y-%m', 'now', 'localtime', 'start of month', '-' || (i + 1) || ' months') FROM m WHERE i < 11)
             SELECT m.month, (SELECT COUNT(*) FROM plays p
                              WHERE strftime('%Y-%m', p.played_at_ms / 1000, 'unixepoch', 'localtime') = m.month), NULL, NULL
             FROM m WHERE ?1 > 0 ORDER BY m.month",
            1,
        )?;

        let tags = {
            let (of, album, genre, year, track, cover) = self.conn.query_row(
                &format!(
                    "SELECT COUNT(*), COALESCE(SUM(COALESCE(al.title, '') != ''), 0), COALESCE(SUM(COALESCE(t.genre, '') != ''), 0),
                            COALESCE(SUM(COALESCE(t.year, al.year) IS NOT NULL), 0), COALESCE(SUM(t.track_no > 0), 0),
                            COALESCE(SUM(t.cover = 1), 0)
                     FROM tracks t LEFT JOIN albums al ON al.id = t.album_id WHERE {HERE}"
                ),
                [],
                |r| Ok((n(r.get(0)?), n(r.get(1)?), n(r.get(2)?), n(r.get(3)?), n(r.get(4)?), n(r.get(5)?))),
            )?;
            [("Album", album), ("Genre", genre), ("Year", year), ("Track number", track), ("Cover in the file", cover)]
                .into_iter()
                .map(|(label, have)| Share { label: label.into(), have, of })
                .collect()
        };

        Ok(LibraryStats { totals, formats, quality, decades, genres, no_genre: n(untagged), artists_by_songs, artists_by_plays, most_played, months, tags })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Song;
    use rusqlite::params;

    fn file(library: &mut Library, title: &str, artist: &str, codec: &str, rate: u32, depth: Option<u32>, genre: Option<&str>, year: Option<i32>) -> i64 {
        let id = library.adopt(&Song { title: title.into(), artist: artist.into(), duration_ms: Some(100_000), ..Default::default() }).unwrap();
        library
            .conn
            .execute(
                "UPDATE tracks SET origin = 'file', locator = ?2, file_size = 1000, codec = ?3, sample_rate = ?4, bit_depth = ?5,
                                   genre = ?6, year = ?7, track_no = 1 WHERE id = ?1",
                params![id, format!("C:/m/{title}"), codec, rate, depth, genre, year],
            )
            .unwrap();
        id
    }

    #[test]
    fn the_files_are_counted_by_format_quality_decade_genre_and_artist() {
        let mut library = Library::open_in_memory().unwrap();
        let a = file(&mut library, "A", "Radiohead", "flac", 96_000, Some(24), Some("Alternative"), Some(2007));
        let b = file(&mut library, "B", "Radiohead", "flac", 44_100, Some(16), Some("Alternative"), Some(1997));
        file(&mut library, "C", "Miles Davis", "aac", 44_100, None, Some("Jazz"), Some(1959));
        file(&mut library, "D", "Cher", "opus", 48_000, None, None, None);
        // Only an entry, no file: not counted.
        library.adopt(&Song { title: "E".into(), artist: "Kendrick Lamar".into(), ..Default::default() }).unwrap();
        for at in [1_000, 2_000, 3_000] {
            library.conn.execute("INSERT INTO plays(track_id, played_at_ms) VALUES (?1, ?2)", params![a, at]).unwrap();
        }
        library.conn.execute("INSERT INTO plays(track_id, played_at_ms) VALUES (?1, 9)", [b]).unwrap();

        let s = library.library_stats(10).unwrap();
        assert_eq!((s.totals.songs, s.totals.artists, s.totals.genres, s.totals.bytes), (4, 3, 2, 4000));
        assert_eq!((s.totals.lossless, s.totals.hi_res, s.totals.plays, s.totals.played_ms), (2, 1, 4, 400_000));
        let pairs = |bars: &[Bar]| bars.iter().map(|b| (b.label.clone(), b.value)).collect::<Vec<_>>();
        assert_eq!(pairs(&s.formats), vec![("FLAC".into(), 2), ("AAC".into(), 1), ("Opus".into(), 1)]);
        assert_eq!(s.formats[0].bytes, Some(2000));
        assert_eq!(pairs(&s.quality), vec![("Hi-res lossless".into(), 1), ("CD-quality lossless".into(), 1), ("Lossy".into(), 2)]);
        assert_eq!(pairs(&s.decades), vec![("1950s".into(), 1), ("1990s".into(), 1), ("2000s".into(), 1), ("No year".into(), 1)]);
        assert_eq!(s.decades[0].key.as_deref(), Some("1950"));
        assert_eq!(pairs(&s.genres), vec![("Alternative".into(), 2), ("Jazz".into(), 1)]);
        assert_eq!(s.no_genre, 1);
        assert_eq!(pairs(&s.artists_by_songs)[0], ("Radiohead".into(), 2));
        assert_eq!(pairs(&s.artists_by_plays), vec![("Radiohead".into(), 4)]);
        assert_eq!(pairs(&s.most_played), vec![("A".into(), 3), ("B".into(), 1)]);
        assert_eq!(s.most_played[0].detail.as_deref(), Some("Radiohead"));
        assert_eq!(s.months.len(), 12);
        assert!(s.months.windows(2).all(|w| w[0].label < w[1].label));
        let tags: Vec<_> = s.tags.iter().map(|t| (t.label.as_str(), t.have, t.of)).collect();
        assert_eq!(tags[1], ("Genre", 3, 4));
        assert_eq!(tags[2], ("Year", 3, 4));

        // With a top of one, the rest of the genres fold into "Other genres".
        let s = library.library_stats(1).unwrap();
        assert_eq!(pairs(&s.genres), vec![("Alternative".into(), 2), ("Other genres".into(), 1)]);
    }

    #[test]
    fn an_empty_library_has_numbers_of_nothing() {
        let library = Library::open_in_memory().unwrap();
        let s = library.library_stats(10).unwrap();
        assert_eq!(s.totals, Totals::default());
        assert!(s.formats.is_empty() && s.genres.is_empty() && s.most_played.is_empty());
        assert_eq!(s.months.len(), 12);
        assert!(s.tags.iter().all(|t| t.of == 0));
    }
}
