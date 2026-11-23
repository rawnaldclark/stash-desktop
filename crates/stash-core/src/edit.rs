//! Editing a file's tags: the listener changes what a song is called, who it is by, its album, genre, year and the
//! rest, for one song or many at once, and the change is written into the files themselves, so any other player sees
//! it too. The library's rows are then read again from the files, exactly as a scan would.
//!
//! Only files can be edited: a song that is only an entry in the library has no tags to write.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::library::{Library, ScanChange};
use crate::scan::read_track;
use crate::table::read_more;

/// What to change. A field left out stays as it is in every file; a field given as "" is cleared; anything else is
/// written (trimmed). So one edit can set the genre of a hundred songs and leave their titles alone.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TagEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_no: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_no: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
}

impl TagEdit {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The first thing wrong with what was typed, in words for the listener; `None` when it can be written.
    pub fn wrong(&self) -> Option<&'static str> {
        let number = |v: &Option<String>, most: u32| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some_and(|s| !s.parse::<u32>().is_ok_and(|n| n <= most));
        if self.title.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return Some("A song needs a title");
        }
        if self.artist.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return Some("A song needs an artist");
        }
        if self.year.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some_and(|y| !y.parse::<u32>().is_ok_and(|n| (1000..=9999).contains(&n))) {
            return Some("The year is four digits");
        }
        if number(&self.track_no, 9_999) {
            return Some("The track number is a number");
        }
        if number(&self.disc_no, 999) {
            return Some("The disc number is a number");
        }
        None
    }
}

/// One field's value across the songs being edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Common {
    /// The value they all have ("" when they all have none); `None` when they differ.
    pub value: Option<String>,
}

/// What the songs being edited say now, field by field, for the form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagsNow {
    /// How many of the songs asked about are files that can be edited, and how many are not.
    pub files: usize,
    pub others: usize,
    pub title: Common,
    pub artist: Common,
    pub album: Common,
    pub album_artist: Common,
    pub genre: Common,
    pub year: Common,
    pub track_no: Common,
    pub disc_no: Common,
    pub composer: Common,
}

/// What an edit did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct EditReport {
    pub written: usize,
    /// Songs that are not files here: left alone.
    pub skipped: usize,
    /// Files that could not be written, each with why (read-only, in use, gone).
    pub failed: Vec<(i64, String)>,
}

