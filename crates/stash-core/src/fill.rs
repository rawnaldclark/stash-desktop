//! Filling in missing tags: a file that says only its title and artist is looked up in a public music catalogue (the
//! iTunes Search API, which needs no account), and what the catalogue knows of it (album, year, track number, genre)
//! is offered for the blanks. Nothing a file already says is ever changed, and nothing is written here: the caller
//! shows what was found and writes what the listener accepts (see [`crate::edit`]).
//!
//! The lookup sends the song's title and artist to the catalogue, so it only ever runs when the listener asks for it.

use std::time::Duration;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::edit::TagEdit;
use crate::identity::fold;
use crate::library::Library;
use crate::lyrics::Unreachable;
use crate::matching::{artists, same_title};

/// The catalogue's address.
pub const CATALOGUE: &str = "https://itunes.apple.com";
/// How far a catalogue recording's length may be from the file's and still be the same recording.
const LENGTH_SLACK_MS: u64 = 4_000;

/// A file with blanks, as it is looked up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FillSong {
    pub id: i64,
    pub title: String,
    pub artist: String,
    #[serde(skip)]
    pub duration_ms: u64,
}

/// What the catalogue says of a recording.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Found {
    pub album: String,
    pub album_artist: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genre: String,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    results: Vec<Hit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    collection_name: String,
    #[serde(default)]
    collection_artist_name: String,
    #[serde(default)]
    release_date: String,
    #[serde(default)]
    track_number: u32,
    #[serde(default)]
    disc_number: u32,
    #[serde(default)]
    primary_genre_name: String,
    #[serde(default)]
    track_time_millis: u64,
}

/// An album's name without the catalogue's own label for a single or an EP.
fn album_name(name: &str) -> String {
    let name = name.trim();
    for label in [" - Single", " - EP"] {
        if let Some(cut) = name.strip_suffix(label) {
            return cut.trim().to_string();
        }
    }
    name.to_string()
}

/// Whether the catalogue's credit names the artist the file names (either may list more: "A, B & C" has "A").
fn same_artist(wanted: &str, credit: &str) -> bool {
    let (w, c) = (fold(wanted), fold(credit));
    if w.is_empty() || c.is_empty() {
        return false;
    }
    if w == c {
        return true;
    }
    let (wanted, credit) = (artists(wanted), artists(credit));
    wanted.iter().any(|a| credit.contains(a))
}

/// The catalogue recording that is this song, from the catalogue's answer to a search: the same title and artist, and
/// the same length when the file's is known. Of several, the one closest in length, and then the earliest released
/// (the album a song first came out on, not a later collection of hits).
pub fn pick(song: &FillSong, body: &str) -> Option<Found> {
    let answer: Answer = serde_json::from_str(body).ok()?;
    let mut best: Option<(u64, bool, &str, &Hit)> = None;
    for hit in &answer.results {
        if (!hit.kind.is_empty() && hit.kind != "song") || !same_title(&song.title, &hit.track_name) || !same_artist(&song.artist, &hit.artist_name) {
            continue;
        }
        let off = match (song.duration_ms, hit.track_time_millis) {
            (0, _) | (_, 0) => LENGTH_SLACK_MS,
            (a, b) => a.abs_diff(b),
        };
        if off > LENGTH_SLACK_MS {
            continue;
        }
        // Lengths within a second are the same; a collection by various artists comes after the artist's own record.
        let various = fold(&hit.collection_artist_name) == "various artists";
        let key = (off / 1_000, various, hit.release_date.as_str());
        if best.as_ref().is_none_or(|(o, v, d, _)| key < (*o, *v, *d)) {
            best = Some((key.0, key.1, key.2, hit));
        }
    }
    let (.., hit) = best?;
    let number = |n: u32| Some(n).filter(|n| (1..=9_999).contains(n));
    Some(Found {
        album: album_name(&hit.collection_name),
        album_artist: if hit.collection_artist_name.trim().is_empty() { hit.artist_name.trim().to_string() } else { hit.collection_artist_name.trim().to_string() },
        year: hit.release_date.get(..4).and_then(|y| y.parse().ok()).filter(|y| (1000..=9999).contains(y)),
        track_no: number(hit.track_number),
        disc_no: number(hit.disc_number),
        genre: hit.primary_genre_name.trim().to_string(),
    })
}

