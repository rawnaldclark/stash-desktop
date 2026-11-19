//! Scanning a picked folder: its changes page by page, every audio file new or changed read for its tags (a few at
//! once), each written as the song it is ([`Library::file_cloud_song`]). A pass from the start sees every file, and once
//! it is whole a file it did not see is gone; after it, only what changed since is asked for. Where the scan stands is
//! kept after each page, so a scan stopped (the computer off, the connection down) goes on from there.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Serialize;
use sha1::{Digest, Sha1};

use super::store::{Account, Folder, Leaving};
use super::tags::{self, FileSong};
use super::{Change, CloudError, Drive, File, Root};
use crate::library::Library;

/// The core's address for a cloud song's picture: its bytes' SHA-1 follows.
pub const ART: &str = "/api/art/cloud/";
/// How many files are read at once.
const AT_ONCE: usize = 4;
/// How many files are read before what was found is written.
const CHUNK: usize = 40;

/// How far a scan has come.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Audio files met.
    pub files: u64,
    /// Files whose tags were read (new or changed).
    pub read: u64,
    /// Songs new to the library.
    pub added: u64,
    /// Songs that left with their files.
    pub left: u64,
    /// The file being read.
    pub file: Option<String>,
}

/// What a scan works with.
pub struct Scanner<'a> {
    pub drive: &'a dyn Drive,
    /// Reads pieces of files (their links).
    pub agent: &'a ureq::Agent,
    /// Where the songs' pictures are kept.
    pub covers: &'a Path,
    pub stop: &'a AtomicBool,
}

fn failed(e: crate::Error) -> CloudError {
    CloudError::Failed(e.to_string())
}

fn now_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `length` bytes of the file at `url` from `start`.
pub fn fetch(agent: &ureq::Agent, url: &str, start: u64, length: u64) -> io::Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .header("Range", format!("bytes={start}-{}", start + length - 1))
        .call()
        .map_err(io::Error::other)?;
    match response.status().as_u16() {
        206 => {}
        // The whole file: only its start is of use.
        200 if start == 0 => {}
        status => {
            return Err(io::Error::other(format!(
                "the file's server answered {status}"
            )));
        }
    }
    let mut bytes = Vec::with_capacity(length as usize);
    response
        .body_mut()
        .as_reader()
        .take(length)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Keeps the picture with the others, by its bytes, and answers its address.
pub fn keep_cover(covers: &Path, cover: &crate::covers::Cover) -> Option<String> {
    let hex: String = Sha1::digest(&cover.bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = covers.join(&hex);
    if !path.exists() {
        std::fs::create_dir_all(covers).ok()?;
        let part = covers.join(format!("{hex}.part"));
        std::fs::write(&part, &cover.bytes).ok()?;
        std::fs::rename(&part, &path).ok()?;
    }
    Some(format!("{ART}{hex}"))
}

/// The picture kept under `name` (a SHA-1 in hex), and what kind of image it is.
pub fn cover(covers: &Path, name: &str) -> Option<crate::covers::Cover> {
    if name.len() != 40 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = std::fs::read(covers.join(name.to_ascii_lowercase())).ok()?;
    Some(crate::covers::Cover {
        content_type: crate::covers::kind(&bytes)?,
        bytes,
    })
}

/// The folders between the picked one and a file, from the parents the pages named (OneDrive).
fn folders_of(
    names: &HashMap<String, (String, Option<String>)>,
    root: &str,
    parent: Option<&str>,
) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut at = parent?;
    for _ in 0..32 {
        if at == root {
            out.reverse();
            return Some(out);
        }
        let (name, up) = names.get(at)?;
        out.push(name.clone());
        at = up.as_deref()?;
    }
    None
}

