//! Scanning a music folder into the library.
//!
//! A scan walks the folder, reads the tags of every audio file it has not seen (or whose size or time changed), and
//! writes what it found in batches, so a list in the window fills while a long scan runs. A file that is no longer
//! there is marked missing, never deleted: its row keeps its play counts for the day the drive is plugged in again.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{MAIN_SEPARATOR, Path};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use lofty::file::FileType;
use lofty::prelude::*;
use serde::Serialize;
use walkdir::WalkDir;

use crate::library::{Library, NewTrack, Origin, ScanChange};
use crate::{Error, Result};

/// How many files are written at a time.
const BATCH: usize = 500;

/// The file name endings a scan reads.
const AUDIO: &[&str] = &[
    "flac", "mp3", "m4a", "m4b", "mp4", "aac", "ogg", "oga", "opus", "wav", "aiff", "aif", "ape",
    "wv", "mpc",
];

/// The artist of a file whose tags name none.
pub const UNKNOWN_ARTIST: &str = "Unknown artist";

/// What a scan did, so far or in the end.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ScanReport {
    /// Audio files found.
    pub files: usize,
    pub added: usize,
    /// Files whose tags were read again because their size or time changed.
    pub changed: usize,
    pub unchanged: usize,
    /// Files that could not be read as audio. They are left out, and named in `unreadable_examples` (the first few).
    pub unreadable: usize,
    pub unreadable_examples: Vec<String>,
    /// Files the library had under this folder that are not there now.
    pub missing: usize,
    pub seconds: f64,
}

/// Scans `root` into `library`. `progress` hears the running totals after each batch is written.
pub fn scan_folder(
    library: &mut Library,
    root: &Path,
    mut progress: impl FnMut(&ScanReport),
) -> Result<ScanReport> {
    let root =
        std::path::absolute(root).map_err(|e| Error::Scan(format!("{}: {e}", root.display())))?;
    if !root.is_dir() {
        return Err(Error::Scan(format!("{} is not a folder", root.display())));
    }
    let started = Instant::now();
    let now = unix_seconds(SystemTime::now());
    let mut report = ScanReport::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut batch: Vec<ScanChange> = Vec::with_capacity(BATCH);

    for entry in WalkDir::new(&root).follow_links(false) {
        let Ok(entry) = entry else {
            continue; // a folder that cannot be listed: what is inside it is not judged missing, and not added
        };
        let path = entry.path();
        if !entry.file_type().is_file() || !is_audio(path) {
            continue;
        }
        let locator = path.to_string_lossy().into_owned();
        report.files += 1;
        seen.insert(locator.clone());

        let Ok(meta) = entry.metadata() else {
            note_unreadable(&mut report, &locator, "its size and time cannot be read");
            continue;
        };
        let (size, mtime) = (meta.len(), meta.modified().map(unix_seconds).unwrap_or(0));
        let known = library.file_state(&locator)?;
        if let Some(state) = known
            && state.size == Some(size)
            && state.mtime == Some(mtime)
        {
            report.unchanged += 1;
            if state.missing {
                batch.push(ScanChange::Seen(state.id));
            }
        } else {
            match read_track(path, &locator, size, mtime, now) {
                Ok(track) => match known {
                    Some(state) => {
                        report.changed += 1;
                        batch.push(ScanChange::Changed(state.id, track));
                    }
                    None => {
                        report.added += 1;
                        batch.push(ScanChange::Added(track));
                    }
                },
                Err(why) => note_unreadable(&mut report, &locator, &why),
            }
        }

        if batch.len() >= BATCH {
            library.apply_scan(&batch)?;
            batch.clear();
            report.seconds = started.elapsed().as_secs_f64();
            progress(&report);
        }
    }
    library.apply_scan(&batch)?;

    // Only paths below this folder are judged: "D:\Music" must not touch "D:\Music2".
    let mut prefix = root.to_string_lossy().into_owned();
    if !prefix.ends_with(MAIN_SEPARATOR) {
        prefix.push(MAIN_SEPARATOR);
    }
    report.missing = library.mark_missing(&prefix, &seen, now)?;
    report.seconds = started.elapsed().as_secs_f64();
    progress(&report);
    Ok(report)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn note_unreadable(report: &mut ScanReport, locator: &str, why: &str) {
    report.unreadable += 1;
    if report.unreadable_examples.len() < 10 {
        report.unreadable_examples.push(format!("{locator}: {why}"));
    }
}

fn text(value: Option<Cow<'_, str>>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The track a file holds: its tags where it has them, its name where it does not, and what the audio is.
fn read_track(
    path: &Path,
    locator: &str,
    size: u64,
    mtime: i64,
    now: i64,
) -> std::result::Result<NewTrack, String> {
    let file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let properties = file.properties();
    let tag = file.primary_tag().or_else(|| file.first_tag());

    let title = tag
        .and_then(|t| text(t.title()))
        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| locator.to_string());
    let artist = tag
        .and_then(|t| text(t.artist()))
        .unwrap_or_else(|| UNKNOWN_ARTIST.to_string());
    let lossless = matches!(
        file.file_type(),
        FileType::Flac | FileType::Wav | FileType::Aiff | FileType::Ape | FileType::WavPack
    );
    // An MP4 file holds AAC or Apple Lossless; only the lossless one has a bit depth.
    let codec = match file.file_type() {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Mp4 if properties.bit_depth().is_some() => "alac",
        FileType::Mp4 | FileType::Aac => "aac",
        FileType::Opus => "opus",
        FileType::Vorbis => "vorbis",
        FileType::Wav | FileType::Aiff => "pcm",
        FileType::Ape => "ape",
        FileType::WavPack => "wavpack",
        FileType::Mpc => "musepack",
        FileType::Speex => "speex",
        _ => "",
    };
    let bit_depth = properties
        .bit_depth()
        .map(u32::from)
        .filter(|_| lossless || codec == "alac");

    Ok(NewTrack {
        title,
        artist,
        album_artist: tag
            .and_then(|t| t.get_string(ItemKey::AlbumArtist))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        album: tag.and_then(|t| text(t.album())),
        year: tag.and_then(|t| t.date()).map(|d| i32::from(d.year)),
        disc_no: tag.and_then(|t| t.disk()).unwrap_or(1),
        track_no: tag.and_then(|t| t.track()).unwrap_or(0),
        duration_ms: properties.duration().as_millis() as u64,
        isrc: tag
            .and_then(|t| t.get_string(ItemKey::Isrc))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        origin: Origin::File,
        addon_id: None,
        locator: locator.to_string(),
        codec: (!codec.is_empty()).then(|| codec.to_string()),
        sample_rate: properties.sample_rate(),
        bit_depth,
        added_at: now,
        file_size: Some(size),
        file_mtime: Some(mtime),
    })
}
