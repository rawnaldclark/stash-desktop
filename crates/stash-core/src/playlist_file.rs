//! Playlist files (.m3u, .m3u8): another player's playlist comes in as a playlist here, and a playlist goes out as a
//! file other players read.
//!
//! Coming in, each entry becomes, in this order: a file the library has (by its full path; a path written relative to
//! the playlist file is found beside it); a file on this computer the library does not have yet (its tags are read and
//! it is added, as a scan would); else the song the entry names (its `#EXTINF` "Artist - Title", or a file name of
//! that shape), found among the library's songs or kept as one the add-ons play. Web addresses (radio streams) are
//! left out, and so is an entry that names no song.
//!
//! Going out: each song that is a file on this computer (its own, or saved from an add-on), by its full path, with its
//! length, artist and title. The others are left out: no other player could play them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::Result;
use crate::import::Song;
use crate::library::Library;

/// The most entries a playlist file brings in.
pub const ENTRIES_MOST: usize = 10_000;

/// One entry of a playlist file: where it is, and what its `#EXTINF` line said of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub location: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_ms: Option<u64>,
}

/// A playlist file as written: its own name for the playlist (`#PLAYLIST`), and its entries in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub name: Option<String>,
    pub entries: Vec<Entry>,
}

/// What a playlist file brought in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Brought {
    /// The new playlist.
    pub id: i64,
    pub name: String,
    /// Its songs (an entry named twice is in it once).
    pub songs: usize,
    /// Entries that were files the library has, files it did not have (added now), and songs found by their words.
    pub files: usize,
    pub added: usize,
    pub by_words: usize,
    /// Entries left out: web addresses, and entries that name no song.
    pub left_out: usize,
}

/// Reads a playlist file's text: UTF-8 (an .m3u8, or an .m3u written so), else the Windows code page older players
/// write .m3u files in.
pub fn parse(bytes: &[u8]) -> Parsed {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    };
    let mut parsed = Parsed::default();
    let mut said: Option<Entry> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(info) = strip_prefix_ignore_case(rest, "EXTINF:") {
                said = Some(extinf(info));
            } else if let Some(name) = strip_prefix_ignore_case(rest, "PLAYLIST:") {
                let name = name.trim();
                if !name.is_empty() {
                    parsed.name = Some(name.to_string());
                }
            }
            continue;
        }
        if parsed.entries.len() == ENTRIES_MOST {
            break;
        }
        let mut entry = said.take().unwrap_or_default();
        entry.location = line.to_string();
        parsed.entries.push(entry);
    }
    parsed
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// `#EXTINF:<seconds>[ attributes],<Artist - Title>`.
fn extinf(info: &str) -> Entry {
    // The display text follows the first comma that is not inside an attribute's quotes.
    let mut quoted = false;
    let mut comma = None;
    for (i, c) in info.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                comma = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (head, display) = match comma {
        Some(i) => (&info[..i], info[i + 1..].trim()),
        None => (info, ""),
    };
    let seconds = head
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0);
    let (artist, title) = artist_and_title(display);
    Entry {
        location: String::new(),
        title,
        artist,
        duration_ms: seconds.map(|s| (s * 1000.0).round() as u64),
    }
}

/// "Artist - Title" (a leading track number, "01 - ", is passed over); with no " - ", only a title.
fn artist_and_title(text: &str) -> (Option<String>, Option<String>) {
    let mut text = text.trim();
    if let Some((number, rest)) = text.split_once(" - ")
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
    {
        text = rest.trim();
    }
    let some = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    match text.split_once(" - ") {
        Some((artist, title)) => (some(artist), some(title)),
        None => (None, some(text)),
    }
}