/// Scans the account's `folder` into `library`, telling `progress` how far it has come.
pub fn scan(
    library: &mut Library,
    s: &Scanner<'_>,
    account: &Account,
    folder: &Folder,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Progress, CloudError> {
    let kind = s.drive.kind();
    let root = Root {
        id: folder.item_id.clone(),
        path_lower: folder.path.to_lowercase(),
    };
    let mut done = Progress::default();
    let mut cursor = folder.cursor.clone();
    let mut delta = folder.delta && cursor.is_some();
    let mut generation = folder.generation;
    if cursor.is_none() {
        generation += 1;
        library
            .cloud_folder_at(folder.id, None, false, generation)
            .map_err(failed)?;
    }
    // A pass that sees every file: it began from the start (now, or before and stopped on the way).
    let mut whole = !delta;
    let mut names: HashMap<String, (String, Option<String>)> = HashMap::new();
    loop {
        if s.stop.load(Ordering::Relaxed) {
            return Err(CloudError::Busy("the scan was stopped".into()));
        }
        let page = match s.drive.page(&root, cursor.as_deref()) {
            Err(CloudError::CursorGone) => {
                (cursor, delta, whole) = (None, false, true);
                generation += 1;
                library
                    .cloud_folder_at(folder.id, None, false, generation)
                    .map_err(failed)?;
                continue;
            }
            other => other?,
        };
        let mut to_read: Vec<File> = Vec::new();
        let mut removed: Vec<(Option<String>, Option<String>)> = Vec::new();
        let mut here: Vec<String> = Vec::new();
        for change in page.changes {
            match change {
                Change::Folder { id, name, parent } => {
                    names.insert(id, (name, parent));
                }
                Change::Removed { id, path_lower } => removed.push((id, path_lower)),
                Change::File(mut file) => {
                    if !super::is_audio(&file.name) {
                        continue;
                    }
                    done.files += 1;
                    here.push(file.id.clone());
                    if file.folders.is_none() {
                        file.folders = folders_of(&names, &root.id, file.parent.as_deref());
                    }
                    let known = library.cloud_known(account.id, &file.id).map_err(failed)?;
                    match known {
                        Some(known) if known.version == file.version && known.size == file.size => {
                            library
                                .cloud_seen(account.id, &file, folder.id, generation)
                                .map_err(failed)?;
                        }
                        _ => to_read.push(file),
                    }
                }
            }
        }
        for chunk in to_read.chunks(CHUNK) {
            let read = read_all(s, chunk, &mut done, progress)?;
            for (file, song) in read {
                let art = song.cover.as_ref().and_then(|c| keep_cover(s.covers, c));
                let filed = library
                    .file_cloud_song(
                        kind,
                        account.id,
                        folder.id,
                        &file,
                        &song.track,
                        art.as_deref(),
                        generation,
                    )
                    .map_err(failed)?;
                done.read += 1;
                if filed.added {
                    done.added += 1;
                }
            }
            done.file = None;
            progress(&done);
        }
        // Removed after the files are written: a file moved is removed from where it was, and is where it is now.
        for (id, path) in removed {
            let leaving = match (&id, &path) {
                (Some(id), _) => Leaving::Ids(std::slice::from_ref(id)),
                (None, Some(path)) => Leaving::Under {
                    path,
                    except: &here,
                },
                (None, None) => continue,
            };
            done.left += library
                .cloud_items_leave(kind, account.id, leaving)
                .map_err(failed)? as u64;
        }
        cursor = Some(page.cursor);
        if !page.more {
            if whole {
                done.left += library
                    .cloud_items_leave(
                        kind,
                        account.id,
                        Leaving::Stale {
                            folder: folder.id,
                            generation,
                        },
                    )
                    .map_err(failed)? as u64;
            }
            library
                .cloud_folder_at(folder.id, cursor.as_deref(), true, generation)
                .map_err(failed)?;
            progress(&done);
            return Ok(done);
        }
        library
            .cloud_folder_at(folder.id, cursor.as_deref(), delta, generation)
            .map_err(failed)?;
        progress(&done);
    }
}

/// Reads the tags of `files`, a few at once. A file whose link cannot be had is known by its name; the provider
/// signed out, or stopped answering, stops the scan.
fn read_all(
    s: &Scanner<'_>,
    files: &[File],
    done: &mut Progress,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Vec<(File, FileSong)>, CloudError> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, FileSong)>> = Mutex::new(Vec::new());
    let trouble: Mutex<Option<CloudError>> = Mutex::new(None);
    let reading: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..AT_ONCE.min(files.len()) {
            scope.spawn(|| {
                loop {
                    if s.stop.load(Ordering::Relaxed) || trouble.lock().unwrap().is_some() {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else {
                        return;
                    };
                    *reading.lock().unwrap() = Some(file.name.clone());
                    match read_one(s, file) {
                        Ok(song) => out.lock().unwrap().push((i, song)),
                        Err(e) => {
                            trouble.lock().unwrap().get_or_insert(e);
                            return;
                        }
                    }
                }
            });
        }
        // While they read, the file being read is told.
        while next.load(Ordering::Relaxed) < files.len() && trouble.lock().unwrap().is_none() {
            if s.stop.load(Ordering::Relaxed) {
                break;
            }
            done.file = reading.lock().unwrap().clone();
            progress(done);
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    });
    if let Some(e) = trouble.into_inner().unwrap() {
        return Err(e);
    }
    if s.stop.load(Ordering::Relaxed) {
        return Err(CloudError::Busy("the scan was stopped".into()));
    }
    let mut read = out.into_inner().unwrap();
    read.sort_by_key(|(i, _)| *i);
    let mut seen = HashSet::new();
    Ok(read
        .into_iter()
        .filter(|(i, _)| seen.insert(*i))
        .map(|(i, song)| (files[i].clone(), song))
        .collect())
}

