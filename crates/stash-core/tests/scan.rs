//! Scanning a folder: real files with real tags, written here and read back through the scanner.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{RATE, scratch, signal, write_wav};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use stash_core::scan::scan_folder;
use stash_core::{Counts, Library};

/// A short WAV at `path` with these tags (none when `title` is empty).
fn song(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    album_artist: Option<&str>,
    track: u32,
) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_wav(path, RATE, &signal(4_410));
    if title.is_empty() {
        return;
    }
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.to_string());
    tag.set_artist(artist.to_string());
    tag.set_album(album.to_string());
    tag.set_track(track);
    if let Some(by) = album_artist {
        tag.insert_text(ItemKey::AlbumArtist, by.to_string());
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

/// A small collection: an album, a two-artist compilation, a file with no tags, and things that are not music.
fn collection(name: &str) -> PathBuf {
    let root = scratch(name).join("Music");
    song(
        &root.join("Radiohead/Pablo Honey/02 Creep.wav"),
        "Creep",
        "Radiohead",
        "Pablo Honey",
        None,
        2,
    );
    song(
        &root.join("Radiohead/Pablo Honey/01 You.wav"),
        "You",
        "Radiohead",
        "Pablo Honey",
        None,
        1,
    );
    song(
        &root.join("Various/Now 1/01 Song A.wav"),
        "Song A",
        "Artist A",
        "Now 1",
        Some("Various Artists"),
        1,
    );
    song(
        &root.join("Various/Now 1/02 Song B.wav"),
        "Song B",
        "Artist B",
        "Now 1",
        Some("Various Artists"),
        2,
    );
    song(
        &root.join("loose/untitled recording.wav"),
        "",
        "",
        "",
        None,
        0,
    );
    fs::write(
        root.join("Radiohead/Pablo Honey/cover.jpg"),
        b"not a picture",
    )
    .unwrap();
    fs::write(root.join("notes.txt"), b"not music").unwrap();
    fs::write(root.join("broken.flac"), b"this is not a FLAC file").unwrap();
    root
}

fn titles(library: &Library, query: &str) -> Vec<String> {
    library
        .search(query, 10)
        .unwrap()
        .into_iter()
        .map(|t| format!("{} / {}", t.title, t.artist))
        .collect()
}

#[test]
fn a_scan_reads_tags_names_untagged_files_and_leaves_out_what_is_not_music() {
    let root = collection("scan-first");
    let mut library = Library::open_in_memory().unwrap();
    let mut heard = 0;
    let report = scan_folder(&mut library, &root, |_| heard += 1).unwrap();

    assert_eq!(
        (
            report.files,
            report.added,
            report.changed,
            report.unchanged,
            report.missing
        ),
        (6, 5, 0, 0, 0)
    );
    assert_eq!(report.unreadable, 1, "the file that only claims to be FLAC");
    assert!(
        report.unreadable_examples[0].contains("broken.flac"),
        "{:?}",
        report.unreadable_examples
    );
    assert!(heard >= 1, "progress was told at least at the end");
    // Five tracks; two albums (the compilation is one album under its album artist); and the artists named.
    assert_eq!(
        library.counts().unwrap(),
        Counts {
            tracks: 5,
            albums: 2,
            artists: 5,
            liked: 0,
            playlists: 0,
        }
    );
    assert_eq!(titles(&library, "creep"), ["Creep / Radiohead"]);
    assert_eq!(
        titles(&library, "untitled"),
        ["untitled recording / Unknown artist"],
        "a file with no tags is named after itself"
    );
    let compilation = library
        .albums_at(0, 10)
        .unwrap()
        .into_iter()
        .find(|a| a.title == "Now 1")
        .unwrap();
    assert_eq!(
        (compilation.artist.as_str(), compilation.track_count),
        ("Various Artists", 2)
    );
    // What the audio is comes from the file, not from a tag.
    let creep = &library.search("creep", 1).unwrap()[0];
    assert_eq!(
        (
            creep.codec.as_deref(),
            creep.sample_rate,
            creep.bit_depth,
            creep.duration_ms
        ),
        (Some("pcm"), Some(RATE), Some(16), 100)
    );
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn a_second_scan_reads_nothing_again_and_a_changed_file_keeps_its_row() {
    let root = collection("scan-again");
    let mut library = Library::open_in_memory().unwrap();
    scan_folder(&mut library, &root, |_| {}).unwrap();
    let creep_id = library.search("creep", 1).unwrap()[0].id;

    let again = scan_folder(&mut library, &root, |_| {}).unwrap();
    assert_eq!(
        (again.added, again.changed, again.unchanged, again.missing),
        (0, 0, 5, 0)
    );

    // The file is tagged anew (its size changes with the longer title): the same row takes the new tags.
    song(
        &root.join("Radiohead/Pablo Honey/02 Creep.wav"),
        "Creep (Acoustic Version)",
        "Radiohead",
        "Pablo Honey",
        None,
        2,
    );
    let changed = scan_folder(&mut library, &root, |_| {}).unwrap();
    assert_eq!(
        (changed.added, changed.changed, changed.unchanged),
        (0, 1, 4)
    );
    let found = library.search("acoustic", 5).unwrap();
    assert_eq!(
        (found.len(), found[0].id, found[0].title.as_str()),
        (1, creep_id, "Creep (Acoustic Version)")
    );
    assert_eq!(
        library.counts().unwrap().tracks,
        5,
        "no second row for the same file"
    );
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn a_file_that_is_gone_is_marked_missing_not_deleted_and_comes_back() {
    let root = collection("scan-missing");
    let mut library = Library::open_in_memory().unwrap();
    scan_folder(&mut library, &root, |_| {}).unwrap();

    let file = root.join("Radiohead/Pablo Honey/01 You.wav");
    let kept = root.join("01 You.wav.away");
    fs::rename(&file, &kept).unwrap();
    let gone = scan_folder(&mut library, &root, |_| {}).unwrap();
    assert_eq!((gone.missing, gone.unchanged), (1, 4));
    assert_eq!(
        (
            library.counts().unwrap().tracks,
            library.missing_count().unwrap()
        ),
        (5, 1),
        "the row stays"
    );
    // A scan that still does not find it does not count it again.
    assert_eq!(scan_folder(&mut library, &root, |_| {}).unwrap().missing, 0);

    fs::rename(&kept, &file).unwrap();
    let back = scan_folder(&mut library, &root, |_| {}).unwrap();
    assert_eq!(
        (back.added, back.unchanged, library.missing_count().unwrap()),
        (0, 5, 0),
        "the same row, there again"
    );
    let _ = fs::remove_dir_all(root.parent().unwrap());
}

#[test]
fn scanning_one_folder_never_judges_a_neighbour_whose_name_starts_the_same() {
    let base = scratch("scan-neighbours");
    let (music, music2) = (base.join("Music"), base.join("Music2"));
    song(&music.join("a.wav"), "In Music", "A", "One", None, 1);
    song(&music2.join("b.wav"), "In Music Two", "B", "Two", None, 1);
    let mut library = Library::open_in_memory().unwrap();
    scan_folder(&mut library, &music, |_| {}).unwrap();
    scan_folder(&mut library, &music2, |_| {}).unwrap();

    let report = scan_folder(&mut library, &music, |_| {}).unwrap();
    assert_eq!((report.missing, library.missing_count().unwrap()), (0, 0));
    assert!(
        scan_folder(&mut library, &base.join("not-there"), |_| {}).is_err(),
        "a folder that does not exist is an error, not an empty scan"
    );
    let _ = fs::remove_dir_all(&base);
}
