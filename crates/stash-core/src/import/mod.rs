//! Bringing a library in from another Stash: the Android app or the web player.
//!
//! What comes in are descriptions of songs (title, artist, album, length, ISRC, and the ids other sources know them
//! by), with the listener's likes, playlists and plays. No audio travels. A song the library already has, as a file
//! or from an earlier import, is recognised by [`crate::identity`] and gains what the import knows about it; a song
//! it does not have becomes a row of its own, which plays through the listener's add-ons.
//!
//! Importing adds. It never removes, reorders or un-likes, so the same file can be imported twice, and a newer one
//! after an older one.

pub mod web_library;

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::identity::{isrcs_agree, words_key};
use crate::library::{Library, NewTrack, Origin, Writer};

/// A song as an import describes it. Already cleaned by the reader that made it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// The recording's own length, when the writer could vouch for it.
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    pub explicit: Option<bool>,
    /// What other sources call this song: (source id, that source's id for it).
    pub refs: Vec<(String, String)>,
    /// A cover on the web (https).
    pub art_url: Option<String>,
    pub added_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Playlist {
    /// The writer's own id for the playlist, the same in every file it writes: importing again merges into it.
    pub external_id: String,
    pub name: String,
    pub items: Vec<Song>,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    /// Set for a shared mix someone else owns: its share id.
    pub follow_id: Option<String>,
}

/// Everything one file holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Incoming {
    /// What kind of file it was: "web-library" or "android-backup".
    pub source: &'static str,
    /// Who wrote it, in the file's own words.
    pub generator: Option<String>,
    pub likes: Vec<(Song, i64)>,
    pub playlists: Vec<Playlist>,
    /// Plays that counted, with when each started.
    pub plays: Vec<(Song, i64)>,
    /// Songs of the library that are in none of the above.
    pub songs: Vec<Song>,
}

/// What an import changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Songs the library did not have.
    pub songs_added: usize,
    /// Songs it had already (as a file, or from before).
    pub songs_already: usize,
    pub likes_added: usize,
    pub likes_already: usize,
    pub playlists_added: usize,
    /// Playlists that were here and gained the songs they lacked.
    pub playlists_merged: usize,
    pub playlist_songs_added: usize,
    pub plays_added: usize,
    pub plays_already: usize,
}

/// Why a file was not imported, in words for the listener.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("That file is not a Stash library file")]
    NotStash,
    #[error("That file comes from a newer Stash than this one")]
    Newer,
    #[error("That file is too big to be a Stash library file")]
    TooBig,
    #[error("{0}")]
    Unreadable(String),
    #[error(transparent)]
    Library(#[from] crate::Error),
}

impl From<rusqlite::Error> for ImportError {
    fn from(e: rusqlite::Error) -> Self {
        ImportError::Library(e.into())
    }
}

struct Merger<'t> {
    tx: &'t rusqlite::Transaction<'t>,
    writer: Writer<'t>,
    /// Songs met in this import, by ISRC or by words, to their rows.
    met: HashMap<String, i64>,
    /// Rows this import added.
    new_rows: HashSet<i64>,
    /// Rows whose plays changed.
    played: HashSet<i64>,
    now_s: i64,
    summary: Summary,
}