fn read_one(s: &Scanner<'_>, file: &File) -> Result<FileSong, CloudError> {
    let now = now_s();
    let url = match &file.link {
        Some(link) => link.clone(),
        None => match s.drive.link(&file.id) {
            Ok(link) => link.url,
            // Gone between the list and now: it is left out (the next scan sees it gone).
            Err(CloudError::Gone) => {
                return Ok(FileSong {
                    track: tags::by_name(&file.name, file.folders.as_deref(), now),
                    cover: None,
                    tagged: false,
                });
            }
            Err(e) => return Err(e),
        },
    };
    if file.size == 0 {
        return Ok(FileSong {
            track: tags::by_name(&file.name, file.folders.as_deref(), now),
            cover: None,
            tagged: false,
        });
    }
    Ok(tags::read(
        &file.name,
        file.size,
        file.folders.as_deref(),
        now,
        |start, length| fetch(s.agent, &url, start, length),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::{Entry, Kind, Link, Me, Page};

    /// A drive whose pages are given, and whose files are not there to read (songs are known by their names).
    struct Pages {
        pages: Mutex<Vec<Result<Page, CloudError>>>,
        asked: Mutex<Vec<Option<String>>>,
    }

    impl Drive for Pages {
        fn kind(&self) -> Kind {
            Kind::Dropbox
        }
        fn me(&self) -> Result<Me, CloudError> {
            unreachable!()
        }
        fn browse(&self, _: Option<&str>) -> Result<Vec<Entry>, CloudError> {
            unreachable!()
        }
        fn path_of(&self, _: &str) -> Result<String, CloudError> {
            unreachable!()
        }
        fn page(&self, _: &Root, cursor: Option<&str>) -> Result<Page, CloudError> {
            self.asked.lock().unwrap().push(cursor.map(str::to_string));
            self.pages.lock().unwrap().remove(0)
        }
        fn link(&self, _: &str) -> Result<Link, CloudError> {
            Err(CloudError::Gone)
        }
    }

    fn file(id: &str, name: &str, folders: &[&str], version: &str) -> Change {
        Change::File(File {
            id: id.into(),
            name: name.into(),
            size: 100,
            version: Some(version.into()),
            folders: Some(folders.iter().map(|f| f.to_string()).collect()),
            path_lower: Some(format!("/music/{}/{}", folders.join("/"), name).to_lowercase()),
            ..Default::default()
        })
    }

    #[test]
    fn a_folder_is_scanned_whole_then_by_its_changes_and_a_file_gone_takes_its_song() {
        let mut library = Library::open_in_memory().unwrap();
        let account_id = library
            .keep_cloud_account(Kind::Dropbox, "dbid:1", "Me", None, "r", 1)
            .unwrap();
        let folder_id = library
            .add_cloud_folder(account_id, "id:music", "/Music", 1)
            .unwrap();
        let account = library.cloud_account(account_id).unwrap().unwrap();
        let drive = Pages {
            pages: Mutex::new(vec![
                Ok(Page {
                    changes: vec![
                        file("id:1", "01 - One.flac", &["Band", "Record"], "a"),
                        file("id:x", "cover.jpg", &["Band", "Record"], "a"),
                    ],
                    cursor: "c1".into(),
                    more: true,
                }),
                Ok(Page {
                    changes: vec![file("id:2", "02 - Two.mp3", &["Band", "Record"], "a")],
                    cursor: "c2".into(),
                    more: false,
                }),
                // Since: one removed, one unchanged.
                Ok(Page {
                    changes: vec![
                        Change::Removed {
                            id: None,
                            path_lower: Some("/music/band/record/02 - two.mp3".into()),
                        },
                        file("id:1", "01 - One.flac", &["Band", "Record"], "a"),
                    ],
                    cursor: "c3".into(),
                    more: false,
                }),
                // The provider forgot the mark: read whole again, and a file not seen leaves.
                Err(CloudError::CursorGone),
                Ok(Page {
                    changes: vec![],
                    cursor: "c4".into(),
                    more: false,
                }),
            ]),
            asked: Mutex::new(Vec::new()),
        };
        let agent = crate::cloud::agent();
        let covers = std::env::temp_dir();
        let stop = AtomicBool::new(false);
        let s = Scanner {
            drive: &drive,
            agent: &agent,
            covers: &covers,
            stop: &stop,
        };
        let mut told = 0;
        let mut tell = |_: &Progress| told += 1;

        let folder = library.cloud_folder(folder_id).unwrap().unwrap();
        let first = scan(&mut library, &s, &account, &folder, &mut tell).unwrap();
        assert_eq!(
            (first.files, first.read, first.added, first.left),
            (2, 2, 2, 0)
        );
        let one = library
            .cloud_known(account_id, "id:1")
            .unwrap()
            .unwrap()
            .track
            .unwrap();
        let described = library.describe(one).unwrap().unwrap();
        assert_eq!(described.kept.title, "One");
        assert_eq!(described.kept.artist, "Band");
        assert_eq!(described.kept.album.as_deref(), Some("Record"));

        let folder = library.cloud_folder(folder_id).unwrap().unwrap();
        assert_eq!(
            (
                folder.cursor.as_deref(),
                folder.delta,
                folder.generation,
                folder.songs
            ),
            (Some("c2"), true, 1, 2)
        );
        let second = scan(&mut library, &s, &account, &folder, &mut tell).unwrap();
        assert_eq!((second.files, second.read, second.left), (1, 0, 1));

        let folder = library.cloud_folder(folder_id).unwrap().unwrap();
        let third = scan(&mut library, &s, &account, &folder, &mut tell).unwrap();
        assert_eq!(third.left, 1);
        assert!(library.describe(one).unwrap().is_none());
        assert_eq!(
            *drive.asked.lock().unwrap(),
            vec![
                None,
                Some("c1".into()),
                Some("c2".into()),
                Some("c3".into()),
                None
            ]
        );
        assert_eq!(
            library.cloud_folder(folder_id).unwrap().unwrap().generation,
            2
        );
        assert!(told > 0);
    }

    #[test]
    fn a_onedrive_files_folders_come_from_the_folders_the_pages_named() {
        let mut names = HashMap::new();
        names.insert(
            "A".to_string(),
            ("Artist".to_string(), Some("R".to_string())),
        );
        names.insert(
            "B".to_string(),
            ("Album".to_string(), Some("A".to_string())),
        );
        assert_eq!(
            folders_of(&names, "R", Some("B")),
            Some(vec!["Artist".to_string(), "Album".into()])
        );
        assert_eq!(folders_of(&names, "R", Some("R")), Some(vec![]));
        assert_eq!(folders_of(&names, "R", Some("unknown")), None);
    }

    #[test]
    fn a_picture_is_kept_by_its_bytes_and_given_back_only_as_an_image() {
        let covers =
            std::env::temp_dir().join(format!("stash-cloud-covers-{}", std::process::id()));
        let png = crate::covers::Cover {
            content_type: "image/png",
            bytes: b"\x89PNG\r\n\x1a\nrest".to_vec(),
        };
        let address = keep_cover(&covers, &png).unwrap();
        let name = address.strip_prefix(ART).unwrap();
        assert_eq!(cover(&covers, name), Some(png));
        assert_eq!(cover(&covers, "../../etc"), None);
        let _ = std::fs::remove_dir_all(&covers);
    }
}
