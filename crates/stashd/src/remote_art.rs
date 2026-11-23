//! Covers that live on a server (named by an add-on, or kept in the library from one), fetched by the core for the
//! window and the system's media controls: `GET /api/art/remote?u={address}`. Neither fetches them itself, as
//! neither would hold to where the add-on that named them may send Stash: a cover's server could redirect the window
//! to a device on the listener's network. The core fetches each held to that add-on's reach (noted as its tracks go
//! out to the window; a cover no add-on named this run, one kept in the library, is held to the internet), JPEG or PNG
//! only, and keeps the last ones fetched so the window can ask again.
//!
//! Each is answered on a thread of its own, a few at a time, so a page of covers does not keep the window's other
//! requests waiting.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use stash_core::downloads;
use stash_core::reach::Reach;
use tiny_http::{Request, Response, StatusCode};

use crate::serve::header;

/// What the route starts with.
pub(crate) const ROUTE: &str = "/api/art/remote?";
/// How many add-on covers' reaches are kept (all are forgotten when there are more).
const MOST_NOTED: usize = 8192;
/// How many covers are kept, the last fetched.
const MOST_KEPT: usize = 160;
/// How many are fetched at once; more are answered "busy" (the window draws none, and asks again next time).
const MOST_AT_ONCE: usize = 12;

static NOTED: LazyLock<Mutex<HashMap<String, Reach>>> = LazyLock::new(Default::default);
/// A cover kept: its address, and its bytes.
type Kept = (String, Arc<Vec<u8>>);

static KEPT: LazyLock<Mutex<VecDeque<Kept>>> = LazyLock::new(Default::default);
static FETCHING: AtomicUsize = AtomicUsize::new(0);

/// Where covers are kept on this computer once fetched (a `Covers` folder beside the library): a cover seen once
/// shows again with no connection, and after Stash is opened again.
static FOLDER: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Says where covers are kept. Once, as the core starts; without it they are kept in memory only.
pub(crate) fn keep_in(folder: std::path::PathBuf) {
    let _ = std::fs::create_dir_all(&folder);
    let _ = FOLDER.set(folder);
}

/// The file a cover at `url` is kept in: named by the address's SHA-256, so nothing of the address is a path.
fn kept_file(url: &str) -> Option<std::path::PathBuf> {
    use sha2::Digest as _;
    let name: String = sha2::Sha256::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(FOLDER.get()?.join(name))
}

/// An add-on of `reach` named the cover at `url`.
pub(crate) fn note(url: &str, reach: Reach) {
    let mut noted = NOTED.lock().unwrap();
    if noted.len() >= MOST_NOTED {
        noted.clear();
    }
    noted.insert(url.to_string(), reach);
}

/// Answers a request for a cover, on a thread of its own. `origin` is the window's, when it may read the picture.
pub(crate) fn answer(request: Request, origin: Option<String>, offline: bool) {
    let spawned = std::thread::Builder::new()
        .name("stash-cover".into())
        .spawn(move || respond(request, origin, offline));
    if let Err(e) = spawned {
        eprintln!("a cover could not be fetched: {e}");
    }
}

fn respond(request: Request, origin: Option<String>, offline: bool) {
    let url = request
        .url()
        .strip_prefix(ROUTE)
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(name, _)| name == "u")
                .map(|(_, value)| value.into_owned())
        })
        .unwrap_or_default();
    let mut response = match picture(&url, offline) {
        Ok(bytes) => {
            let kind = if bytes.starts_with(b"\x89PNG") {
                "image/png"
            } else {
                "image/jpeg"
            };
            Response::from_data(bytes.to_vec())
                .with_status_code(200)
                .with_header(header("Content-Type", kind))
                .with_header(header("Cache-Control", "private, max-age=3600"))
                .with_header(header("Vary", "Origin"))
                .with_header(header("X-Content-Type-Options", "nosniff"))
        }
        Err(status) => Response::from_data(Vec::new()).with_status_code(StatusCode(status)),
    };
    // The window reads a cover's colours: it is told it may.
    if let Some(origin) = origin {
        response = response.with_header(header("Access-Control-Allow-Origin", &origin));
    }
    let _ = request.respond(response);
}

/// The cover at `url`: kept, or fetched held to the reach of the add-on that named it.
fn picture(url: &str, offline: bool) -> Result<Arc<Vec<u8>>, u16> {
    if url.is_empty() {
        return Err(400);
    }
    if let Some((_, bytes)) = KEPT.lock().unwrap().iter().find(|(u, _)| u == url) {
        return Ok(Arc::clone(bytes));
    }
    // Kept on this computer from an earlier look: no connection is needed for it.
    let file = kept_file(url);
    if let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok()).filter(|b| !b.is_empty()) {
        return Ok(remember(url, bytes));
    }
    // Offline (and on the listener's own files only, which is offline for good), nothing is asked of the internet.
    if offline {
        return Err(404);
    }
    if FETCHING.fetch_add(1, Ordering::AcqRel) >= MOST_AT_ONCE {
        FETCHING.fetch_sub(1, Ordering::AcqRel);
        return Err(503);
    }
    let reach = NOTED
        .lock()
        .unwrap()
        .get(url)
        .copied()
        .unwrap_or(Reach::Internet);
    let fetched = downloads::fetch_cover(url, reach);
    FETCHING.fetch_sub(1, Ordering::AcqRel);
    let bytes = fetched.ok_or(404u16)?;
    // Written beside its final name first, so a cover half-written is never read as one.
    if let Some(file) = file {
        let part = file.with_extension("part");
        if std::fs::write(&part, &bytes).is_ok() {
            let _ = std::fs::rename(&part, &file);
        }
    }
    Ok(remember(url, bytes))
}

/// Whether the cover at `url` is kept on this computer already.
pub(crate) fn is_kept(url: &str) -> bool {
    kept_file(url).is_some_and(|f| f.is_file())
}

/// Fetches the cover at `url` and keeps it on this computer, unless it is kept already. Whether it is kept now.
pub(crate) fn keep(url: &str) -> bool {
    is_kept(url) || (picture(url, false).is_ok() && is_kept(url))
}

/// Keeps a cover in memory among the last ones asked for.
fn remember(url: &str, bytes: Vec<u8>) -> Arc<Vec<u8>> {
    let bytes = Arc::new(bytes);
    let mut kept = KEPT.lock().unwrap();
    if kept.len() >= MOST_KEPT {
        kept.pop_front();
    }
    kept.push_back((url.to_string(), Arc::clone(&bytes)));
    bytes
}