/// Writes `edit` into the tags of the file at `path`. A file with no tags gets the kind its format normally carries.
pub fn write_tags(path: &Path, edit: &TagEdit) -> std::result::Result<(), String> {
    let mut file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let kind = file.primary_tag_type();
    if file.primary_tag().is_none() {
        // Keep what another kind of tag in the file says (an old ID3v1, say), in the kind the format prefers.
        let mut fresh = Tag::new(kind);
        if let Some(other) = file.first_tag() {
            for item in other.items() {
                fresh.insert_unchecked(item.clone());
            }
            for picture in other.pictures() {
                fresh.push_picture(picture.clone());
            }
        }
        file.insert_tag(fresh);
    }
    let tag = file.primary_tag_mut().ok_or("the file takes no tags")?;
    let text = |v: &Option<String>| v.as_deref().map(str::trim).map(str::to_string);

    if let Some(v) = text(&edit.title) {
        tag.set_title(v);
    }
    if let Some(v) = text(&edit.artist) {
        tag.set_artist(v);
    }
    match text(&edit.album) {
        Some(v) if v.is_empty() => tag.remove_album(),
        Some(v) => tag.set_album(v),
        None => {}
    }
    match text(&edit.genre) {
        Some(v) if v.is_empty() => tag.remove_genre(),
        Some(v) => tag.set_genre(v),
        None => {}
    }
    for (value, key) in [(text(&edit.album_artist), ItemKey::AlbumArtist), (text(&edit.composer), ItemKey::Composer)] {
        match value {
            Some(v) if v.is_empty() => tag.remove_key(key),
            Some(v) => {
                tag.insert_text(key, v);
            }
            None => {}
        }
    }
    match text(&edit.year) {
        Some(v) if v.is_empty() => {
            tag.remove_key(ItemKey::RecordingDate);
            tag.remove_key(ItemKey::Year);
        }
        Some(v) => {
            tag.remove_key(ItemKey::Year);
            tag.insert_text(ItemKey::RecordingDate, v);
        }
        None => {}
    }
    match text(&edit.track_no) {
        Some(v) if v.is_empty() => tag.remove_track(),
        Some(v) => tag.set_track(v.parse().map_err(|_| "the track number is a number")?),
        None => {}
    }
    match text(&edit.disc_no) {
        Some(v) if v.is_empty() => tag.remove_disk(),
        Some(v) => tag.set_disk(v.parse().map_err(|_| "the disc number is a number")?),
        None => {}
    }
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

impl Library {
    /// The file of the song `id`, when it is a file that is here.
    fn editable_file(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT locator FROM tracks WHERE id = ?1 AND origin = 'file' AND locator != '' AND missing_since IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// What the songs `ids` say now, for the form: each field's value when they agree.
    pub fn tags_now(&self, ids: &[i64]) -> Result<TagsNow> {
        let mut read = self.conn.prepare_cached(
            "SELECT t.title, a.name, COALESCE(al.title, ''), COALESCE(aa.name, ''), COALESCE(t.genre, ''),
                    COALESCE(CAST(COALESCE(t.year, al.year) AS TEXT), ''), CASE WHEN t.track_no > 0 THEN CAST(t.track_no AS TEXT) ELSE '' END,
                    CAST(t.disc_no AS TEXT), COALESCE(t.composer, ''),
                    t.origin = 'file' AND t.locator != '' AND t.missing_since IS NULL
             FROM tracks t JOIN artists a ON a.id = t.artist_id
             LEFT JOIN albums al ON al.id = t.album_id LEFT JOIN artists aa ON aa.id = al.artist_id
             WHERE t.id = ?1",
        )?;
        let mut fields: [Option<Common>; 9] = Default::default();
        let (mut files, mut others) = (0, 0);
        for &id in ids {
            let Some((values, file)) = read
                .query_row([id], |r| {
                    let mut v: [String; 9] = Default::default();
                    for (i, slot) in v.iter_mut().enumerate() {
                        *slot = r.get(i)?;
                    }
                    Ok((v, r.get::<_, bool>(9)?))
                })
                .optional()?
            else {
                continue;
            };
            if !file {
                others += 1;
                continue;
            }
            files += 1;
            for (slot, value) in fields.iter_mut().zip(values) {
                match slot {
                    None => *slot = Some(Common { value: Some(value) }),
                    Some(common) if common.value.as_deref() != Some(value.as_str()) => common.value = None,
                    Some(_) => {}
                }
            }
        }
        let mut it = fields.into_iter().map(|f| f.unwrap_or(Common { value: Some(String::new()) }));
        let mut next = || it.next().unwrap();
        Ok(TagsNow {
            files,
            others,
            title: next(),
            artist: next(),
            album: next(),
            album_artist: next(),
            genre: next(),
            year: next(),
            track_no: next(),
            disc_no: next(),
            composer: next(),
        })
    }

    /// Writes `edit` into the files of the songs `ids`, and reads each file's row again from what was written. A song
    /// that is not a file here is skipped; a file that cannot be written is reported and left as it was.
    pub fn edit_tags(&mut self, ids: &[i64], edit: &TagEdit, now: i64) -> Result<EditReport> {
        let mut report = EditReport::default();
        for &id in ids {
            let Some(locator) = self.editable_file(id)? else {
                report.skipped += 1;
                continue;
            };
            let path = Path::new(&locator);
            if let Err(why) = write_tags(path, edit) {
                report.failed.push((id, why));
                continue;
            }
            let meta = std::fs::metadata(path);
            let (size, mtime) = meta
                .map(|m| {
                    let at = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
                    (m.len(), at)
                })
                .unwrap_or((0, 0));
            match read_track(path, &locator, size, mtime, now) {
                Ok(track) => {
                    self.apply_scan(&[ScanChange::Changed(id, track)])?;
                    self.set_more_tags(&[(id, read_more(path).ok())])?;
                    report.written += 1;
                }
                Err(why) => report.failed.push((id, why)),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan_folder;

    /// A second of silence as a WAV file.
    fn wav(path: &Path) {
        let rate = 8_000u32;
        let data = vec![0u8; (rate * 2) as usize];
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(path, out).unwrap();
    }

    fn set(pairs: &[(&str, &str)]) -> TagEdit {
        let get = |k: &str| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string());
        TagEdit {
            title: get("title"),
            artist: get("artist"),
            album: get("album"),
            album_artist: get("album_artist"),
            genre: get("genre"),
            year: get("year"),
            track_no: get("track_no"),
            disc_no: get("disc_no"),
            composer: get("composer"),
        }
    }

    fn library_of(name: &str, files: &[&str]) -> (Library, std::path::PathBuf, Vec<i64>) {
        let dir = std::env::temp_dir().join(format!("stash-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            wav(&dir.join(f));
        }
        let mut library = Library::open_in_memory().unwrap();
        scan_folder(&mut library, &dir, |_| {}).unwrap();
        let ids = library
            .conn
            .prepare("SELECT id FROM tracks ORDER BY locator")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<i64>>>()
            .unwrap();
        (library, dir, ids)
    }

    #[test]
    fn one_songs_tags_are_written_into_its_file_and_read_back() {
        let (mut library, dir, ids) = library_of("one", &["a.wav"]);
        let edit = set(&[("title", " So What "), ("artist", "Miles Davis"), ("album", "Kind of Blue"), ("genre", "Jazz"), ("year", "1959"), ("track_no", "1"), ("composer", "Miles Davis")]);
        assert_eq!(edit.wrong(), None);
        let report = library.edit_tags(&ids, &edit, 10).unwrap();
        assert_eq!((report.written, report.skipped, report.failed.len()), (1, 0, 0));

        // In the file itself: another player reads the same.
        let more = read_more(&dir.join("a.wav")).unwrap();
        assert_eq!((more.genre.as_deref(), more.year, more.composer.as_deref()), (Some("Jazz"), Some(1959), Some("Miles Davis")));
        // And in the library, as the form shows it.
        let now = library.tags_now(&ids).unwrap();
        assert_eq!(now.files, 1);
        assert_eq!(now.title.value.as_deref(), Some("So What"));
        assert_eq!(now.artist.value.as_deref(), Some("Miles Davis"));
        assert_eq!(now.album.value.as_deref(), Some("Kind of Blue"));
        assert_eq!(now.genre.value.as_deref(), Some("Jazz"));
        assert_eq!(now.year.value.as_deref(), Some("1959"));
        assert_eq!(now.track_no.value.as_deref(), Some("1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn many_songs_take_only_the_fields_given_and_a_blank_clears() {
        let (mut library, dir, ids) = library_of("many", &["a.wav", "b.wav", "c.wav"]);
        library.edit_tags(&ids[..1], &set(&[("title", "First"), ("genre", "Rock")]), 10).unwrap();
        library.edit_tags(&ids[1..2], &set(&[("title", "Second"), ("genre", "Pop")]), 10).unwrap();

        // The form for all three: the titles differ, and so do the genres.
        let before = library.tags_now(&ids).unwrap();
        assert_eq!(before.files, 3);
        assert_eq!(before.title.value, None);
        assert_eq!(before.genre.value, None);

        // One genre and one album for all of them; their titles are left alone.
        let report = library.edit_tags(&ids, &set(&[("genre", "Jazz"), ("album", "Together")]), 11).unwrap();
        assert_eq!(report.written, 3);
        let after = library.tags_now(&ids).unwrap();
        assert_eq!(after.genre.value.as_deref(), Some("Jazz"));
        assert_eq!(after.album.value.as_deref(), Some("Together"));
        assert_eq!(after.title.value, None);
        assert_eq!(library.tags_now(&ids[..1]).unwrap().title.value.as_deref(), Some("First"));
        assert_eq!(library.genres(false).unwrap(), vec![(Some("Jazz".to_string()), 3)]);

        // A blank clears the field in every file.
        library.edit_tags(&ids, &set(&[("genre", "")]), 12).unwrap();
        assert_eq!(read_more(&dir.join("a.wav")).unwrap().genre, None);
        assert_eq!(library.tags_now(&ids).unwrap().genre.value.as_deref(), Some(""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_cannot_be_written_is_said_and_the_rest_still_are() {
        let (mut library, dir, ids) = library_of("fail", &["a.wav", "b.wav"]);
        std::fs::remove_file(dir.join("a.wav")).unwrap(); // gone since the scan
        let entry = library.adopt(&crate::import::Song { title: "Only an entry".into(), artist: "Someone".into(), ..Default::default() }).unwrap();
        let report = library.edit_tags(&[ids[0], ids[1], entry, 9_999], &set(&[("genre", "Jazz")]), 10).unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(report.skipped, 2); // the entry with no file, and the id that is no song
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, ids[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonsense_is_caught_before_anything_is_written() {
        assert_eq!(set(&[("title", "  ")]).wrong(), Some("A song needs a title"));
        assert_eq!(set(&[("artist", "")]).wrong(), Some("A song needs an artist"));
        assert_eq!(set(&[("year", "59")]).wrong(), Some("The year is four digits"));
        assert_eq!(set(&[("track_no", "one")]).wrong(), Some("The track number is a number"));
        assert_eq!(set(&[("disc_no", "-1")]).wrong(), Some("The disc number is a number"));
        assert_eq!(set(&[("year", ""), ("track_no", ""), ("genre", "")]).wrong(), None);
        assert!(TagEdit::default().is_empty());
        assert!(!set(&[("genre", "")]).is_empty());
    }
}