impl Merger<'_> {
    /// The library's row for `song`: the one it has, with what the import adds to it, or a new one.
    fn track(&mut self, song: &Song) -> crate::Result<i64> {
        let words = words_key(&song.title, &song.artist);
        let met_as = match &song.isrc {
            Some(isrc) => format!("isrc:{isrc}"),
            None => words.clone(),
        };
        if let Some(id) = self.met.get(&met_as) {
            return Ok(*id);
        }
        let mut found: Option<i64> = None;
        if let Some(isrc) = &song.isrc {
            found = self
                .tx
                .prepare_cached("SELECT id FROM tracks WHERE isrc = ?1 ORDER BY origin = 'file' DESC, id LIMIT 1")?
                .query_row([isrc], |r| r.get(0))
                .optional()?;
        }
        if found.is_none() {
            // The same words: one song, unless both have an ISRC and they differ. A file before a description.
            let mut same_words = self
                .tx
                .prepare_cached("SELECT id, isrc FROM tracks WHERE words_key = ?1 ORDER BY origin = 'file' DESC, id")?;
            let mut rows = same_words.query([&words])?;
            while let Some(row) = rows.next()? {
                let theirs: Option<String> = row.get(1)?;
                if isrcs_agree(theirs.as_deref(), song.isrc.as_deref()) {
                    found = Some(row.get(0)?);
                    break;
                }
            }
        }
        let id = match found {
            Some(id) => {
                // It gains what it lacked; nothing it has is changed.
                self.tx
                    .prepare_cached(
                        "UPDATE tracks SET isrc = COALESCE(isrc, ?2),
                                           duration_ms = CASE WHEN duration_ms = 0 THEN ?3 ELSE duration_ms END,
                                           explicit = COALESCE(explicit, ?4),
                                           art_url = COALESCE(art_url, ?5)
                         WHERE id = ?1",
                    )?
                    .execute(params![
                        id,
                        song.isrc,
                        song.duration_ms.unwrap_or(0) as i64,
                        song.explicit,
                        song.art_url
                    ])?;
                if !self.new_rows.contains(&id) {
                    self.summary.songs_already += 1;
                }
                id
            }
            None => {
                let id = self.writer.insert(&NewTrack {
                    title: song.title.clone(),
                    artist: song.artist.clone(),
                    album_artist: None,
                    album: song.album.clone(),
                    year: None,
                    disc_no: 1,
                    track_no: 0,
                    duration_ms: song.duration_ms.unwrap_or(0),
                    isrc: song.isrc.clone(),
                    origin: Origin::Described,
                    addon_id: None,
                    locator: String::new(),
                    codec: None,
                    sample_rate: None,
                    bit_depth: None,
                    added_at: song.added_at_ms.map(|ms| ms / 1000).unwrap_or(self.now_s),
                    file_size: None,
                    file_mtime: None,
                })?;
                self.tx
                    .prepare_cached("UPDATE tracks SET explicit = ?2, art_url = ?3 WHERE id = ?1")?
                    .execute(params![id, song.explicit, song.art_url])?;
                self.new_rows.insert(id);
                self.summary.songs_added += 1;
                id
            }
        };
        for (source, theirs) in &song.refs {
            self.tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO track_refs(track_id, source, ref) VALUES (?1, ?2, ?3)",
                )?
                .execute(params![id, source, theirs])?;
        }
        self.met.insert(met_as, id);
        Ok(id)
    }

    fn like(&mut self, song: &Song, liked_at_ms: i64) -> crate::Result<()> {
        let id = self.track(song)?;
        let changed = self
            .tx
            .prepare_cached(
                "UPDATE tracks SET liked_at_ms = ?2 WHERE id = ?1 AND liked_at_ms IS NULL",
            )?
            .execute(params![id, liked_at_ms])?;
        if changed > 0 {
            self.summary.likes_added += 1;
        } else {
            self.summary.likes_already += 1;
        }
        Ok(())
    }

    fn playlist(&mut self, playlist: &Playlist) -> crate::Result<()> {
        let here: Option<i64> = match &playlist.follow_id {
            Some(follow) => self
                .tx
                .prepare_cached(
                    "SELECT id FROM playlists WHERE follow_id = ?1 OR external_id = ?2 LIMIT 1",
                )?
                .query_row(params![follow, playlist.external_id], |r| r.get(0))
                .optional()?,
            None => self
                .tx
                .prepare_cached("SELECT id FROM playlists WHERE external_id = ?1")?
                .query_row([&playlist.external_id], |r| r.get(0))
                .optional()?,
        };
        let add_item =
            "INSERT INTO playlist_items(playlist_id, position, track_id) VALUES (?1, ?2, ?3)";
        match here {
            // A new one is added whole, in its order.
            None => {
                self.tx
                    .prepare_cached(
                        "INSERT INTO playlists(external_id, name, created_at_ms, updated_at_ms, follow_id, position)
                         VALUES (?1, ?2, ?3, ?4, ?5, (SELECT IFNULL(MAX(position), 0) + 1 FROM playlists))",
                    )?
                    .execute(params![
                        playlist.external_id,
                        playlist.name,
                        playlist.created_at_ms,
                        playlist.updated_at_ms,
                        playlist.follow_id
                    ])?;
                let id = self.tx.last_insert_rowid();
                for (position, song) in playlist.items.iter().enumerate() {
                    let track = self.track(song)?;
                    self.tx.prepare_cached(add_item)?.execute(params![
                        id,
                        position as i64,
                        track
                    ])?;
                }
                self.summary.playlists_added += 1;
            }
            // A shared mix someone else owns that is here already is left as it is.
            Some(_) if playlist.follow_id.is_some() => {}
            // One that is here gains the songs it does not have, at the end.
            Some(id) => {
                let mut has: HashSet<i64> = self
                    .tx
                    .prepare_cached("SELECT track_id FROM playlist_items WHERE playlist_id = ?1")?
                    .query_map([id], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut next: i64 = self.tx.query_row(
                    "SELECT IFNULL(MAX(position), -1) + 1 FROM playlist_items WHERE playlist_id = ?1",
                    [id],
                    |r| r.get(0),
                )?;
                for song in &playlist.items {
                    let track = self.track(song)?;
                    if has.insert(track) {
                        self.tx
                            .prepare_cached(add_item)?
                            .execute(params![id, next, track])?;
                        next += 1;
                        self.summary.playlist_songs_added += 1;
                    }
                }
                self.summary.playlists_merged += 1;
            }
        }
        Ok(())
    }

    fn play(&mut self, song: &Song, played_at_ms: i64) -> crate::Result<()> {
        let id = self.track(song)?;
        let added = self
            .tx
            .prepare_cached("INSERT OR IGNORE INTO plays(track_id, played_at_ms) VALUES (?1, ?2)")?
            .execute(params![id, played_at_ms])?;
        if added > 0 {
            self.summary.plays_added += 1;
            self.played.insert(id);
        } else {
            self.summary.plays_already += 1;
        }
        Ok(())
    }
}

