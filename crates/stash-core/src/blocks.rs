//! Songs and artists the listener chose not to play ("Don't play this"). Stash never plays them on its own: autoplay does
//! not pick them, and a list played through leaves them out (`stashd`). A song the listener plays by itself still plays.
//!
//! A song is kept by its words ([`words_key`]), so every copy of it counts: a file, an add-on's, a song another source
//! named. An artist is kept by its folded name ([`fold`]), the second half of every one of its songs' words.

use std::collections::HashSet;

use rusqlite::params;
use serde::Serialize;

use crate::Result;
use crate::identity::{fold, words_key};
use crate::library::Library;

/// SQL that is true when track `t` is one the listener chose not to play: its words, or its artist's name, are kept in
/// `blocked`. A folded title holds no bar, so the words' first bar ends the title and the artist's name follows.
macro_rules! blocked {
    () => {
        "EXISTS (SELECT 1 FROM blocked b WHERE (b.kind = 'song' AND b.key = t.words_key)
                OR (b.kind = 'artist' AND b.key = substr(t.words_key, instr(t.words_key, '|') + 1)))"
    };
}
pub(crate) use blocked;

/// SQL naming how track `t` is chosen not to be played: "song" (its own words), else "artist", else NULL.
macro_rules! blocked_kind {
    () => {
        "(SELECT b.kind FROM blocked b WHERE (b.kind = 'song' AND b.key = t.words_key)
                OR (b.kind = 'artist' AND b.key = substr(t.words_key, instr(t.words_key, '|') + 1))
          ORDER BY b.kind = 'song' DESC LIMIT 1)"
    };
}
pub(crate) use blocked_kind;

/// What is not played: one song (every copy of it), or every song of an artist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Song,
    Artist,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::Song => "song",
            BlockKind::Artist => "artist",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "song" => Some(BlockKind::Song),
            "artist" => Some(BlockKind::Artist),
            _ => None,
        }
    }

    /// The key it is kept by; None when the words leave nothing to know it by (a name of only signs).
    /// Whether a song with these words is one the choice kept under `key` covers.
    pub fn covers(self, key: &str, title: &str, artist: &str) -> bool {
        self.key(title, artist).as_deref() == Some(key)
    }

    pub fn key(self, title: &str, artist: &str) -> Option<String> {
        let artist_key = fold(artist);
        match self {
            BlockKind::Song if fold(title).is_empty() || artist_key.is_empty() => None,
            BlockKind::Song => Some(words_key(title, artist)),
            BlockKind::Artist if artist_key.is_empty() => None,
            BlockKind::Artist => Some(artist_key),
        }
    }
}

/// One song or artist the listener chose not to play, with the words it was chosen by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocked {
    pub kind: &'static str,
    pub key: String,
    /// The song's title as it was shown; empty for an artist.
    pub title: String,
    pub artist: String,
    pub at_ms: i64,
}

impl Library {
    /// Chooses not to play a song (`kind` Song: `title` by `artist`) or every song of `artist`, or (`on` false) to
    /// play it again. Answers the key it is kept by; None when the words leave nothing to know it by.
    pub fn set_blocked(
        &mut self,
        kind: BlockKind,
        title: &str,
        artist: &str,
        on: bool,
        now_ms: i64,
    ) -> Result<Option<String>> {
        let Some(key) = kind.key(title, artist) else {
            return Ok(None);
        };
        if on {
            let title = if kind == BlockKind::Song { title } else { "" };
            self.conn.execute(
                "INSERT INTO blocked(kind, key, title, artist, at_ms) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(kind, key) DO NOTHING",
                params![
                    kind.as_str(),
                    key,
                    crate::identity::clean_text(title, 300),
                    crate::identity::clean_text(artist, 300),
                    now_ms
                ],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM blocked WHERE kind = ?1 AND key = ?2",
                params![kind.as_str(), key],
            )?;
        }
        Ok(Some(key))
    }

