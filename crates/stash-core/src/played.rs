//! Copies of songs as they were played from an add-on, kept so the song plays again with no connection and without
//! asking the add-on (whose allowance is the listener's). The copy is written while the song streams (the core relays
//! the stream to the engine and keeps what passes), never fetched a second time. Copies live in a folder of their own
//! next to the library, up to a size the listener chooses; the least recently played go first.

use std::path::Path;

use rusqlite::{OptionalExtension, params};

use crate::{Library, Result};

/// The setting that holds how much room copies may take, in megabytes ("0": none are kept).
pub const CAP_SETTING: &str = "played.cap_mb";
/// How much room copies take unless the listener says otherwise.
pub const DEFAULT_CAP_MB: u64 = 2048;

/// A song's copy: its file (a name in the copies' folder), size, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedCopy {
    pub file: String,
    pub bytes: u64,
    pub tier: Option<String>,
    pub addon: Option<String>,
}

/// How many songs are kept, how much room they take, and how much they may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PlayedRoom {
    pub songs: u64,
    pub bytes: u64,
    pub cap_mb: u64,
}

impl Library {
    /// How much room copies may take, in bytes (0: none are kept).
    pub fn played_cap_bytes(&self) -> Result<u64> {
        Ok(self.played_cap_mb()?.saturating_mul(1024 * 1024))
    }

    fn played_cap_mb(&self) -> Result<u64> {
        Ok(self
            .setting(CAP_SETTING)?
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_CAP_MB))
    }

    /// The copy of the song `id`, when one is kept; it is noted as used now.
    pub fn played_copy(&mut self, id: i64, now_ms: i64) -> Result<Option<PlayedCopy>> {
        let copy = self
            .conn
            .prepare_cached(
                "SELECT file, bytes, tier, addon FROM played_copies WHERE track_id = ?1",
            )?
            .query_row([id], |r| {
                Ok(PlayedCopy {
                    file: r.get(0)?,
                    bytes: r.get::<_, i64>(1)? as u64,
                    tier: r.get(2)?,
                    addon: r.get(3)?,
                })
            })
            .optional()?;
        if copy.is_some() {
            self.conn.execute(
                "UPDATE played_copies SET used_at_ms = ?2 WHERE track_id = ?1",
                params![id, now_ms],
            )?;
        }
        Ok(copy)
    }

    /// Keeps `file` as the song `id`'s copy (in place of one kept before), then makes room: the least recently played
    /// copies go until all fit in the listener's room. Answers the files no longer kept (to be deleted), the old copy
    /// of this song among them when it was another file. `false` in the answer's first place: the song is gone from
    /// the library, or copies are not kept, and `file` is not kept either (it is in the answer).
    pub fn keep_played_copy(
        &mut self,
        id: i64,
        file: &str,
        bytes: u64,
        tier: Option<&str>,
        addon: Option<&str>,
        now_ms: i64,
    ) -> Result<(bool, Vec<String>)> {
        let cap = self.played_cap_bytes()?;
        let tx = self.conn.transaction()?;
        let exists: bool = tx
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM tracks WHERE id = ?1)")?
            .query_row([id], |r| r.get(0))?;
        if !exists || cap == 0 || bytes > cap {
            return Ok((false, vec![file.to_string()]));
        }
        let mut gone: Vec<String> = tx
            .prepare_cached("SELECT file FROM played_copies WHERE track_id = ?1 AND file != ?2")?
            .query_map(params![id, file], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        tx.execute(
            "INSERT INTO played_copies(track_id, file, bytes, tier, addon, kept_at_ms, used_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(track_id) DO UPDATE SET file = excluded.file, bytes = excluded.bytes, tier = excluded.tier,
                 addon = excluded.addon, kept_at_ms = excluded.kept_at_ms, used_at_ms = excluded.used_at_ms",
            params![id, file, bytes as i64, tier, addon, now_ms],
        )?;
        let mut total: u64 = tx
            .prepare_cached("SELECT COALESCE(SUM(bytes), 0) FROM played_copies")?
            .query_row([], |r| r.get::<_, i64>(0))? as u64;
        if total > cap {
            let oldest: Vec<(i64, String, u64)> = tx
                .prepare_cached(
                    "SELECT track_id, file, bytes FROM played_copies WHERE track_id != ?1 ORDER BY used_at_ms, track_id",
                )?
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64)))?
                .collect::<rusqlite::Result<_>>()?;
            for (other, other_file, other_bytes) in oldest {
                if total <= cap {
                    break;
                }
                tx.execute("DELETE FROM played_copies WHERE track_id = ?1", [other])?;
                gone.push(other_file);
                total -= other_bytes;
            }
        }
        tx.commit()?;
        Ok((true, gone))
    }

    /// Forgets the song `id`'s copy (its file is gone, or the song became a file of its own). Answers the file.
    pub fn drop_played_copy(&mut self, id: i64) -> Result<Option<String>> {
        let file: Option<String> = self
            .conn
            .prepare_cached("SELECT file FROM played_copies WHERE track_id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        self.conn
            .execute("DELETE FROM played_copies WHERE track_id = ?1", [id])?;
        Ok(file)
    }

    /// How many songs are kept and the room they take.
    pub fn played_room(&self) -> Result<PlayedRoom> {
        let (songs, bytes) = self
            .conn
            .prepare_cached("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM played_copies")?
            .query_row([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })?;
        Ok(PlayedRoom {
            songs,
            bytes,
            cap_mb: self.played_cap_mb()?,
        })
    }

    /// Sets the room copies may take, and makes room at once. Answers the files no longer kept.
    pub fn set_played_cap(&mut self, cap_mb: u64) -> Result<Vec<String>> {
        self.set_setting(CAP_SETTING, &cap_mb.to_string())?;
        let cap = cap_mb.saturating_mul(1024 * 1024);
        let rows: Vec<(i64, String, u64)> = self
            .conn
            .prepare_cached("SELECT track_id, file, bytes FROM played_copies ORDER BY used_at_ms DESC, track_id DESC")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64)))?
            .collect::<rusqlite::Result<_>>()?;
        // The most recently played stay, as many as fit.
        let mut used = 0u64;
        let mut gone = Vec::new();
        for (id, file, bytes) in rows {
            if used + bytes <= cap {
                used += bytes;
            } else {
                self.conn
                    .execute("DELETE FROM played_copies WHERE track_id = ?1", [id])?;
                gone.push(file);
            }
        }
        Ok(gone)
    }

    /// Forgets every copy. Answers their files.
    pub fn clear_played_copies(&mut self) -> Result<Vec<String>> {
        let files: Vec<String> = self
            .conn
            .prepare_cached("SELECT file FROM played_copies")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.conn.execute("DELETE FROM played_copies", [])?;
        Ok(files)
    }

    /// Every file the library keeps as a copy (to tell a stray file in the folder from a kept one).
    pub fn played_files(&self) -> Result<Vec<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT file FROM played_copies")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
}

