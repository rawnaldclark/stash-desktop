//! The playing song's waveform for the seek bar (`stash_core::waveform`): a file is decoded once, the first time it is
//! asked for, and its waveform kept in a `Waveforms` folder beside the library (half a kilobyte a song), named by the
//! file's path, size and time, so a changed file is drawn again.
//!
//! - `GET /api/waveform/<track id>`: `{"peaks": [0..=255; 480]}`; 404 when the song is not a file here or its audio
//!   cannot be decoded (an Opus file); 503 while another file is being decoded (the window asks again).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use serde_json::json;
use stash_core::waveform::{SLICES, peaks};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, refuse, reply_json};

pub(crate) const ROUTE: &str = "/api/waveform/";

static FOLDER: OnceLock<PathBuf> = OnceLock::new();
/// One file is decoded at a time: a listener skipping through songs must not hold every worker.
static DECODING: Mutex<()> = Mutex::new(());

/// Says where waveforms are kept. Once, as the core starts; without it they are worked out each time.
pub(crate) fn keep_in(folder: PathBuf) {
    let _ = std::fs::create_dir_all(&folder);
    let _ = FOLDER.set(folder);
}

/// The file a waveform is kept in: the SHA-256 of the song's path, size and time, so nothing of the path is a path.
fn kept_file(locator: &str) -> Option<PathBuf> {
    use sha2::Digest as _;
    let meta = std::fs::metadata(locator).ok()?;
    let at = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let name: String = sha2::Sha256::digest(format!("{locator}\n{}\n{at}", meta.len()).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(FOLDER.get()?.join(name))
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    let Some(id) = path.strip_prefix(ROUTE).and_then(|id| id.parse::<i64>().ok()) else {
        return Ok(None);
    };
    if *request.method() != Method::Get {
        return Ok(None);
    }
    let Some(locator) = state.library.lock().unwrap().file_of(id)? else {
        return Ok(Some(refuse(404, "not a file on this computer")));
    };
    let kept = kept_file(&locator);
    // Kept: the waveform, or an empty file for one that could not be decoded (it is not tried again).
    if let Some(bytes) = kept.as_ref().and_then(|k| std::fs::read(k).ok()) {
        return Ok(Some(answer(&bytes)));
    }
    let Ok(_turn) = DECODING.try_lock() else {
        return Ok(Some(refuse(503, "another song's waveform is being drawn")));
    };
    // The library is let go while the file is decoded.
    let wave = peaks(std::path::Path::new(&locator)).unwrap_or_default();
    if let Some(k) = kept {
        let _ = std::fs::write(k, &wave);
    }
    Ok(Some(answer(&wave)))
}

fn answer(bytes: &[u8]) -> Reply {
    if bytes.len() == SLICES {
        reply_json(200, json!({ "peaks": bytes }))
    } else {
        refuse(404, "this file's audio cannot be drawn")
    }
}
