//! The repository is public, and Stash's Last.fm proxy holds a key: its address comes in when a build is made
//! (`STASH_LASTFM_PROXY`), never from the source. This fails when anyone puts such an address, or a key, into a file
//! that could be committed: every file git tracks, and every new one it does not ignore. (The words looked for are put
//! together here, so this file does not find itself.)

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// The files a commit could take: those git tracks, and new ones it does not ignore. Without git, every file under
/// `root` outside the skipped folders.
fn files(root: &Path) -> Vec<PathBuf> {
    let listed = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output();
    let mut found = Vec::new();
    match listed {
        Ok(out) if out.status.success() => {
            for name in out.stdout.split(|b| *b == 0).filter(|n| !n.is_empty()) {
                let name = String::from_utf8_lossy(name);
                let skipped = name.split('/').any(|part| SKIPPED.contains(&part));
                let path = root.join(name.as_ref());
                if !skipped && path.is_file() {
                    found.push(path);
                }
            }
        }
        _ => walk(root, &mut found),
    }
    found
}

fn walk(folder: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIPPED.contains(&name.as_str()) {
                walk(&path, found);
            }
        } else {
            found.push(path);
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

/// What gives a text away, in a few words each: an address of the kind the proxy has, a key or secret written as a
/// setting with a value (`api_key=…`, `"apiKey": "…"`, `API-SECRET = …`), or a key's 32 hex digits on a line that names
/// a key or a secret.
fn giveaways(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut found = Vec::new();
    let host = ["workers", ".dev"].concat();
    if lower.contains(&host) {
        found.push(format!("an address on {host}"));
    }
    if let Some(at) = key_setting(&lower) {
        found.push(format!("a key written as a setting (at byte {at})"));
    }
    for (n, line) in lower.lines().enumerate() {
        if (line.contains("key") || line.contains("secret")) && has_hex_run(line, 32) {
            found.push(format!("32 hex digits beside a key's name (line {})", n + 1));
        }
    }
    found
}

/// Where `api`, then `_`, `-` or nothing, then `key` or `secret`, then quotes or spaces, then `:` or `=`, then a value of
/// at least 16 letters and digits is written: a key given its value, in any of the ways settings files write one.
fn key_setting(lower: &str) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(found) = lower[from..].find("api") {
        let start = from + found;
        from = start + 3;
        let mut at = from;
        if matches!(bytes.get(at), Some(b'_' | b'-')) {
            at += 1;
        }
        let name = ["key", "secret"]
            .iter()
            .find(|name| lower[at..].starts_with(*name));
        let Some(name) = name else { continue };
        at += name.len();
        while matches!(bytes.get(at), Some(b'"' | b'\'' | b' ' | b'\t')) {
            at += 1;
        }
        if !matches!(bytes.get(at), Some(b':' | b'=')) {
            continue;
        }
        at += 1;
        while matches!(bytes.get(at), Some(b'"' | b'\'' | b' ' | b'\t')) {
            at += 1;
        }
        let value = bytes[at.min(bytes.len())..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            .count();
        if value >= 16 {
            return Some(start);
        }
    }
    None
}

/// Whether `line` holds a run of exactly `n` hex digits, standing alone (not part of a longer word or number).
fn has_hex_run(line: &str, n: usize) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word.len() == n && word.chars().all(|c| c.is_ascii_hexdigit()))
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
fn a_key_is_found_however_a_settings_file_writes_it() {
    let value = "0123456789abcdef0123456789abcdef";
    let api = |rest: &str| ["api", rest].concat();
    for written in [
        format!("{}={value}", api("_key")),
        format!("{} = \"{value}\"", api("_KEY")),
        format!("\"{}\": \"{value}\"", api("Key")),
        format!("{}: {value}", api("-secret")),
        format!("{}='{value}'", api("_secret")),
    ] {
        assert!(!giveaways(&written).is_empty(), "{written}");
    }
    // Names of fields and checks of a key, with no key in them, are not a key.
    for code in [
        format!("fn ask({}: &str)", api("_key")),
        format!("assert!(!key.contains(\"{}\"))", api("_key")),
        format!("q.contains_key(\"{}\")", api("_key")),
        "checksum = \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"".to_string(),
    ] {
        assert!(giveaways(&code).is_empty(), "{code}: {:?}", giveaways(&code));
    }
    assert!(!giveaways(&format!("lastfm key: {value}")).is_empty(), "a bare key beside its name");
    assert!(!giveaways(&format!("https://x.y.{}{}/", "workers", ".dev")).is_empty());
}

#[test]
fn no_file_of_the_repository_holds_a_proxys_address_or_a_key() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    let files = files(&root);
    assert!(files.len() > 100, "only {} files were read", files.len());
    for file in files {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        for giveaway in giveaways(&text_of(&bytes)) {
            found.push(format!("{} holds {giveaway}", file.display()));
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}