/// Deletes `files` from the copies' folder `folder` (only plain names in it: never a path that leads elsewhere).
pub fn delete_files(folder: &Path, files: &[String]) -> Vec<String> {
    let mut left = Vec::new();
    for file in files {
        let plain = Path::new(file)
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(file));
        if plain {
            let path = folder.join(file);
            if std::fs::remove_file(&path).is_err() && path.exists() {
                left.push(file.clone());
            }
        }
    }
    left
}

/// Whether `name` is one the copies are written under: `<song>.<kind>`, or `<song>-<n>.part` while one comes in.
fn ours(name: &str) -> bool {
    let Some((stem, ending)) = name.rsplit_once('.') else {
        return false;
    };
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if ending == "part" {
        return stem.split_once('-').is_some_and(|(song, n)| {
            number(song) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_hexdigit())
        });
    }
    number(stem) && !ending.is_empty() && ending.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Deletes from `folder` what the library does not keep: copies cut short (`.part`) and copies it no longer knows.
/// Only files named the way copies are named are touched (the folder may hold something else).
pub fn tidy(folder: &Path, kept: &[String]) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| t.is_file()) && ours(&name) && !kept.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidying_deletes_only_what_copies_leave_behind() {
        let folder = std::env::temp_dir().join(format!("stash-played-tidy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        for name in [
            "12.flac",
            "13.flac",
            "12-1f.part",
            "My Song.flac",
            "notes.txt",
            "7.flac.bak",
        ] {
            std::fs::write(folder.join(name), b"x").unwrap();
        }
        tidy(&folder, &["12.flac".to_string()]);
        let mut left: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["12.flac", "7.flac.bak", "My Song.flac", "notes.txt"]);
        // And a name that is not a plain name in the folder is never deleted.
        delete_files(
            &folder,
            &["../notes.txt".to_string(), "12.flac".to_string()],
        );
        assert!(!folder.join("12.flac").exists() && folder.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&folder);
    }
}
