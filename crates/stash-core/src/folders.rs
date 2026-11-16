//! Playlist folders: the listener's own grouping of playlists, in the sidebar and Library › Playlists. A folder holds
//! playlists (not other folders), has a name and a place among the folders, and can be folded shut. Deleting one keeps
//! its playlists: they come out of it. Folders are this computer's own: the phone and the web player have none, and
//! the file written for them leaves them out.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::Result;
use crate::identity::clean_text;
use crate::library::Library;

/// The longest name a folder keeps.
const NAME_MOST: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaylistFolder {
    pub id: i64,
    pub name: String,
    pub position: i64,
    /// Folded shut: its playlists are not listed under it.
    pub collapsed: bool,
}

impl Library {
    /// The folders, in their order.
    pub fn playlist_folders(&self) -> Result<Vec<PlaylistFolder>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, position, collapsed FROM playlist_folders ORDER BY position, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PlaylistFolder {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    position: r.get(2)?,
                    collapsed: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// A new folder, after the others. `None` when the name has nothing in it.
    pub fn create_playlist_folder(&mut self, name: &str) -> Result<Option<i64>> {
        let name = clean_text(name, NAME_MOST);
        if name.is_empty() {
            return Ok(None);
        }
        self.conn.execute(
            "INSERT INTO playlist_folders(name, position)
             VALUES (?1, (SELECT IFNULL(MAX(position), 0) + 1 FROM playlist_folders))",
            [name],
        )?;
        Ok(Some(self.conn.last_insert_rowid()))
    }

    /// Names a folder anew. `None` when the name has nothing in it; false when there is no such folder.
    pub fn rename_playlist_folder(&mut self, id: i64, name: &str) -> Result<Option<bool>> {
        let name = clean_text(name, NAME_MOST);
        if name.is_empty() {
            return Ok(None);
        }
        let n = self.conn.execute(
            "UPDATE playlist_folders SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(Some(n > 0))
    }

    /// Deletes a folder; its playlists come out of it and stay. False when there is no such folder.
    pub fn delete_playlist_folder(&mut self, id: i64) -> Result<bool> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE playlists SET folder_id = NULL WHERE folder_id = ?1",
            [id],
        )?;
        let n = tx.execute("DELETE FROM playlist_folders WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Folds a folder shut, or opens it. False when there is no such folder.
    pub fn fold_playlist_folder(&mut self, id: i64, collapsed: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE playlist_folders SET collapsed = ?2 WHERE id = ?1",
            params![id, collapsed],
        )? > 0)
    }

    /// Puts a playlist in a folder, or (`None`) takes it out of any. False when there is no such playlist or folder.
    pub fn set_playlist_folder(&mut self, playlist: i64, folder: Option<i64>) -> Result<bool> {
        if let Some(folder) = folder {
            let there = self
                .conn
                .query_row(
                    "SELECT 1 FROM playlist_folders WHERE id = ?1",
                    [folder],
                    |_| Ok(()),
                )
                .optional()?;
            if there.is_none() {
                return Ok(false);
            }
        }
        Ok(self.conn.execute(
            "UPDATE playlists SET folder_id = ?2 WHERE id = ?1",
            params![playlist, folder],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Library {
        Library::open_in_memory().unwrap()
    }

    #[test]
    fn folders_hold_playlists_and_deleting_one_keeps_them() {
        let mut lib = library();
        let (road, trip) = (
            lib.create_playlist("Road", 1).unwrap(),
            lib.create_playlist("Trip", 2).unwrap(),
        );
        assert_eq!(lib.create_playlist_folder("  ").unwrap(), None);
        let summer = lib.create_playlist_folder("Summer").unwrap().unwrap();
        let work = lib.create_playlist_folder("Work").unwrap().unwrap();
        let names: Vec<String> = lib
            .playlist_folders()
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["Summer", "Work"]);

        // In and out of folders; no such folder or playlist is said.
        assert!(lib.set_playlist_folder(road, Some(summer)).unwrap());
        assert!(lib.set_playlist_folder(trip, Some(summer)).unwrap());
        assert!(!lib.set_playlist_folder(trip, Some(9999)).unwrap());
        assert!(!lib.set_playlist_folder(9999, Some(work)).unwrap());
        let folder_of = |lib: &Library, id: i64| {
            lib.playlists()
                .unwrap()
                .into_iter()
                .find(|p| p.id == id)
                .unwrap()
                .folder
        };
        assert_eq!(
            (folder_of(&lib, road), folder_of(&lib, trip)),
            (Some(summer), Some(summer))
        );
        assert!(lib.set_playlist_folder(trip, None).unwrap());
        assert_eq!(folder_of(&lib, trip), None);

        // Renamed (an empty name is refused), folded shut and opened.
        assert_eq!(lib.rename_playlist_folder(summer, "").unwrap(), None);
        assert_eq!(
            lib.rename_playlist_folder(summer, "Summer 2026").unwrap(),
            Some(true)
        );
        assert_eq!(lib.rename_playlist_folder(9999, "x").unwrap(), Some(false));
        assert!(lib.fold_playlist_folder(summer, true).unwrap());
        assert!(lib.playlist_folders().unwrap()[0].collapsed);

        // Deleted: its playlists stay, out of any folder.
        assert!(lib.delete_playlist_folder(summer).unwrap());
        assert!(!lib.delete_playlist_folder(summer).unwrap());
        assert_eq!(folder_of(&lib, road), None);
        assert_eq!(lib.playlists().unwrap().len(), 2);
        // A playlist deleted takes nothing of the folder with it.
        assert!(lib.set_playlist_folder(road, Some(work)).unwrap());
        lib.delete_playlist(road).unwrap();
        assert_eq!(lib.playlist_folders().unwrap().len(), 1);
    }
}