/// A byte of the Windows code page 1252 as a character (its 0x80–0x9F differ from Latin-1).
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// Where an entry is on this computer: its path made whole (a relative one beside the playlist file), or `None` for
/// a web address.
fn place_of(location: &str, beside: &Path) -> Option<PathBuf> {
    let path = match location.strip_prefix("file://") {
        Some(rest) => {
            let decoded = percent_decode(rest);
            // file:///C:/Music/a.flac names C:/Music/a.flac; file:///home/a.flac names /home/a.flac.
            let bytes = decoded.as_bytes();
            if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
                decoded[1..].to_string()
            } else {
                decoded
            }
        }
        None if location.contains("://") => return None,
        None => location.to_string(),
    };
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        beside.join(path)
    };
    // As a scan writes it: whole, with this computer's separators.
    std::path::absolute(&path).ok()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Library {
    /// The library's file at `locator`, as written or (on a computer whose file names do not care for case) in any
    /// case.
    fn file_track(&self, locator: &str) -> Result<Option<i64>> {
        let exact = self
            .conn
            .prepare_cached("SELECT id FROM tracks WHERE locator = ?1 AND origin = 'file' LIMIT 1")?
            .query_row([locator], |r| r.get(0))
            .optional()?;
        if exact.is_some() || !cfg!(windows) {
            return Ok(exact);
        }
        Ok(self
            .conn
            .prepare_cached(
                "SELECT id FROM tracks WHERE lower(locator) = lower(?1) AND origin = 'file' LIMIT 1",
            )?
            .query_row([locator], |r| r.get(0))
            .optional()?)
    }

    /// Makes a playlist of a playlist file's entries (see the module's words), named `name`. `beside` is the folder
    /// the file is in.
    pub fn playlist_from_file(
        &mut self,
        parsed: &Parsed,
        name: &str,
        beside: &Path,
        now_ms: i64,
    ) -> Result<Brought> {
        let now_s = now_ms / 1000;
        let mut ids: Vec<Option<i64>> = vec![None; parsed.entries.len()];
        let mut by_words: Vec<(usize, Song)> = Vec::new();
        let (mut files, mut added, mut left_out) = (0, 0, 0);
        for (n, entry) in parsed.entries.iter().enumerate() {
            let place = place_of(&entry.location, beside);
            if let Some(path) = &place {
                let locator = path.to_string_lossy().into_owned();
                if let Some(id) = self.file_track(&locator)? {
                    ids[n] = Some(id);
                    files += 1;
                    continue;
                }
                if crate::scan::is_audio(path)
                    && let Ok(meta) = std::fs::metadata(path)
                    && meta.is_file()
                {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs() as i64);
                    if let Ok(track) =
                        crate::scan::read_track(path, &locator, meta.len(), mtime, now_s)
                    {
                        self.add_tracks([track])?;
                        if let Some(id) = self.file_track(&locator)? {
                            ids[n] = Some(id);
                            added += 1;
                            continue;
                        }
                    }
                }
            } else if entry.location.contains("://") {
                left_out += 1;
                continue;
            }
            // The song it names: its #EXTINF words, else its file's name.
            let (artist, title) = match (&entry.artist, &entry.title) {
                (Some(artist), Some(title)) => (Some(artist.clone()), Some(title.clone())),
                _ => {
                    let stem = Path::new(&entry.location.replace('\\', "/"))
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    artist_and_title(&stem)
                }
            };
            match (artist, title) {
                (Some(artist), Some(title)) => by_words.push((
                    n,
                    Song {
                        title,
                        artist,
                        duration_ms: entry.duration_ms,
                        ..Song::default()
                    },
                )),
                _ => left_out += 1,
            }
        }
        if !by_words.is_empty() {
            let songs: Vec<Song> = by_words.iter().map(|(_, s)| s.clone()).collect();
            let found = self.adopt_songs(&songs)?;
            for ((n, _), id) in by_words.iter().zip(found) {
                ids[*n] = Some(id);
            }
        }
        let id = self.create_playlist(name, now_ms)?;
        let ids: Vec<i64> = ids.into_iter().flatten().collect();
        let songs = self.add_to_playlist(id, &ids, now_ms)?;
        Ok(Brought {
            id,
            name: name.to_string(),
            songs,
            files,
            added,
            by_words: by_words.len(),
            left_out,
        })
    }

    /// The playlist `id` (or, with `None`, the liked songs) as an .m3u8 file: the text, how many songs it holds, and
    /// how many were left out (songs only the add-ons play, or whose file is missing). `None`: no such playlist.
    pub fn playlist_file(&self, id: Option<i64>) -> Result<Option<(String, String, usize, usize)>> {
        const MOST: usize = 100_000;
        let (name, ids) = match id {
            Some(id) => {
                let name: Option<String> = self
                    .conn
                    .query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })
                    .optional()?;
                let Some(name) = name else {
                    return Ok(None);
                };
                (name, self.playlist_track_ids(id, 0, MOST)?)
            }
            None => ("Liked songs".to_string(), self.liked_ids_at(0, MOST)?),
        };
        let mut rows: HashMap<i64, (String, String, u64, Option<String>)> = HashMap::new();
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT id, title, (SELECT name FROM artists a WHERE a.id = t.artist_id), duration_ms,
                        CASE WHEN origin = 'file' AND missing_since IS NULL AND locator <> '' THEN locator END
                 FROM tracks t WHERE id IN ({marks})"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let found = stmt.query_map(rusqlite::params_from_iter(chunk), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        r.get::<_, i64>(3)?.max(0) as u64,
                        r.get::<_, Option<String>>(4)?,
                    ),
                ))
            })?;
            for row in found {
                let (id, row) = row?;
                rows.insert(id, row);
            }
        }
        let one_line = |s: &str| s.replace(['\r', '\n'], " ");
        let mut text = format!("#EXTM3U\n#PLAYLIST:{}\n", one_line(&name));
        let (mut written, mut left_out) = (0, 0);
        for id in &ids {
            match rows.get(id) {
                Some((title, artist, duration_ms, Some(path))) => {
                    let seconds = if *duration_ms > 0 {
                        (*duration_ms as f64 / 1000.0).round() as i64
                    } else {
                        -1
                    };
                    text.push_str(&format!(
                        "#EXTINF:{seconds},{} - {}\n{}\n",
                        one_line(artist),
                        one_line(title),
                        one_line(path)
                    ));
                    written += 1;
                }
                _ => left_out += 1,
            }
        }
        Ok(Some((name, text, written, left_out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_file_is_read_as_written() {
        let text = "\u{FEFF}#EXTM3U\r\n#PLAYLIST:Road trip\r\n\r\n#EXTINF:215,Ada Vale - Northern Lights\r\nC:\\Music\\Ada Vale\\01 Northern Lights.flac\r\n#EXTINF:-1 tvg-name=\"a, b\",Radio\r\nhttp://radio.example/stream\r\n# a comment\r\nrelative/Ben Okoro - Low Tide.mp3\r\n#extinf:180.4,Just a title\r\nfile:///C:/Music/x%20y.mp3\r\n";
        let parsed = parse(text.as_bytes());
        assert_eq!(parsed.name.as_deref(), Some("Road trip"));
        let entry = |location: &str, artist: Option<&str>, title: Option<&str>, ms| Entry {
            location: location.into(),
            artist: artist.map(Into::into),
            title: title.map(Into::into),
            duration_ms: ms,
        };
        assert_eq!(
            parsed.entries,
            [
                entry(
                    "C:\\Music\\Ada Vale\\01 Northern Lights.flac",
                    Some("Ada Vale"),
                    Some("Northern Lights"),
                    Some(215_000)
                ),
                entry("http://radio.example/stream", None, Some("Radio"), None),
                entry("relative/Ben Okoro - Low Tide.mp3", None, None, None),
                entry(
                    "file:///C:/Music/x%20y.mp3",
                    None,
                    Some("Just a title"),
                    Some(180_400)
                ),
            ]
        );
        // An old .m3u in the Windows code page.
        let old = parse(b"#EXTINF:10,Beyonc\xE9 - D\xE9j\xE0 Vu \x96 Live\nsong.mp3\n");
        assert_eq!(
            (
                old.entries[0].artist.as_deref(),
                old.entries[0].title.as_deref()
            ),
            (Some("Beyoncé"), Some("Déjà Vu – Live"))
        );
        // A leading track number is no artist.
        assert_eq!(
            artist_and_title("07 - Ada Vale - Northern Lights"),
            (Some("Ada Vale".into()), Some("Northern Lights".into()))
        );
        assert_eq!(artist_and_title("Track 7"), (None, Some("Track 7".into())));
    }

    #[test]
    fn a_place_is_made_whole_beside_the_playlist_file() {
        let beside = std::env::temp_dir();
        assert_eq!(place_of("http://radio.example/a", &beside), None);
        let relative = place_of("sub/a.mp3", &beside).unwrap();
        assert!(
            relative.is_absolute() && relative.starts_with(std::path::absolute(&beside).unwrap())
        );
        assert!(relative.ends_with(Path::new("sub").join("a.mp3")));
        if cfg!(windows) {
            assert_eq!(
                place_of("file:///C:/Music/x%20y.mp3", &beside).unwrap(),
                PathBuf::from("C:\\Music\\x y.mp3")
            );
            assert_eq!(
                place_of("C:/Music/../Music/a.mp3", &beside).unwrap(),
                PathBuf::from("C:\\Music\\a.mp3")
            );
        }
    }
}
