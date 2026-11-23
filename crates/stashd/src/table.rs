//! The library as a table (`stash_core::table`): its routes, and the pass that reads each file's extra tags once.
//!
//! - `GET /api/table?sort=&down=&q=&genre=&here=&offset=&limit=`: a page of songs in the order asked, with how many
//!   there are in all for that question, and how many files still wait to have their tags read.
//! - `GET /api/table/ids?...&offset=&limit=`: the ids from a place on, in the same order (what plays from a row).
//! - `GET /api/genres?here=`: every genre with its count.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use stash_core::table::{SortBy, TableAsk, read_more};
use tiny_http::{Method, Request};
use url::Url;

use crate::serve::{Reply, State, refuse, reply_json};

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
        if !wait(60) {
            return;
        }
    }
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !matches!(path, "/api/table" | "/api/table/ids" | "/api/genres") {
        return Ok(None);
    }
    if *request.method() != Method::Get {
        return Ok(None);
    }
    let url = Url::parse(&format!("http://local{}", request.url())).context("the request's path")?;
    let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
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
    let by = match param("sort") {
        None => SortBy::Title,
        Some(s) => match SortBy::parse(&s) {
            Some(by) => by,
            None => return Ok(Some(refuse(400, "sort is title, artist, album, genre, year, length, format, added or plays"))),
        },
    };
    let down = param("down").as_deref() == Some("1");
    let ask = TableAsk { text: param("q").filter(|q| !q.trim().is_empty()), genre: param("genre"), here };
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