    /// Every song and artist the listener chose not to play, the latest first.
    /// Plays a song or an artist again by the key it is kept under (what was kept of its words may be shorter than
    /// they were). Answers whether there was such a choice.
    pub fn unblock_key(&mut self, kind: BlockKind, key: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM blocked WHERE kind = ?1 AND key = ?2",
            params![kind.as_str(), key],
        )? > 0)
    }

    pub fn blocks(&self) -> Result<Vec<Blocked>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT kind, key, title, artist, at_ms FROM blocked ORDER BY at_ms DESC, kind, key",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let kind: String = r.get(0)?;
                Ok(Blocked {
                    kind: if kind == "artist" { "artist" } else { "song" },
                    key: r.get(1)?,
                    title: r.get(2)?,
                    artist: r.get(3)?,
                    at_ms: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Which of these tracks of the library the listener chose not to play.
    pub fn blocked_of(&self, ids: &[i64]) -> Result<HashSet<i64>> {
        if !self.anything_blocked()? {
            return Ok(HashSet::new());
        }
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            blocked!(),
            " FROM tracks t WHERE t.id = ?1"
        ))?;
        let mut out = HashSet::new();
        for &id in ids {
            let found: Option<bool> =
                rusqlite::OptionalExtension::optional(stmt.query_row([id], |r| r.get(0)))?;
            if found == Some(true) {
                out.insert(id);
            }
        }
        Ok(out)
    }

    /// Whether a song by its words (one that is not the library's: an add-on's) is one the listener chose not to play.
    pub fn words_blocked(&self, title: &str, artist: &str) -> Result<bool> {
        let artist_key = fold(artist);
        let song_key = words_key(title, artist);
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM blocked WHERE (kind = 'song' AND key = ?1) OR (kind = 'artist' AND key = ?2))",
            params![song_key, artist_key],
            |r| r.get(0),
        )?)
    }

    /// The library's songs a choice covers: every copy of a song's words, or every song of an artist's.
    pub fn ids_blocked_by(&self, kind: BlockKind, key: &str) -> Result<Vec<i64>> {
        let sql = match kind {
            BlockKind::Song => "SELECT id FROM tracks WHERE words_key = ?1",
            BlockKind::Artist => {
                "SELECT id FROM tracks WHERE substr(words_key, instr(words_key, '|') + 1) = ?1"
            }
        };
        let ids = self
            .conn
            .prepare_cached(sql)?
            .query_map([key], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    /// How each of these songs is chosen not to be played now: "song", "artist", or not at all.
    pub fn blocked_kinds(&self, ids: &[i64]) -> Result<Vec<(i64, Option<String>)>> {
        let mut stmt = self.conn.prepare_cached(concat!(
            "SELECT ",
            blocked_kind!(),
            " FROM tracks t WHERE t.id = ?1"
        ))?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            let kind: Option<Option<String>> =
                rusqlite::OptionalExtension::optional(stmt.query_row([id], |r| r.get(0)))?;
            if let Some(kind) = kind {
                out.push((id, kind));
            }
        }
        Ok(out)
    }

    /// Whether the listener chose not to play anything at all: most never do, and nothing is looked up for them.
    pub fn anything_blocked(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT EXISTS (SELECT 1 FROM blocked)", [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::Song;

    fn song(title: &str, artist: &str) -> Song {
        Song {
            title: title.into(),
            artist: artist.into(),
            ..Song::default()
        }
    }

    #[test]
    fn a_song_is_known_by_its_words_and_an_artist_by_its_name() {
        let mut library = Library::open_in_memory().unwrap();
        let ids = library
            .adopt_songs(&[
                song("Halo", "Beyoncé"),
                song("Halo (feat. Someone)", "BEYONCE"),
                song("Crazy in Love", "Beyonce"),
                song("Halo", "Other Artist"),
            ])
            .unwrap();
        assert!(library.blocked_of(&ids).unwrap().is_empty());

        // The song: every copy of its words, not another artist's song of that name.
        let key = library
            .set_blocked(BlockKind::Song, "Halo", "Beyoncé", true, 1)
            .unwrap();
        assert_eq!(key.as_deref(), Some("halo|beyonce"));
        let blocked = library.blocked_of(&ids).unwrap();
        assert_eq!(blocked, HashSet::from([ids[0], ids[1]]));
        assert!(library.words_blocked("HALO", "beyonce").unwrap());
        assert!(!library.words_blocked("Halo", "Other Artist").unwrap());

        // The artist: every song of that name, whatever case or accents.
        library
            .set_blocked(BlockKind::Artist, "", "beyonce", true, 2)
            .unwrap();
        assert_eq!(
            library.blocked_of(&ids).unwrap(),
            HashSet::from([ids[0], ids[1], ids[2]])
        );
        assert!(library.words_blocked("Single Ladies", "Beyoncé").unwrap());
        let listed = library.blocks().unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|b| (b.kind, b.title.as_str(), b.artist.as_str()))
                .collect::<Vec<_>>(),
            [("artist", "", "beyonce"), ("song", "Halo", "Beyoncé")]
        );

        // Played again: the song stays out while its artist does.
        library
            .set_blocked(BlockKind::Artist, "", "Beyoncé", false, 3)
            .unwrap();
        assert_eq!(
            library.blocked_of(&ids).unwrap(),
            HashSet::from([ids[0], ids[1]])
        );
        // Autoplay does not pick them.
        let picks = library
            .autoplay_picks(&[], &[], &HashSet::new(), 0, false, 10)
            .unwrap();
        assert!(
            !picks.contains(&ids[0]) && !picks.contains(&ids[1]),
            "{picks:?}"
        );
        assert!(
            picks.contains(&ids[2]) && picks.contains(&ids[3]),
            "{picks:?}"
        );
        // A name of only signs is not kept: it would stand for every such name.
        assert_eq!(
            library
                .set_blocked(BlockKind::Artist, "", "!!!", true, 4)
                .unwrap(),
            None
        );
    }
}
