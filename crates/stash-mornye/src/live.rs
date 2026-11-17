//! Playing a song while its file comes in (spec §11.5; Lokal `morneye_live_playback.dart`): only a single-file
//! download whose length the server told, once 64 KiB are in and they begin a FLAC file (STREAMINFO first) or a
//! fast-start MP4 (`moov` before `mdat`, `mp4a` or `alac`, nothing encrypted). Then the file is read as it grows: a
//! read waits for its bytes (100 ms at a time), and fails after 45 s with none.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What the first bytes of a growing file are, when it can be played as it comes in: `audio/flac` (with its sample
/// rate and bit depth) or `audio/mp4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playable {
    Flac { sample_rate: u32, bit_depth: u32 },
    Mp4,
}

impl Playable {
    pub fn content_type(&self) -> &'static str {
        match self {
            Playable::Flac { .. } => "audio/flac",
            Playable::Mp4 => "audio/mp4",
        }
    }
}

/// How much must be in before playing starts.
pub const ENOUGH: u64 = 64 * 1024;

/// The fewest bytes either header check looks at (live.dart:74).
const SMALLEST_HEAD: usize = 42;
/// How long a read waits between looks.
const POLL: Duration = Duration::from_millis(100);
/// How long a read waits for new bytes before it gives up.
const STALL: Duration = Duration::from_secs(45);
/// Boxes whose presence means the audio needs a key (live.dart:129).
const PROTECTION: [&[u8; 4]; 6] = [b"sinf", b"schm", b"enca", b"encv", b"pssh", b"drms"];

/// Whether `head` (the first [`ENOUGH`] bytes or more) begins a file that can be played as it comes in
/// (live.dart:56-135).
pub fn playable(head: &[u8]) -> Option<Playable> {
    if head.len() < SMALLEST_HEAD {
        return None;
    }
    if head.starts_with(b"fLaC") {
        let (sample_rate, bit_depth) = stream_info(head)?;
        return Some(Playable::Flac {
            sample_rate,
            bit_depth,
        });
    }
    if &head[4..8] == b"ftyp" {
        return fast_start_mp4(head).then_some(Playable::Mp4);
    }
    None
}

/// Whether a destination's name can be played as it comes in (`.flac`, `.m4a`, `.mp4`).
pub fn playable_name(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "flac" | "m4a" | "mp4"
            )
        })
}

/// A FLAC file's first metadata block, when it is a sane STREAMINFO: (sample rate, bit depth).
fn stream_info(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 22 || &b[..4] != b"fLaC" || b[4] & 0x7f != 0 {
        return None;
    }
    let length = (u32::from(b[5]) << 16) | (u32::from(b[6]) << 8) | u32::from(b[7]);
    if length != 34 {
        return None;
    }
    let rate = (u32::from(b[18]) << 12) | (u32::from(b[19]) << 4) | (u32::from(b[20]) >> 4);
    let bits = (((u32::from(b[20]) & 1) << 4) | (u32::from(b[21]) >> 4)) + 1;
    ((8000..=768_000).contains(&rate) && (8..=32).contains(&bits)).then_some((rate, bits))
}

fn word(data: &[u8], at: usize) -> usize {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
}

/// The body of the first box `name` among `data`'s boxes, when every box before it, and it, is whole.
fn child<'a>(data: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let mut at = 0;
    while at + 8 <= data.len() {
        let length = word(data, at);
        if length < 8 || length > data.len() - at {
            return None;
        }
        if &data[at + 4..at + 8] == name {
            return Some(&data[at + 8..at + length]);
        }
        at += length;
    }
    None
}

/// A complete, unencrypted audio `moov` right after `ftyp` (live.dart:73-135).
fn fast_start_mp4(head: &[u8]) -> bool {
    if head.len() < 16 || &head[4..8] != b"ftyp" {
        return false;
    }
    let ftyp = word(head, 0);
    if ftyp < 8 || ftyp > head.len() {
        return false;
    }
    let rest = &head[ftyp..];
    let Some(moov) = child(rest, b"moov") else {
        return false;
    };
    // `mdat` (or anything) before `moov` is not fast-start.
    if rest.len() < 8 || word(rest, 0) < 8 || &rest[4..8] != b"moov" {
        return false;
    }
    let audio = (|| {
        let mdia = child(child(moov, b"trak")?, b"mdia")?;
        let handler = child(mdia, b"hdlr")?;
        if handler.len() < 12 || &handler[8..12] != b"soun" {
            return None;
        }
        let stsd = child(child(child(mdia, b"minf")?, b"stbl")?, b"stsd")?;
        if stsd.len() < 16 || word(stsd, 4) < 1 {
            return None;
        }
        let entry = word(stsd, 8);
        if entry < 8 || entry > stsd.len() - 8 {
            return None;
        }
        matches!(&stsd[12..16], b"mp4a" | b"alac").then_some(())
    })();
    audio.is_some()
        && !PROTECTION
            .iter()
            .any(|marker| moov.windows(4).any(|window| window == &marker[..]))
}

