//! What the library keeps of the listener's cloud storage: the accounts, the folders picked as music, and the song each
//! audio file in them is.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::{File, Kind};
use crate::Result;
use crate::library::{Library, NewTrack, Writer};

/// A connected account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub id: i64,
    pub kind: Kind,
    pub name: String,
    pub email: Option<String>,
    /// Stash may read it: the provider still takes its sign-in.
    pub signed_in: bool,
    pub connected_at_ms: i64,
}

/// A folder picked as music, and where its scan stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Folder {
    pub id: i64,
    pub account: i64,
    #[serde(skip)]
    pub item_id: String,
    /// Where it is, as it was when it was picked ("/Music").
    pub path: String,
    pub added_at_ms: i64,
    #[serde(skip)]
    pub cursor: Option<String>,
    /// The cursor is the mark to ask what changed since (a pass was whole).
    #[serde(skip)]
    pub delta: bool,
    #[serde(skip)]
    pub generation: i64,
    /// When its last scan ended well, and what went wrong with the last one, if it did.
    pub scanned_at_ms: Option<i64>,
    pub trouble: Option<String>,
    /// How many songs its files are.
    pub songs: u64,
}

/// What the library knows of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    /// Its song; none once the listener took the song out of the library.
    pub track: Option<i64>,
    pub version: Option<String>,
    pub size: u64,
}

/// What a file's audio is: (codec, sample rate, bit depth).
pub type Audio = (Option<String>, Option<u32>, Option<u32>);

/// What a file came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filed {
    pub track: Option<i64>,
    /// Its song is new to the library.
    pub added: bool,
}

/// Which files are gone.
#[derive(Debug, Clone, Copy)]
pub enum Leaving<'a> {
    /// These files of the account.
    Ids(&'a [String]),
    /// The file or folder at this lower-case path, and all below it, but for the files `except` (seen again where they
    /// are now: a file moved is removed from where it was).
    Under { path: &'a str, except: &'a [String] },
    /// The folder's files a whole pass of `generation` did not see.
    Stale { folder: i64, generation: i64 },
    /// All of the folder's files.
    Folder(i64),
    /// All of the account's files.
    Account,
}

fn account_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Account>> {
    let provider: String = r.get(1)?;
    Ok(Kind::from_id(&provider).map(|kind| Account {
        id: r.get(0).unwrap_or_default(),
        kind,
        name: r.get(2).unwrap_or_default(),
        email: r.get(3).unwrap_or_default(),
        signed_in: r.get::<_, Option<String>>(4).unwrap_or_default().is_some(),
        connected_at_ms: r.get(5).unwrap_or_default(),
    }))
}

const ACCOUNT: &str =
    "SELECT id, provider, name, email, token, connected_at_ms FROM cloud_accounts";
const FOLDER: &str =
    "SELECT f.id, f.account_id, f.item_id, f.path, f.added_at_ms, f.cursor, f.delta, f.generation,
            f.scanned_at_ms, f.trouble,
            (SELECT COUNT(*) FROM cloud_items i WHERE i.folder_id = f.id AND i.track_id IS NOT NULL)
     FROM cloud_folders f";

fn folder_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get(0)?,
        account: r.get(1)?,
        item_id: r.get(2)?,
        path: r.get(3)?,
        added_at_ms: r.get(4)?,
        cursor: r.get(5)?,
        delta: r.get(6)?,
        generation: r.get(7)?,
        scanned_at_ms: r.get(8)?,
        trouble: r.get(9)?,
        songs: r.get::<_, i64>(10)? as u64,
    })
}

