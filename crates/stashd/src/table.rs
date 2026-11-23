//! The library as a table (`stash_core::table`): its routes, and the pass that reads each file's extra tags once.
//!
//! - `GET /api/table?sort=&down=&q=&genre=&here=&offset=&limit=`: a page of songs in the order asked, with how many
//!   there are in all for that question, and how many files still wait to have their tags read.
//! - `GET /api/table/ids?...&offset=&limit=`: the ids from a place on, in the same order (what plays from a row).
//! - `GET /api/genres?here=`: every genre with its count. `GET /api/decades?here=`: every decade with its count.
//!   `GET /api/places`: the folders that hold the library's files, with how many songs sit in each.
//!   The table takes `decade=` and `folder=` to show only those.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use stash_core::edit::TagEdit;
use stash_core::table::{SortBy, TableAsk, read_more};
use tiny_http::{Method, Request};
use url::Url;

use crate::serve::{Reply, State, body_json, refuse, reply_json};

/// Files read between two looks at whether Stash is closing.
const BATCH: usize = 40;

/// Reads the extra tags (genre, year, composer, bit rate) of every file that has not had them read: the files scanned
/// before the library kept them, files Stash saved, and files a scan found changed. Local work only: it runs offline
/// too. It looks again every so often, for files that came since.
pub(crate) fn run(state: &State, stop: &AtomicBool) {
    let wait = |seconds: u64| {
        for _ in 0..seconds {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        true
    };
    if !wait(3) {
        return;
    }
    loop {
        let mut read_any = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let files = match state.library.lock().unwrap().files_without_tags(BATCH) {
                Ok(files) => files,
                Err(_) => break,
            };
            if files.is_empty() {
                break;
            }
            // The files are read with the library let go: reading tags must not hold up the window.
            let read: Vec<_> = files
                .iter()
                .map(|(id, path)| (*id, read_more(Path::new(path)).ok()))
                .collect();
            if state.library.lock().unwrap().set_more_tags(&read).is_err() {
                break;
            }
            read_any = true;
        }
        // The lists are read again once, when a run of files is done.
        if read_any {
            state.edited();
        }
        // With a connection, the covers of files that carry none of their own are fetched and kept, a few a second,
        // so the listener's own music has its covers with no connection too. Never offline.
        if !state.offline.load(Ordering::Relaxed) {
            let wanted = state.library.lock().unwrap().file_cover_addresses().unwrap_or_default();
            for url in wanted {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                if state.offline.load(Ordering::Relaxed) {
                    break;
                }
                if !crate::remote_art::is_kept(&url) {
                    crate::remote_art::keep(&url);
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        }
        if !wait(60) {
            return;
        }
    }
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !matches!(path, "/api/table" | "/api/table/ids" | "/api/genres" | "/api/decades" | "/api/places" | "/api/tags") {
        return Ok(None);
    }
    // POST /api/tags {ids, set}: writes the fields of `set` into the files of those songs (a field left out stays, ""
    // clears it), and answers how many were written, skipped (not files here) and failed, each failure with why.
    if path == "/api/tags" && *request.method() == Method::Post {
        let body = body_json(request)?;
        let ids: Vec<i64> = body
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();
        let Some(edit) = body.get("set").and_then(|v| serde_json::from_value::<TagEdit>(v.clone()).ok()) else {
            return Ok(Some(refuse(400, "set names the fields to change")));
        };
        if ids.is_empty() || ids.len() > 5_000 || edit.is_empty() {
            return Ok(Some(refuse(400, "ids are the songs (5,000 at most) and set the fields to change")));
        }
        if let Some(why) = edit.wrong() {
            return Ok(Some(refuse(400, why)));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let report = state.library.lock().unwrap().edit_tags(&ids, &edit, now)?;
        if report.written > 0 {
            state.edited();
        }
        return Ok(Some(reply_json(200, json!(report))));
    }
    if *request.method() != Method::Get {
        return Ok(None);
    }
    let url = Url::parse(&format!("http://local{}", request.url())).context("the request's path")?;
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    // GET /api/tags?ids=1,2,3: what those songs say now, each field's value when they agree, for the form.
    if path == "/api/tags" {
        let ids: Vec<i64> = param("ids").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).take(5_000).collect();
        return Ok(Some(reply_json(200, json!(state.library.lock().unwrap().tags_now(&ids)?))));
    }
    let number = |name: &str, default: usize| param(name).and_then(|v| v.parse::<usize>().ok()).unwrap_or(default);
    let here = param("here").as_deref() == Some("1");
    let library = state.library.lock().unwrap();
    if path == "/api/genres" {
        let rows: Vec<_> = library
            .genres(here)?
            .into_iter()
            .map(|(genre, songs)| json!({ "genre": genre, "songs": songs }))
            .collect();
        return Ok(Some(reply_json(200, json!({ "rows": rows, "waiting": library.tags_waiting()? }))));
    }
    // The decades the songs are from, and the folders that hold the files, each with its count.
    if path == "/api/decades" {
        let rows: Vec<_> = library
            .decades(here)?
            .into_iter()
            .map(|(decade, songs)| json!({ "decade": decade, "songs": songs }))
            .collect();
        return Ok(Some(reply_json(200, json!({ "rows": rows }))));
    }
    if path == "/api/places" {
        let rows: Vec<_> = library
            .file_folders()?
            .into_iter()
            .map(|(folder, songs)| json!({ "folder": folder, "songs": songs }))
            .collect();
        return Ok(Some(reply_json(200, json!({ "rows": rows, "roots": library.folders()? }))));
    }
    let by = match param("sort") {
        None => SortBy::Title,
        Some(s) => match SortBy::parse(&s) {
            Some(by) => by,
            None => return Ok(Some(refuse(400, "sort is title, artist, album, genre, year, length, format, added or plays"))),
        },
    };
    let down = param("down").as_deref() == Some("1");
    let ask = TableAsk {
        text: param("q").filter(|q| !q.trim().is_empty()),
        genre: param("genre"),
        here,
        decade: param("decade").and_then(|d| d.parse().ok()),
        folder: param("folder").filter(|f| !f.trim().is_empty()),
    };
    let offset = number("offset", 0);
    if path == "/api/table/ids" {
        let ids = library.table_ids(&ask, by, down, offset, number("limit", 1_000).clamp(1, 5_000))?;
        return Ok(Some(reply_json(200, json!({ "ids": ids }))));
    }
    let rows = library.table_at(&ask, by, down, offset, number("limit", 200).clamp(1, 500))?;
    Ok(Some(reply_json(
        200,
        json!({ "rows": rows, "count": library.table_count(&ask)?, "waiting": library.tags_waiting()? }),
    )))
}
