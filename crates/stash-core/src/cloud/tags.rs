//! A cloud file's tags, read without downloading it: the tag reader is handed a reader that fetches only the pieces it
//! reads (a file's tags are at its start, an MP4's index may be at its end), with a cap on how much it may fetch. A
//! file that cannot be read so is known by its name and its folders.

use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

use lofty::config::ParseOptions;
use lofty::file::FileType;
use lofty::probe::Probe;

use crate::covers::Cover;
use crate::library::{NewTrack, Origin};

/// How much is fetched at a time.
const BLOCK: u64 = 256 * 1024;
/// The most fetched of one file: tags with a large picture fit; a reader that goes through the audio is stopped.
const CAP: u64 = 12 * 1024 * 1024;
/// How many pieces are kept while one file is read.
const KEPT: usize = 16;

/// A file of `size` bytes read by fetching pieces of it (`fetch(start, length)`).
pub struct RangeReader<F> {
    fetch: F,
    size: u64,
    pos: u64,
    pieces: HashMap<u64, Vec<u8>>,
    order: Vec<u64>,
    fetched: u64,
}

impl<F: FnMut(u64, u64) -> io::Result<Vec<u8>>> RangeReader<F> {
    pub fn new(size: u64, fetch: F) -> Self {
        Self {
            fetch,
            size,
            pos: 0,
            pieces: HashMap::new(),
            order: Vec::new(),
            fetched: 0,
        }
    }

    /// How many bytes were fetched.
    pub fn fetched(&self) -> u64 {
        self.fetched
    }

    fn piece(&mut self, start: u64) -> io::Result<&Vec<u8>> {
        if !self.pieces.contains_key(&start) {
            let length = BLOCK.min(self.size - start);
            if self.fetched + length > CAP {
                return Err(io::Error::other("the file's tags are too far into it"));
            }
            let bytes = (self.fetch)(start, length)?;
            if bytes.len() as u64 != length {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the file came back shorter than it is",
                ));
            }
            self.fetched += length;
            if self.order.len() >= KEPT {
                let oldest = self.order.remove(0);
                self.pieces.remove(&oldest);
            }
            self.order.push(start);
            self.pieces.insert(start, bytes);
        }
        Ok(&self.pieces[&start])
    }
}