impl Library {
    /// The connected accounts, oldest first.
    pub fn cloud_accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{ACCOUNT} ORDER BY id"))?;
        let rows = stmt
            .query_map([], account_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().flatten().collect())
    }

    pub fn cloud_account(&self, id: i64) -> Result<Option<Account>> {
        Ok(self
            .conn
            .prepare_cached(&format!("{ACCOUNT} WHERE id = ?1"))?
            .query_row([id], account_row)
            .optional()?
            .flatten())
    }

    /// The account's sign-in (its refresh token): not for showing.
    pub fn cloud_token(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT token FROM cloud_accounts WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .flatten())
    }

    /// Keeps a signed-in account: one connected before is signed in again (its folders and songs stay).
    pub fn keep_cloud_account(
        &mut self,
        kind: Kind,
        remote_id: &str,
        name: &str,
        email: Option<&str>,
        token: &str,
        now_ms: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO cloud_accounts(provider, remote_id, name, email, token, connected_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(provider, remote_id) DO UPDATE SET name = excluded.name, email = excluded.email,
                                                            token = excluded.token",
            params![kind.id(), remote_id, name, email, token, now_ms],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM cloud_accounts WHERE provider = ?1 AND remote_id = ?2",
            params![kind.id(), remote_id],
            |r| r.get(0),
        )?)
    }

    /// A new sign-in for the account, or none: the provider no longer takes it (the listener signs in again).
    pub fn set_cloud_token(&mut self, id: i64, token: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE cloud_accounts SET token = ?2 WHERE id = ?1",
            params![id, token],
        )?;
        Ok(())
    }

    pub fn cloud_folders(&self, account: i64) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{FOLDER} WHERE f.account_id = ?1 ORDER BY f.path, f.id"
        ))?;
        Ok(stmt
            .query_map([account], folder_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn cloud_folder(&self, id: i64) -> Result<Option<Folder>> {
        Ok(self
            .conn
            .prepare_cached(&format!("{FOLDER} WHERE f.id = ?1"))?
            .query_row([id], folder_row)
            .optional()?)
    }

    /// Picks the account's folder `item_id` (at `path`) as music; one picked already stays as it is.
    pub fn add_cloud_folder(
        &mut self,
        account: i64,
        item_id: &str,
        path: &str,
        now_ms: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO cloud_folders(account_id, item_id, path, added_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![account, item_id, path, now_ms],
        )?;
        Ok(self.conn.query_row(
            "SELECT id FROM cloud_folders WHERE account_id = ?1 AND item_id = ?2",
            params![account, item_id],
            |r| r.get(0),
        )?)
    }

    /// Where the folder's scan stands.
    pub fn cloud_folder_at(
        &mut self,
        folder: i64,
        cursor: Option<&str>,
        delta: bool,
        generation: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cloud_folders SET cursor = ?2, delta = ?3, generation = ?4 WHERE id = ?1",
            params![folder, cursor, delta, generation],
        )?;
        Ok(())
    }

    /// A scan of the folder ended: well (`trouble` none), or not.
    pub fn cloud_folder_done(
        &mut self,
        folder: i64,
        at_ms: i64,
        trouble: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cloud_folders SET trouble = ?3,
                    scanned_at_ms = CASE WHEN ?3 IS NULL THEN ?2 ELSE scanned_at_ms END
             WHERE id = ?1",
            params![folder, at_ms, trouble],
        )?;
        Ok(())
    }

    pub fn cloud_known(&self, account: i64, item: &str) -> Result<Option<Known>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT track_id, version, size FROM cloud_items WHERE account_id = ?1 AND item_id = ?2",
            )?
            .query_row(params![account, item], |r| {
                Ok(Known {
                    track: r.get(0)?,
                    version: r.get(1)?,
                    size: r.get::<_, i64>(2)? as u64,
                })
            })
            .optional()?)
    }

    /// A file seen as it was (its bytes unchanged): it is stamped with the pass, and kept where it is now.
    pub fn cloud_seen(
        &mut self,
        account: i64,
        file: &File,
        folder: i64,
        generation: i64,
    ) -> Result<()> {
        self.conn
            .prepare_cached(
                "UPDATE cloud_items SET generation = ?4, folder_id = ?3, name = ?5, path_lower = COALESCE(?6, path_lower)
                 WHERE account_id = ?1 AND item_id = ?2",
            )?
            .execute(params![account, file.id, folder, generation, file.name, file.path_lower])?;
        Ok(())
    }

    /// Files the song `file` holds (`track`, read from it; `art`, the address of its picture): a file the library
    /// knows keeps its song, which gains what the file says of its audio; a new one is the song the library has
    /// already (by its ISRC or its words), which gains what it lacked, or a new song. Either way, what the provider
    /// calls the file is kept as the song's.
    #[allow(clippy::too_many_arguments)]
    pub fn file_cloud_song(
        &mut self,
        kind: Kind,
        account: i64,
        folder: i64,
        file: &File,
        track: &NewTrack,
        art: Option<&str>,
        generation: i64,
    ) -> Result<Filed> {
        let tx = self.conn.transaction()?;
        let known: Option<(Option<i64>, bool)> = tx
            .prepare_cached(
                "SELECT track_id, added FROM cloud_items WHERE account_id = ?1 AND item_id = ?2",
            )?
            .query_row(params![account, file.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let filed = match known {
            Some((song, added)) => {
                if let Some(id) = song {
                    tx.prepare_cached(
                        "UPDATE tracks SET duration_ms = CASE WHEN ?2 > 0 THEN ?2 ELSE duration_ms END,
                                           codec = COALESCE(?3, codec), sample_rate = COALESCE(?4, sample_rate),
                                           bit_depth = COALESCE(?5, bit_depth)
                         WHERE id = ?1 AND origin = 'described'",
                    )?
                    .execute(params![
                        id,
                        track.duration_ms as i64,
                        track.codec,
                        track.sample_rate,
                        track.bit_depth
                    ])?;
                }
                Filed { track: song, added }
            }
            None => {
                let words = crate::identity::words_key(&track.title, &track.artist);
                match crate::import::same_song(&tx, &words, track.isrc.as_deref())? {
                    Some(id) => {
                        tx.prepare_cached(
                            "UPDATE tracks SET isrc = COALESCE(isrc, ?2),
                                               duration_ms = CASE WHEN duration_ms = 0 THEN ?3 ELSE duration_ms END,
                                               art_url = COALESCE(art_url, ?4)
                             WHERE id = ?1",
                        )?
                        .execute(params![id, track.isrc, track.duration_ms as i64, art])?;
                        tx.prepare_cached(
                            "UPDATE tracks SET codec = COALESCE(codec, ?2), sample_rate = COALESCE(sample_rate, ?3),
                                               bit_depth = COALESCE(bit_depth, ?4)
                             WHERE id = ?1 AND origin = 'described'",
                        )?
                        .execute(params![id, track.codec, track.sample_rate, track.bit_depth])?;
                        Filed {
                            track: Some(id),
                            added: false,
                        }
                    }
                    None => {
                        let id = Writer::new(&tx)?.insert(track)?;
                        tx.prepare_cached("UPDATE tracks SET art_url = ?2 WHERE id = ?1")?
                            .execute(params![id, art])?;
                        Filed {
                            track: Some(id),
                            added: true,
                        }
                    }
                }
            }
        };
        if let Some(id) = filed.track {
            tx.prepare_cached(
                "INSERT INTO track_refs(track_id, source, ref) VALUES (?1, ?2, ?3)
                 ON CONFLICT(track_id, source) DO NOTHING",
            )?
            .execute(params![id, kind.id(), file.id])?;
        }
        tx.prepare_cached(
            "INSERT INTO cloud_items(account_id, item_id, folder_id, track_id, name, path_lower, size, version, codec,
                                     sample_rate, bit_depth, generation, added)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(account_id, item_id) DO UPDATE SET
                folder_id = excluded.folder_id, name = excluded.name, path_lower = excluded.path_lower,
                size = excluded.size, version = excluded.version, codec = excluded.codec,
                sample_rate = excluded.sample_rate, bit_depth = excluded.bit_depth, generation = excluded.generation",
        )?
        .execute(params![
            account,
            file.id,
            folder,
            filed.track,
            file.name,
            file.path_lower,
            file.size as i64,
            file.version,
            track.codec,
            track.sample_rate,
            track.bit_depth,
            generation,
            filed.added
        ])?;
        tx.commit()?;
        Ok(filed)
    }

    /// Files gone from the account (or no longer music to the listener): each is forgotten, its song no longer plays
    /// from it, and a song the scan brought that the listener did nothing with (liked, put in a playlist, saved)
    /// leaves the library. Answers how many songs left.
    pub fn cloud_items_leave(
        &mut self,
        kind: Kind,
        account: i64,
        leaving: Leaving<'_>,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut gone: Vec<(String, Option<i64>, bool)> = Vec::new();
        {
            let mut rows = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<()> {
                let mut stmt = tx.prepare_cached(sql)?;
                let found = stmt
                    .query_map(p, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                gone.extend(found);
                Ok(())
            };
            const ITEM: &str = "SELECT item_id, track_id, added FROM cloud_items";
            match leaving {
                Leaving::Ids(ids) => {
                    for id in ids {
                        rows(
                            &format!("{ITEM} WHERE account_id = ?1 AND item_id = ?2"),
                            &[&account, id],
                        )?;
                    }
                }
                Leaving::Under { path, except } => {
                    let path = path.trim_end_matches('/').to_string();
                    let below = format!("{path}/");
                    // Below by its text, not by LIKE (a path may hold % or _).
                    rows(
                        &format!(
                            "{ITEM} WHERE account_id = ?1 AND (path_lower = ?2 OR substr(path_lower, 1, length(?3)) = ?3)"
                        ),
                        &[&account, &path, &below],
                    )?;
                    gone.retain(|(item, _, _)| !except.contains(item));
                }
                Leaving::Stale { folder, generation } => rows(
                    &format!("{ITEM} WHERE folder_id = ?1 AND generation < ?2"),
                    &[&folder, &generation],
                )?,
                Leaving::Folder(folder) => {
                    rows(&format!("{ITEM} WHERE folder_id = ?1"), &[&folder])?
                }
                Leaving::Account => rows(&format!("{ITEM} WHERE account_id = ?1"), &[&account])?,
            }
        }
        let mut candidates = Vec::new();
        for (item, track, added) in &gone {
            tx.prepare_cached("DELETE FROM cloud_items WHERE account_id = ?1 AND item_id = ?2")?
                .execute(params![account, item])?;
            let Some(track) = track else {
                continue;
            };
            // Another file of the same provider that is this song: the song plays from that one now.
            let other: Option<String> = tx
                .prepare_cached(
                    "SELECT i.item_id FROM cloud_items i JOIN cloud_accounts a ON a.id = i.account_id
                     WHERE i.track_id = ?1 AND a.provider = ?2 LIMIT 1",
                )?
                .query_row(params![track, kind.id()], |r| r.get(0))
                .optional()?;
            match other {
                Some(other) => {
                    tx.prepare_cached(
                        "UPDATE track_refs SET ref = ?3 WHERE track_id = ?1 AND source = ?2",
                    )?
                    .execute(params![track, kind.id(), other])?;
                }
                None => {
                    tx.prepare_cached(
                        "DELETE FROM track_refs WHERE track_id = ?1 AND source = ?2 AND ref = ?3",
                    )?
                    .execute(params![track, kind.id(), item])?;
                    if *added {
                        candidates.push(*track);
                    }
                }
            }
        }
        // Of the songs the scan brought, those the listener did nothing with.
        let mut untouched = Vec::new();
        for track in candidates {
            let kept: bool = tx
                .prepare_cached(
                    "SELECT t.origin <> 'described' OR t.liked_at_ms IS NOT NULL
                            OR EXISTS (SELECT 1 FROM downloads d WHERE d.track_id = t.id)
                            OR EXISTS (SELECT 1 FROM cloud_items i WHERE i.track_id = t.id)
                     FROM tracks t WHERE t.id = ?1",
                )?
                .query_row([track], |r| r.get(0))
                .optional()?
                .unwrap_or(true);
            if !kept {
                untouched.push(track);
            }
        }
        tx.commit()?;
        // A song in a playlist stays (`removable`).
        self.remove_songs(&untouched)
    }

    /// The accounts of `kind` Stash may read, those that hold the file `item` first: where it is played from.
    pub fn cloud_accounts_with(&self, kind: Kind, item: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT a.id FROM cloud_accounts a WHERE a.provider = ?1 AND a.token IS NOT NULL
             ORDER BY EXISTS (SELECT 1 FROM cloud_items i WHERE i.account_id = a.id AND i.item_id = ?2) DESC, a.id",
        )?;
        Ok(stmt
            .query_map(params![kind.id(), item], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?)
    }

    /// What the file `item` holds, as its scan read it.
    pub fn cloud_audio(&self, kind: Kind, item: &str) -> Result<Option<Audio>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT i.codec, i.sample_rate, i.bit_depth FROM cloud_items i
                 JOIN cloud_accounts a ON a.id = i.account_id WHERE a.provider = ?1 AND i.item_id = ?2 LIMIT 1",
            )?
            .query_row(params![kind.id(), item], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?)
    }

    /// Takes the folder off the music folders: its files' songs leave as when the files go. Answers how many left.
    pub fn forget_cloud_folder(&mut self, id: i64) -> Result<usize> {
        let Some(folder) = self.cloud_folder(id)? else {
            return Ok(0);
        };
        let Some(account) = self.cloud_account(folder.account)? else {
            return Ok(0);
        };
        let left = self.cloud_items_leave(account.kind, account.id, Leaving::Folder(id))?;
        self.conn
            .execute("DELETE FROM cloud_folders WHERE id = ?1", [id])?;
        Ok(left)
    }

    /// Disconnects the account: its sign-in is forgotten, its folders with it, and its files' songs leave as when the
    /// files go. Answers how many left.
    pub fn forget_cloud_account(&mut self, id: i64) -> Result<usize> {
        let Some(account) = self.cloud_account(id)? else {
            return Ok(0);
        };
        let left = self.cloud_items_leave(account.kind, id, Leaving::Account)?;
        self.conn
            .execute("DELETE FROM cloud_accounts WHERE id = ?1", [id])?;
        Ok(left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Origin;

    fn song(title: &str, artist: &str) -> NewTrack {
        NewTrack {
            title: title.into(),
            artist: artist.into(),
            album_artist: None,
            album: Some("Album".into()),
            year: None,
            disc_no: 1,
            track_no: 1,
            duration_ms: 200_000,
            isrc: None,
            origin: Origin::Described,
            addon_id: None,
            locator: String::new(),
            codec: Some("flac".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            added_at: 1,
            file_size: None,
            file_mtime: None,
        }
    }

    fn file(id: &str, path: &str) -> File {
        File {
            id: id.into(),
            name: format!("{id}.flac"),
            size: 10,
            version: Some("1".into()),
            path_lower: Some(path.into()),
            ..Default::default()
        }
    }

    fn refs(library: &Library, track: i64) -> Vec<(String, String)> {
        library.describe(track).unwrap().unwrap().refs
    }

    #[test]
    fn a_cloud_file_is_a_song_that_goes_with_it_unless_the_listener_kept_it() {
        let mut library = Library::open_in_memory().unwrap();
        let account = library
            .keep_cloud_account(Kind::Dropbox, "dbid:1", "Me", None, "refresh", 5)
            .unwrap();
        // Signing in again is the same account.
        assert_eq!(
            library
                .keep_cloud_account(Kind::Dropbox, "dbid:1", "Me", Some("m@x"), "refresh2", 6)
                .unwrap(),
            account
        );
        assert_eq!(
            library.cloud_token(account).unwrap().as_deref(),
            Some("refresh2")
        );
        let folder = library
            .add_cloud_folder(account, "id:root", "/Music", 7)
            .unwrap();
        assert_eq!(
            library
                .add_cloud_folder(account, "id:root", "/Music", 8)
                .unwrap(),
            folder
        );

        let a = library
            .file_cloud_song(
                Kind::Dropbox,
                account,
                folder,
                &file("id:a", "/music/a.flac"),
                &song("Alpha", "Band"),
                Some("/api/art/cloud/x"),
                1,
            )
            .unwrap();
        assert!(a.added);
        let a = a.track.unwrap();
        assert_eq!(
            refs(&library, a),
            vec![("dropbox".to_string(), "id:a".to_string())]
        );
        assert_eq!(library.cloud_folders(account).unwrap()[0].songs, 1);
        assert_eq!(
            library.cloud_accounts_with(Kind::Dropbox, "id:a").unwrap(),
            vec![account]
        );
        assert_eq!(
            library.cloud_audio(Kind::Dropbox, "id:a").unwrap(),
            Some((Some("flac".into()), Some(44_100), Some(16)))
        );

        // A song the library had is that song: it gains the file, and stays when the file goes.
        let b_file = file("id:b", "/music/sub/b.flac");
        let mut had = song("Bravo", "Band");
        had.codec = None;
        let had_id = {
            let tx = library.conn.transaction().unwrap();
            let id = Writer::new(&tx).unwrap().insert(&had).unwrap();
            tx.commit().unwrap();
            id
        };
        let b = library
            .file_cloud_song(
                Kind::Dropbox,
                account,
                folder,
                &b_file,
                &song("Bravo", "Band"),
                None,
                1,
            )
            .unwrap();
        assert_eq!(
            b,
            Filed {
                track: Some(had_id),
                added: false
            }
        );

        // A liked song the scan brought stays when its file goes; one the listener did nothing with leaves.
        let c = library
            .file_cloud_song(
                Kind::Dropbox,
                account,
                folder,
                &file("id:c", "/music/sub/c.flac"),
                &song("Charlie", "Band"),
                None,
                1,
            )
            .unwrap()
            .track
            .unwrap();
        library
            .conn
            .execute("UPDATE tracks SET liked_at_ms = 1 WHERE id = ?1", [c])
            .unwrap();
        let left = library
            .cloud_items_leave(
                Kind::Dropbox,
                account,
                Leaving::Under {
                    path: "/music/sub",
                    except: &[],
                },
            )
            .unwrap();
        assert_eq!(left, 0);
        assert!(refs(&library, had_id).is_empty());
        assert!(refs(&library, c).is_empty());
        assert!(library.describe(c).unwrap().is_some());

        // A whole pass that did not see a file: its song leaves.
        library
            .cloud_seen(account, &file("id:a", "/music/a.flac"), folder, 1)
            .unwrap();
        assert_eq!(
            library
                .cloud_items_leave(
                    Kind::Dropbox,
                    account,
                    Leaving::Stale {
                        folder,
                        generation: 2
                    }
                )
                .unwrap(),
            1
        );
        assert!(library.describe(a).unwrap().is_none());

        // Disconnecting forgets the sign-in, the folders and the files.
        library
            .file_cloud_song(
                Kind::Dropbox,
                account,
                folder,
                &file("id:d", "/music/d.flac"),
                &song("Delta", "Band"),
                None,
                2,
            )
            .unwrap();
        assert_eq!(library.forget_cloud_account(account).unwrap(), 1);
        assert!(library.cloud_accounts().unwrap().is_empty());
        let items: i64 = library
            .conn
            .query_row("SELECT COUNT(*) FROM cloud_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(items, 0);
    }

    #[test]
    fn a_song_the_listener_took_out_is_not_brought_back_by_its_file() {
        let mut library = Library::open_in_memory().unwrap();
        let account = library
            .keep_cloud_account(Kind::OneDrive, "me", "Me", None, "r", 1)
            .unwrap();
        let folder = library
            .add_cloud_folder(account, "F!1", "/Music", 1)
            .unwrap();
        let f = file("S!1", "");
        let track = library
            .file_cloud_song(
                Kind::OneDrive,
                account,
                folder,
                &f,
                &song("Echo", "Band"),
                None,
                1,
            )
            .unwrap()
            .track
            .unwrap();
        assert_eq!(library.remove_songs(&[track]).unwrap(), 1);
        assert_eq!(
            library.cloud_known(account, "S!1").unwrap().unwrap().track,
            None
        );
        // The file changes: still no song.
        let again = library
            .file_cloud_song(
                Kind::OneDrive,
                account,
                folder,
                &f,
                &song("Echo", "Band"),
                None,
                2,
            )
            .unwrap();
        assert_eq!(again.track, None);
        // Signed out: no account to play from.
        library.set_cloud_token(account, None).unwrap();
        assert!(
            library
                .cloud_accounts_with(Kind::OneDrive, "S!1")
                .unwrap()
                .is_empty()
        );
        assert!(!library.cloud_accounts().unwrap()[0].signed_in);
    }
}