/// One question to the catalogue at `base`: its answer as it came.
fn ask(base: &str, pairs: &[(&str, &str)]) -> std::result::Result<String, Unreachable> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(concat!("Stash desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut url = url::Url::parse(&format!("{base}/search")).map_err(|e| Unreachable(e.to_string()))?;
    url.query_pairs_mut().extend_pairs(pairs).append_pair("entity", "song");
    let mut answer = agent.get(url.as_str()).call().map_err(|e| Unreachable(e.to_string()))?;
    match answer.status().as_u16() {
        200..=299 => answer
            .body_mut()
            .with_config()
            .limit(16 << 20)
            .read_to_string()
            .map_err(|e| Unreachable(e.to_string())),
        status => Err(Unreachable(format!("the catalogue answered {status}"))),
    }
}

/// The artist a song is looked up under: the first its credit names.
pub fn first_artist(song: &FillSong) -> String {
    artists(&song.artist).into_iter().next().unwrap_or_else(|| fold(&song.artist))
}

/// Asks the catalogue at `base` for this song. `Ok(None)`: the catalogue has no recording that is it.
pub fn look_up(base: &str, song: &FillSong) -> std::result::Result<Option<Found>, Unreachable> {
    // The first artist and the title without a bracketed part find more than the whole credit does.
    let term = format!("{} {}", first_artist(song), fold(&song.title));
    Ok(pick(song, &ask(base, &[("term", &term), ("limit", "50")])?))
}

/// The catalogue's songs by one artist (the two hundred it puts first), as its answer came: several of an artist's
/// files are then found in it with [`pick`], for one question in place of one each.
pub fn artist_songs(base: &str, artist: &str) -> std::result::Result<String, Unreachable> {
    ask(base, &[("term", artist), ("attribute", "artistTerm"), ("limit", "200")])
}

impl Library {
    /// The files that are here and have a blank the catalogue could fill: no album, no genre, no year or no track
    /// number. In artist and title order.
    pub fn files_with_blanks(&self) -> Result<Vec<FillSong>> {
        let mut read = self.conn.prepare(
            "SELECT t.id, t.title, a.name, t.duration_ms
             FROM tracks t JOIN artists a ON a.id = t.artist_id LEFT JOIN albums al ON al.id = t.album_id
             WHERE t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL
               AND (t.album_id IS NULL OR COALESCE(al.title, '') = '' OR COALESCE(t.genre, '') = ''
                    OR COALESCE(t.year, al.year) IS NULL OR t.track_no <= 0)
             ORDER BY a.name COLLATE NOCASE, t.title COLLATE NOCASE, t.id",
        )?;
        let rows = read
            .query_map([], |r| Ok(FillSong { id: r.get(0)?, title: r.get(1)?, artist: r.get(2)?, duration_ms: r.get::<_, i64>(3)?.max(0) as u64 }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// What `found` would add to the song `id`: only its blanks. The album (with its artist) when the file names none;
    /// the genre when it has none; the year, track and disc only when the album is the catalogue's too, as those say
    /// where the song sits on that album and nothing about another. `None` when there is nothing to add, or the song
    /// is no file here.
    pub fn fill_edit(&self, id: i64, found: &Found) -> Result<Option<TagEdit>> {
        let now = self
            .conn
            .query_row(
                "SELECT COALESCE(al.title, ''), COALESCE(aa.name, ''), COALESCE(t.genre, ''), COALESCE(t.year, al.year), t.track_no
                 FROM tracks t LEFT JOIN albums al ON al.id = t.album_id LEFT JOIN artists aa ON aa.id = al.artist_id
                 WHERE t.id = ?1 AND t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, i64>(4)?)),
            )
            .optional()?;
        let Some((album, album_artist, genre, year, track_no)) = now else {
            return Ok(None);
        };
        let mut edit = TagEdit::default();
        let no_album = album.trim().is_empty();
        if no_album && !found.album.is_empty() {
            edit.album = Some(found.album.clone());
            if album_artist.trim().is_empty() && !found.album_artist.is_empty() {
                edit.album_artist = Some(found.album_artist.clone());
            }
        }
        if genre.trim().is_empty() && !found.genre.is_empty() {
            edit.genre = Some(found.genre.clone());
        }
        let same_album = !found.album.is_empty() && (no_album || fold(&album) == fold(&found.album) || fold(&album_name(&album)) == fold(&found.album));
        if same_album {
            if year.is_none() {
                edit.year = found.year.map(|y| y.to_string());
            }
            if track_no <= 0 {
                edit.track_no = found.track_no.map(|n| n.to_string());
                // A disc number is only worth writing with the track number it belongs to, and only past disc one.
                edit.disc_no = found.disc_no.filter(|d| *d > 1 && edit.track_no.is_some()).map(|n| n.to_string());
            }
        }
        Ok(Some(edit).filter(|e| !e.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, artist: &str, ms: u64) -> FillSong {
        FillSong { id: 1, title: title.into(), artist: artist.into(), duration_ms: ms }
    }

    const ANSWER: &str = r#"{"resultCount":5,"results":[
      {"wrapperType":"track","kind":"song","trackName":"Nude","artistName":"Radiohead","collectionName":"The Best Of","collectionArtistName":"","releaseDate":"2008-06-02T07:00:00Z","trackNumber":9,"discNumber":2,"primaryGenreName":"Alternative","trackTimeMillis":255000},
      {"wrapperType":"track","kind":"song","trackName":"Nude","artistName":"Radiohead","collectionName":"In Rainbows","releaseDate":"2007-10-10T07:00:00Z","trackNumber":3,"discNumber":1,"primaryGenreName":"Alternative","trackTimeMillis":255400},
      {"wrapperType":"track","kind":"song","trackName":"Nude (Live)","artistName":"Radiohead","collectionName":"Live Somewhere","releaseDate":"2001-01-01T07:00:00Z","trackNumber":1,"discNumber":1,"primaryGenreName":"Alternative","trackTimeMillis":255000},
      {"wrapperType":"track","kind":"song","trackName":"Nude","artistName":"Someone Else","collectionName":"Covers","releaseDate":"1999-01-01T07:00:00Z","trackNumber":1,"discNumber":1,"primaryGenreName":"Pop","trackTimeMillis":255000},
      {"wrapperType":"track","kind":"song","trackName":"Nude","artistName":"Radiohead","collectionName":"Hits of the 2000s","collectionArtistName":"Various Artists","releaseDate":"2005-01-01T07:00:00Z","trackNumber":14,"discNumber":1,"primaryGenreName":"Rock","trackTimeMillis":255200},
      {"wrapperType":"track","kind":"song","trackName":"Nude","artistName":"Radiohead","collectionName":"Radio Edits - Single","releaseDate":"2006-01-01T07:00:00Z","trackNumber":1,"discNumber":1,"primaryGenreName":"Alternative","trackTimeMillis":199000}
    ]}"#;

    #[test]
    fn the_recording_picked_is_the_same_song_on_the_record_it_first_came_out_on() {
        let found = pick(&song("Nude", "Radiohead", 255_300), ANSWER).unwrap();
        // Not the live one, the cover, the shorter edit or the various-artists collection; and of the artist's own two
        // records, the earlier.
        assert_eq!(found, Found { album: "In Rainbows".into(), album_artist: "Radiohead".into(), year: Some(2007), track_no: Some(3), disc_no: Some(1), genre: "Alternative".into() });
        // A file whose length is not known cannot tell the recordings apart by length: the artist's earliest it is.
        assert_eq!(pick(&song("Nude", "Radiohead", 0), ANSWER).unwrap().album, "Radio Edits");
        // A song the catalogue does not have is not guessed at.
        assert_eq!(pick(&song("Nude", "Radiohead", 400_000), ANSWER), None);
        assert_eq!(pick(&song("Reckoner", "Radiohead", 255_000), ANSWER), None);
        assert_eq!(pick(&song("Nude", "Radiohead", 255_000), "not json"), None);
    }

    #[test]
    fn a_single_loses_the_catalogues_label_and_a_credit_of_several_matches_one() {
        let body = r#"{"results":[{"kind":"song","trackName":"Song (feat. B)","artistName":"A & B","collectionName":"Song (feat. B) - Single","releaseDate":"2020-05-01T07:00:00Z","trackNumber":1,"discNumber":1,"primaryGenreName":"Hip-Hop/Rap","trackTimeMillis":200000}]}"#;
        let found = pick(&song("Song", "A", 201_000), body).unwrap();
        assert_eq!((found.album.as_str(), found.genre.as_str(), found.year), ("Song (feat. B)", "Hip-Hop/Rap", Some(2020)));
        assert_eq!(album_name("Kid A - EP"), "Kid A");
    }

    /// Asks the real catalogue: `cargo test -p stash-core --lib fill::tests::live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        for (title, artist, ms) in [("Nude", "Radiohead", 255_000), ("So What", "Miles Davis", 562_000), ("HUMBLE.", "Kendrick Lamar", 177_000), ("Dreams - 2004 Remaster", "Fleetwood Mac", 257_000), ("No Such Song Zzqx", "Radiohead", 100_000)] {
            println!("{title} / {artist}: {:?}", look_up(CATALOGUE, &song(title, artist, ms)).map_err(|e| e.0));
            std::thread::sleep(Duration::from_secs(3));
        }
    }

    #[test]
    fn only_blanks_are_filled_and_track_numbers_only_for_the_same_album() {
        let mut library = Library::open_in_memory().unwrap();
        let add = |library: &mut Library, title: &str, album: &str| {
            let id = library.adopt(&crate::import::Song { title: title.into(), artist: "Radiohead".into(), album: Some(album.to_string()).filter(|a| !a.is_empty()), ..Default::default() }).unwrap();
            library.conn.execute("UPDATE tracks SET origin = 'file', locator = ?2 WHERE id = ?1", rusqlite::params![id, format!("C:/m/{title}.flac")]).unwrap();
            id
        };
        let bare = add(&mut library, "Nude", "");
        let other = add(&mut library, "Reckoner", "The Best Of");
        let found = Found { album: "In Rainbows".into(), album_artist: "Radiohead".into(), year: Some(2007), track_no: Some(3), disc_no: Some(1), genre: "Alternative".into() };

        // A file with nothing takes it all (disc one is not worth writing).
        let edit = library.fill_edit(bare, &found).unwrap().unwrap();
        assert_eq!(edit, TagEdit { album: Some("In Rainbows".into()), album_artist: Some("Radiohead".into()), genre: Some("Alternative".into()), year: Some("2007".into()), track_no: Some("3".into()), ..Default::default() });
        assert_eq!((edit.title, edit.artist), (None, None));

        // A file that names another album keeps it, and takes only the genre: the catalogue's track number and year
        // are that other record's.
        let edit = library.fill_edit(other, &found).unwrap().unwrap();
        assert_eq!(edit, TagEdit { genre: Some("Alternative".into()), ..Default::default() });

        // Nothing to add is nothing; and both are listed as having blanks.
        library.conn.execute("UPDATE tracks SET genre = 'Rock' WHERE id = ?1", [other]).unwrap();
        assert_eq!(library.fill_edit(other, &found).unwrap(), None);
        assert_eq!(library.fill_edit(9_999, &found).unwrap(), None);
        let blanks: Vec<_> = library.files_with_blanks().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(blanks, vec![bare, other]);
    }
}