impl<F: FnMut(u64, u64) -> io::Result<Vec<u8>>> Read for RangeReader<F> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let start = self.pos - self.pos % BLOCK;
        let offset = (self.pos - start) as usize;
        let piece = self.piece(start)?;
        let n = buf.len().min(piece.len() - offset);
        buf[..n].copy_from_slice(&piece[offset..offset + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<F> Seek for RangeReader<F> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(n) => self.size.checked_add_signed(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a place before the start"))?;
        self.pos = pos;
        Ok(pos)
    }
}

/// A song a cloud file holds, and its picture.
#[derive(Debug, Clone)]
pub struct FileSong {
    pub track: NewTrack,
    pub cover: Option<Cover>,
    /// Whether its tags were read (else it is known by its name).
    pub tagged: bool,
}

/// The file named `name` (of `size` bytes, in `folders` below the picked folder when known), read through `fetch`.
pub fn read(
    name: &str,
    size: u64,
    folders: Option<&[String]>,
    now_s: i64,
    fetch: impl FnMut(u64, u64) -> io::Result<Vec<u8>>,
) -> FileSong {
    let named = by_name(name, folders, now_s);
    let mut reader = io::BufReader::with_capacity(64 * 1024, RangeReader::new(size, fetch));
    let by_ending = std::path::Path::new(name)
        .extension()
        .and_then(FileType::from_ext);
    let options = ParseOptions::new().read_cover_art(true);
    let probed = match by_ending {
        Some(kind) => Ok(Probe::with_file_type(&mut reader, kind)),
        None => Probe::new(&mut reader).guess_file_type(),
    };
    let file = probed.ok().and_then(|p| p.options(options).read().ok());
    let Some(file) = file else {
        return FileSong {
            track: named,
            cover: None,
            tagged: false,
        };
    };
    let mut track = crate::scan::from_tagged(&file, named.title.clone(), now_s);
    // Tags that do not name the artist or the album: the folders do.
    if track.artist == crate::scan::UNKNOWN_ARTIST {
        track.artist = named.artist;
    }
    if track.album.is_none() {
        track.album = named.album;
    }
    track.origin = Origin::Described;
    FileSong {
        cover: crate::covers::of_tagged(&file),
        track,
        tagged: true,
    }
}

/// A song known by its file's name ("01 - Title.flac": "Title") and its folders ("Artist/Album/…").
pub fn by_name(name: &str, folders: Option<&[String]>, now_s: i64) -> NewTrack {
    let stem = std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    // A leading track number ("01 ", "01 - ", "1. ") is not part of the title.
    let digits = stem.chars().take_while(char::is_ascii_digit).count();
    let (track_no, rest) = if (1..=3).contains(&digits) {
        let rest = stem[digits..].trim_start_matches([' ', '-', '.', '_']);
        (stem[..digits].parse().unwrap_or(0), rest)
    } else {
        (0, stem.as_str())
    };
    let title = if rest.trim().is_empty() {
        stem.trim()
    } else {
        rest.trim()
    }
    .to_string();
    let folders = folders.unwrap_or_default();
    let (artist, album) = match folders {
        [.., artist, album] => (Some(artist.clone()), Some(album.clone())),
        [album] => (None, Some(album.clone())),
        [] => (None, None),
    };
    NewTrack {
        title,
        artist: artist.unwrap_or_else(|| crate::scan::UNKNOWN_ARTIST.to_string()),
        album_artist: None,
        album,
        year: None,
        disc_no: 1,
        track_no,
        duration_ms: 0,
        isrc: None,
        origin: Origin::Described,
        addon_id: None,
        locator: String::new(),
        codec: None,
        sample_rate: None,
        bit_depth: None,
        added_at: now_s,
        file_size: None,
        file_mtime: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_read_by_the_pieces_asked_for_and_no_more() {
        let data: Vec<u8> = (0..(BLOCK * 3 + 10)).map(|i| (i % 251) as u8).collect();
        let mut asked = Vec::new();
        let mut reader = RangeReader::new(data.len() as u64, |start, len| {
            asked.push((start, len));
            Ok(data[start as usize..(start + len) as usize].to_vec())
        });
        let mut head = [0u8; 10];
        reader.read_exact(&mut head).unwrap();
        assert_eq!(&head, &data[..10]);
        reader.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &data[data.len() - 4..]);
        // Across a piece's edge.
        reader.seek(SeekFrom::Start(BLOCK - 2)).unwrap();
        let mut across = [0u8; 4];
        reader.read_exact(&mut across).unwrap();
        assert_eq!(&across, &data[(BLOCK - 2) as usize..(BLOCK + 2) as usize]);
        assert_eq!(reader.fetched(), BLOCK * 2 + 10);
        drop(reader);
        assert_eq!(asked, vec![(0, BLOCK), (BLOCK * 3, 10), (BLOCK, BLOCK)]);
    }

    #[test]
    fn a_reader_that_goes_through_the_audio_is_stopped() {
        let size = CAP * 2;
        let mut reader = RangeReader::new(size, |_, len| Ok(vec![0u8; len as usize]));
        let mut sink = Vec::new();
        assert!(reader.read_to_end(&mut sink).is_err());
        assert!(reader.fetched() <= CAP);
    }

    #[test]
    fn a_file_without_tags_is_known_by_its_name_and_folders() {
        let folders = vec!["Rips".to_string(), "Artist".into(), "Album".into()];
        let t = by_name("03 - The Song.flac", Some(&folders), 7);
        assert_eq!(
            (
                t.title.as_str(),
                t.artist.as_str(),
                t.album.as_deref(),
                t.track_no
            ),
            ("The Song", "Artist", Some("Album"), 3)
        );
        let t = by_name("1979.mp3", None, 7);
        assert_eq!((t.title.as_str(), t.track_no), ("1979", 0));
        let t = by_name("07.mp3", Some(&["Album".to_string()]), 7);
        assert_eq!(
            (t.title.as_str(), t.artist.as_str(), t.album.as_deref()),
            ("07", crate::scan::UNKNOWN_ARTIST, Some("Album"))
        );
        // What is not music is not read as tags, and falls back to the name.
        let read = read("x.flac", 4, None, 7, |_, len| Ok(vec![0u8; len as usize]));
        assert!(!read.tagged);
        assert_eq!(read.track.title, "x");
    }
}
