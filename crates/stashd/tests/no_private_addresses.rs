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
        // Text only: what is not UTF-8 (pictures, sounds, libraries) is not read.
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lower = text.to_lowercase();
        for needle in needles {
            if lower.contains(needle.as_str()) {
                found.push(format!("{} holds \"{needle}\"", path.display()));
            }
        }
    }
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
