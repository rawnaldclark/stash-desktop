//! The repository is public, and Stash's Last.fm proxy holds a key: its address comes in when a build is made
//! (`STASH_LASTFM_PROXY`), never from the source. This fails when anyone puts such an address, or a key, into a file
//! of the repository. (The words looked for are put together here, so this file does not find itself.)

use std::path::Path;

/// Folders that are not the repository's own text: builds, packages, git's own, and what is copied in from others.
const SKIPPED: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "third_party",
    "dist",
    ".svelte-kit",
    "build",
];

fn walk(folder: &Path, found: &mut Vec<String>, needles: &[String]) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIPPED.contains(&name.as_str()) {
                walk(&path, found, needles);
            }
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let lower = text_of(&bytes).to_lowercase();
        for needle in needles {
            if lower.contains(needle.as_str()) {
                found.push(format!("{} holds \"{needle}\"", path.display()));
            }
        }
    }
}

/// A file's text, whatever it is written in: UTF-16 with its mark (as some Windows tools write), else UTF-8 with any
/// bytes that are not UTF-8 read as marks (the words looked for are plain ASCII, so they are found in any file).
fn text_of(bytes: &[u8]) -> String {
    let utf16 = |pair: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| pair(*p))
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, ..] => utf16(u16::from_le_bytes),
        [0xFE, 0xFF, ..] => utf16(u16::from_be_bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[test]
fn a_file_in_utf_16_or_not_in_utf_8_is_read_too() {
    let needle = ["api", "_key="].concat();
    let line = ["x API", "_KEY=1"].concat();
    let mut le = vec![0xFF, 0xFE];
    le.extend(line.encode_utf16().flat_map(u16::to_le_bytes));
    assert!(text_of(&le).to_lowercase().contains(&needle));
    let mut be = vec![0xFE, 0xFF];
    be.extend(line.encode_utf16().flat_map(u16::to_be_bytes));
    assert!(text_of(&be).to_lowercase().contains(&needle));
    let mut latin = b"caf\xe9 ".to_vec();
    latin.extend(needle.as_bytes());
    assert!(text_of(&latin).contains(&needle));
}

#[test]
fn no_file_of_the_repository_holds_a_proxys_address_or_a_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let needles = [
        ["workers", ".dev"].concat(),
        ["api", "_key="].concat(),
        ["api", "_secret="].concat(),
    ];
    let mut found = Vec::new();
    walk(&root, &mut found, &needles);
    assert!(found.is_empty(), "{found:#?}");
}
