//! Playlist files in and out (see `stash_core::playlist_file`).
//!
//! POST /api/playlists/file/pick: the system's own dialog, for a playlist file to bring in; answers its path (null when
//! none was chosen). POST /api/playlists/file {path}: makes a playlist of that file, and answers what it brought, with
//! the lists as they now are. GET /api/playlists/{id}/file, and /api/liked/file: the list as an .m3u8 file, how many
//! songs it holds (`X-Songs`) and how many were left out (`X-Left-Out`: songs only the add-ons play).

use std::path::Path;
use std::sync::atomic::Ordering;

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::playlist_file::parse;
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, header, now_ms, refuse, reply, reply_json};

/// The biggest playlist file read.
const FILE_MOST: u64 = 5 * 1024 * 1024;
/// The longest name a playlist gets from a file.
const NAME_MOST: usize = 80;

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    match (request.method(), path) {
        // Asks the listener for a playlist file with the system's own dialog, on the computer the core runs on.
        (Method::Post, "/api/playlists/file/pick") => {
            let picked = rfd::FileDialog::new()
                .set_title("Choose a playlist file")
                .add_filter("Playlist files", &["m3u8", "m3u"])
                .pick_file()
                .map(|file| file.to_string_lossy().into_owned());
            Ok(Some(reply_json(200, json!({ "path": picked }))))
        }
        (Method::Post, "/api/playlists/file") => bring_in(state, request).map(Some),
        (Method::Get, "/api/liked/file") => send_out(state, None).map(Some),
        (Method::Get, _) => match path
            .strip_prefix("/api/playlists/")
            .and_then(|rest| rest.strip_suffix("/file"))
            .and_then(|id| id.parse::<i64>().ok())
        {
            Some(id) => send_out(state, Some(id)).map(Some),
            None => Ok(None),
        },
        _ => Ok(None),
    }
}

fn bring_in(state: &State, request: &mut Request) -> Result<Reply> {
    let body = body_json(request)?;
    let Some(path) = body
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return Ok(refuse(400, "Choose a playlist file"));
    };
    let file = Path::new(path);
    let named_so = file
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"));
    if !named_so {
        return Ok(refuse(400, "That is not a playlist file (.m3u or .m3u8)"));
    }
    let Ok(meta) = std::fs::metadata(file) else {
        return Ok(refuse(400, "That playlist file was not found"));
    };
    if !meta.is_file() {
        return Ok(refuse(400, "That playlist file was not found"));
    }
    if meta.len() > FILE_MOST {
        return Ok(refuse(400, "That playlist file is too big"));
    }
    let Ok(bytes) = std::fs::read(file) else {
        return Ok(refuse(400, "That playlist file could not be read"));
    };
    let parsed = parse(&bytes);
    if parsed.entries.is_empty() {
        return Ok(refuse(400, "That playlist file has no songs in it"));
    }
    let name: String = parsed
        .name
        .clone()
        .or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .map(|n| n.trim().chars().take(NAME_MOST).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Playlist".into());
    let beside = file.parent().unwrap_or(Path::new("."));
    let mut library = state.library.lock().unwrap();
    let brought = library.playlist_from_file(&parsed, &name, beside, now_ms())?;
    let mut answer = json!({
        "brought": brought,
        "playlists": library.playlists()?,
        "folders": library.playlist_folders()?,
        "counts": library.counts()?,
    });
    drop(library);
    state.edited();
    answer["edits"] = json!(state.edits.load(Ordering::Relaxed));
    Ok(reply_json(200, answer))
}

fn send_out(state: &State, id: Option<i64>) -> Result<Reply> {
    let Some((name, text, songs, left_out)) = state.library.lock().unwrap().playlist_file(id)?
    else {
        return Ok(refuse(404, "no such playlist"));
    };
    let file = format!("{}.m3u8", file_name(&name));
    // An ASCII name for older readers, and the name itself (RFC 5987).
    let plain: String = file
        .chars()
        .map(|c| if c.is_ascii() && c != '"' { c } else { '_' })
        .collect();
    let disposition = format!(
        "attachment; filename=\"{plain}\"; filename*=UTF-8''{}",
        percent_encode(&file)
    );
    Ok(
        reply(200, "audio/x-mpegurl; charset=utf-8", text.into_bytes())
            .with_header(header("Content-Disposition", &disposition))
            .with_header(header("X-Songs", &songs.to_string()))
            .with_header(header("X-Left-Out", &left_out.to_string())),
    )
}

/// A name a file can have on any system: without the characters Windows refuses, and not empty.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() {
        "Playlist".into()
    } else {
        cleaned.chars().take(NAME_MOST).collect()
    }
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playlist_name_is_a_file_name_anywhere() {
        assert_eq!(file_name("Road trip: 2026/27?"), "Road trip_ 2026_27_");
        assert_eq!(file_name(" ... "), "Playlist");
        assert_eq!(file_name("Été"), "Été");
        assert_eq!(percent_encode("Été 1.m3u8"), "%C3%89t%C3%A9%201.m3u8");
    }
}