impl Library {
    /// Merges `incoming` into the library, all of it or (on an error) none of it.
    pub fn import(&mut self, incoming: &Incoming) -> crate::Result<Summary> {
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let tx = self.conn.transaction()?;
        let summary = {
            let mut merger = Merger {
                tx: &tx,
                writer: Writer::new(&tx)?,
                met: HashMap::new(),
                new_rows: HashSet::new(),
                played: HashSet::new(),
                now_s,
                summary: Summary::default(),
            };
            for (song, liked_at_ms) in &incoming.likes {
                merger.like(song, *liked_at_ms)?;
            }
            for playlist in &incoming.playlists {
                merger.playlist(playlist)?;
            }
            for (song, played_at_ms) in &incoming.plays {
                merger.play(song, *played_at_ms)?;
            }
            for song in &incoming.songs {
                merger.track(song)?;
            }
            // A song's count of plays is the plays the library holds for it.
            let mut recount = tx.prepare(
                "UPDATE tracks SET play_count = (SELECT COUNT(*) FROM plays WHERE track_id = ?1),
                                   last_played_at = (SELECT MAX(played_at_ms) / 1000 FROM plays WHERE track_id = ?1)
                 WHERE id = ?1",
            )?;
            for id in &merger.played {
                recount.execute([id])?;
            }
            merger.summary
        };
        tx.commit()?;
        Ok(summary)
    }
}