/// A file coming in: where it is written (`part`), what it becomes (`file`), its whole length, and how much is in.
/// Shared between the download (which moves it on) and the reader.
pub struct Growing {
    part: PathBuf,
    file: PathBuf,
    total: u64,
    stall: Duration,
    state: Mutex<State>,
    moved: Condvar,
    /// One open handle, kept across the `.part` → final rename.
    reader: Mutex<Option<File>>,
}

struct State {
    written: u64,
    done: bool,
    failed: Option<String>,
    last_movement: Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Growing {
    pub fn new(part: PathBuf, file: PathBuf, total: u64) -> std::sync::Arc<Growing> {
        Self::with_stall(part, file, total, STALL)
    }

    fn with_stall(part: PathBuf, file: PathBuf, total: u64, stall: Duration) -> Arc<Growing> {
        Arc::new(Growing {
            part,
            file,
            total,
            stall,
            state: Mutex::new(State {
                written: 0,
                done: false,
                failed: None,
                last_movement: Instant::now(),
            }),
            moved: Condvar::new(),
            reader: Mutex::new(None),
        })
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// More is in (`written` never shrinks: a smaller one fails the file with `Audio transfer restarted`).
    pub fn wrote(&self, written: u64) {
        let mut state = lock(&self.state);
        if state.failed.is_some() || state.done || written > self.total {
            return;
        }
        if written < state.written {
            // A retry replaced the bytes a player is reading: never splice two attempts.
            state.failed = Some("Audio transfer restarted".into());
        } else if written > state.written {
            state.written = written;
            state.last_movement = Instant::now();
        }
        drop(state);
        self.moved.notify_all();
    }

    /// All of it is in (`complete`), or the download failed.
    pub fn finished(&self, complete: bool) {
        let mut state = lock(&self.state);
        state.done = true;
        state.last_movement = Instant::now();
        if !complete && state.failed.is_none() {
            state.failed = Some("Mornye stream failed".into());
        }
        drop(state);
        self.moved.notify_all();
    }

    /// Reads `buf.len()` bytes or fewer from `offset`, waiting for them to come in: 0 at the end; fails when the
    /// download failed, ended short, or brought nothing for 45 s (`Mornye stream stalled`).
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if offset >= self.total || buf.is_empty() {
            return Ok(0);
        }
        let end = self.total.min(offset.saturating_add(buf.len() as u64));
        loop {
            let (available, all_in) = {
                let state = lock(&self.state);
                if let Some(error) = &state.failed {
                    return Err(std::io::Error::other(error.clone()));
                }
                if state.done {
                    (self.total, true)
                } else {
                    (state.written, false)
                }
            };
            if offset < available {
                let want = (end.min(available) - offset) as usize;
                let read = self.read_file(offset, &mut buf[..want])?;
                if read > 0 {
                    return Ok(read);
                }
            }
            let state = lock(&self.state);
            if let Some(error) = &state.failed {
                return Err(std::io::Error::other(error.clone()));
            }
            // All of it was said to be in, yet the file has nothing more.
            if all_in {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Mornye stream ended before the declared length",
                ));
            }
            if state.last_movement.elapsed() > self.stall {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Mornye stream stalled",
                ));
            }
            // Woken early when more comes in.
            let _ = self.moved.wait_timeout(state, POLL);
        }
    }

    /// Reads from the open handle, opening the file first: the final one once all is in, else the `.part`. Nothing
    /// is read (0) while neither can be opened yet.
    fn read_file(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut reader = lock(&self.reader);
        if reader.is_none() {
            let done = lock(&self.state).done;
            let paths: &[&PathBuf] = if done {
                &[&self.file, &self.part]
            } else {
                &[&self.part, &self.file]
            };
            *reader = paths.iter().find_map(|path| File::open(path).ok());
        }
        let Some(file) = reader.as_mut() else {
            return Ok(0);
        };
        file.seek(SeekFrom::Start(offset))?;
        file.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A FLAC head: `fLaC`, then a STREAMINFO block of 34 bytes.
    fn flac_head(rate: u32, bits: u32) -> Vec<u8> {
        let mut head = b"fLaC".to_vec();
        head.extend([0x80, 0, 0, 34]); // the last metadata block, STREAMINFO, 34 bytes
        let mut info = vec![0u8; 34];
        info[0..2].copy_from_slice(&4096u16.to_be_bytes());
        info[2..4].copy_from_slice(&4096u16.to_be_bytes());
        info[10] = (rate >> 12) as u8;
        info[11] = (rate >> 4) as u8;
        info[12] = (((rate & 0xf) << 4) as u8) | (1 << 1) | (((bits - 1) >> 4) as u8);
        info[13] = (((bits - 1) & 0xf) << 4) as u8;
        head.extend(info);
        head.resize(64 * 1024, 0);
        head
    }

    #[test]
    fn a_flac_head_is_read() {
        assert_eq!(
            playable(&flac_head(44_100, 16)),
            Some(Playable::Flac {
                sample_rate: 44_100,
                bit_depth: 16
            })
        );
        assert_eq!(
            playable(&flac_head(96_000, 24)),
            Some(Playable::Flac {
                sample_rate: 96_000,
                bit_depth: 24
            })
        );
        assert_eq!(
            playable(&flac_head(96_000, 24)).unwrap().content_type(),
            "audio/flac"
        );
    }

    #[test]
    fn a_bad_flac_head_is_refused() {
        // Too short to tell.
        assert_eq!(playable(&flac_head(44_100, 16)[..41]), None);
        // Another block first (VORBIS_COMMENT).
        let mut head = flac_head(44_100, 16);
        head[4] = 0x04;
        assert_eq!(playable(&head), None);
        // A STREAMINFO of the wrong length.
        let mut head = flac_head(44_100, 16);
        head[7] = 33;
        assert_eq!(playable(&head), None);
        // A rate no one plays.
        assert_eq!(playable(&flac_head(1_000, 16)), None);
        assert_eq!(playable(&flac_head(44_100, 4)), None);
        assert_eq!(playable(&[0u8; 100]), None);
    }

    fn mp4_box(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut data = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        data.extend_from_slice(name);
        data.extend_from_slice(body);
        data
    }

    /// `moov` with one audio track whose sample entry is `entry`; `extra` goes inside the entry.
    fn moov(handler: &[u8; 4], entry: &[u8; 4], extra: &[u8]) -> Vec<u8> {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0u8; 12]);
        let mut sample = vec![0u8; 28];
        sample.extend_from_slice(extra);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(mp4_box(entry, &sample));
        let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &stsd));
        let minf = mp4_box(b"minf", &stbl);
        let mut mdia = mp4_box(b"mdhd", &[0u8; 24]);
        mdia.extend(mp4_box(b"hdlr", &hdlr));
        mdia.extend(minf);
        let mut trak = mp4_box(b"tkhd", &[0u8; 84]);
        trak.extend(mp4_box(b"mdia", &mdia));
        let mut body = mp4_box(b"mvhd", &[0u8; 100]);
        body.extend(mp4_box(b"trak", &trak));
        mp4_box(b"moov", &body)
    }

    fn ftyp() -> Vec<u8> {
        mp4_box(b"ftyp", b"M4A \0\0\0\0M4A mp42isom")
    }

    fn mp4(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut file = parts.concat();
        file.resize(file.len().max(64 * 1024), 0);
        file
    }

    #[test]
    fn a_fast_start_mp4_is_read() {
        let mdat = mp4_box(b"mdat", &[1u8; 1000]);
        let head = mp4(&[ftyp(), moov(b"soun", b"mp4a", b"esds"), mdat.clone()]);
        assert_eq!(playable(&head), Some(Playable::Mp4));
        assert_eq!(playable(&head).unwrap().content_type(), "audio/mp4");
        let alac = mp4(&[ftyp(), moov(b"soun", b"alac", &[]), mdat]);
        assert_eq!(playable(&alac), Some(Playable::Mp4));
    }

    #[test]
    fn other_mp4s_are_refused() {
        let mdat = mp4_box(b"mdat", &[1u8; 1000]);
        // The media first.
        assert_eq!(
            playable(&mp4(&[ftyp(), mdat.clone(), moov(b"soun", b"mp4a", &[])])),
            None
        );
        // Encrypted entries or protection boxes.
        assert_eq!(
            playable(&mp4(&[ftyp(), moov(b"soun", b"enca", &[]), mdat.clone()])),
            None
        );
        let sinf = mp4_box(b"sinf", &mp4_box(b"frma", b"mp4a"));
        assert_eq!(
            playable(&mp4(&[ftyp(), moov(b"soun", b"mp4a", &sinf), mdat.clone()])),
            None
        );
        // Not sound, or a codec the gate does not take.
        assert_eq!(
            playable(&mp4(&[ftyp(), moov(b"vide", b"mp4a", &[]), mdat.clone()])),
            None
        );
        assert_eq!(
            playable(&mp4(&[ftyp(), moov(b"soun", b"fLaC", &[]), mdat.clone()])),
            None
        );
        // A `moov` cut off by the end of the head.
        let whole = [ftyp(), moov(b"soun", b"mp4a", &[])].concat();
        assert_eq!(playable(&whole[..whole.len() - 10]), None);
    }

    #[test]
    fn names_that_can_play_as_they_come() {
        assert!(playable_name(Path::new("work/audio.flac")));
        assert!(playable_name(Path::new("audio.M4A")));
        assert!(playable_name(Path::new("audio.mp4")));
        assert!(!playable_name(Path::new("audio.opus")));
        assert!(!playable_name(Path::new("audio")));
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stash-mornye-live-{name}-{}",
            crate::hex(&crate::random_bytes(6))
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_all(growing: &Growing) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 1000];
        loop {
            let read = growing.read_at(out.len() as u64, &mut buf)?;
            if read == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..read]);
        }
    }

    #[test]
    fn a_read_waits_for_the_writer_and_follows_the_rename() {
        let dir = folder("grow");
        let (part, file) = (dir.join("audio.flac.part"), dir.join("audio.flac"));
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        let growing = Growing::new(part.clone(), file.clone(), data.len() as u64);
        let writer = {
            let (growing, data, part, file) =
                (growing.clone(), data.clone(), part.clone(), file.clone());
            std::thread::spawn(move || {
                let mut out = std::fs::File::create(&part).unwrap();
                for chunk in data.chunks(3_000) {
                    std::thread::sleep(Duration::from_millis(30));
                    out.write_all(chunk).unwrap();
                    out.flush().unwrap();
                    growing.wrote(out.stream_position().unwrap());
                }
                drop(out);
                std::fs::rename(&part, &file).unwrap();
                growing.finished(true);
            })
        };
        assert_eq!(growing.total(), data.len() as u64);
        let read = read_all(&growing).unwrap();
        writer.join().unwrap();
        assert_eq!(read, data);
        // Reads after the end, and reads after the rename, still work.
        let mut buf = [0u8; 10];
        assert_eq!(growing.read_at(data.len() as u64, &mut buf).unwrap(), 0);
        assert_eq!(growing.read_at(5, &mut buf).unwrap(), 10);
        assert_eq!(&buf, &data[5..15]);
        drop(growing);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_restarted_transfer_fails_the_reader() {
        let dir = folder("restart");
        let part = dir.join("a.flac.part");
        std::fs::write(&part, vec![7u8; 100]).unwrap();
        let growing = Growing::new(part, dir.join("a.flac"), 1000);
        growing.wrote(100);
        let mut buf = [0u8; 50];
        assert_eq!(growing.read_at(0, &mut buf).unwrap(), 50);
        growing.wrote(40);
        let error = growing.read_at(50, &mut buf).unwrap_err();
        assert_eq!(error.to_string(), "Audio transfer restarted");
        drop(growing);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_short_or_failed_download_fails_the_reader() {
        let dir = folder("short");
        let file = dir.join("a.flac");
        std::fs::write(&file, vec![1u8; 100]).unwrap();
        let short = Growing::new(dir.join("a.flac.part"), file.clone(), 1000);
        short.wrote(100);
        short.finished(true);
        let mut buf = [0u8; 200];
        assert_eq!(short.read_at(0, &mut buf).unwrap(), 100);
        assert_eq!(
            short.read_at(100, &mut buf).unwrap_err().kind(),
            std::io::ErrorKind::UnexpectedEof
        );

        let failed = Growing::new(dir.join("b.part"), dir.join("b.flac"), 1000);
        failed.finished(false);
        assert!(failed.read_at(0, &mut buf).is_err());
        drop((short, failed));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_reader_gives_up_when_nothing_comes() {
        let dir = folder("stall");
        let growing = Growing::with_stall(
            dir.join("c.part"),
            dir.join("c.flac"),
            1000,
            Duration::from_millis(250),
        );
        let mut buf = [0u8; 10];
        let started = Instant::now();
        let error = growing.read_at(0, &mut buf).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "Mornye stream stalled");
        assert!(started.elapsed() >= Duration::from_millis(250));
        let _ = std::fs::remove_dir_all(dir);
    }
}
