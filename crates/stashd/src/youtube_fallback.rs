//! yt-dlp for the songs YouTube's apps will not play to Stash (see `stash_core::youtube::ytdlp`): the listener's switch,
//! and Stash's own copy, fetched when they ask (with Deno beside it when the computer has no JavaScript runtime
//! yt-dlp can use). Its copy lives in `tools`, beside the library.
//!
//! `GET /api/youtube/ytdlp`: what there is. `POST /api/youtube/ytdlp {"on"}`: the switch (on unless switched off).
//! `POST /api/youtube/ytdlp/install`: fetch Stash's copy (again: the newest).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{Value, json};
use stash_core::library::Library;
use stash_core::youtube::YouTube;
use stash_core::youtube::ytdlp::{self, Progress, YtDlp};
use tiny_http::{Method, Request};

use crate::serve::{Reply, State, body_json, offline, refuse, reply_json};

/// The switch ("1": off).
const OFF: &str = "youtube.ytdlp.off";

pub(crate) struct TubeFallback {
    library: Arc<Mutex<Library>>,
    youtube: Arc<YouTube>,
    /// Where Stash keeps its own copy.
    tools: PathBuf,
    /// A fetch running, and how far it is.
    fetching: Mutex<Option<Progress>>,
    /// Why the last fetch failed.
    trouble: Mutex<Option<String>>,
    /// The copy last asked for its version, and what it said (it starts slowly: asked once).
    version: Mutex<Option<(PathBuf, String)>>,
}

impl TubeFallback {
    /// yt-dlp for `youtube`: Stash's own copy in `tools`, else the system's, unless the listener switched it off.
    pub(crate) fn new(library: Arc<Mutex<Library>>, youtube: Arc<YouTube>, tools: PathBuf) -> Self {
        let fallback = Self {
            library,
            youtube,
            tools,
            fetching: Mutex::default(),
            trouble: Mutex::default(),
            version: Mutex::default(),
        };
        fallback.take_up();
        fallback
    }

    fn on(&self) -> bool {
        self.library
            .lock()
            .unwrap()
            .setting(OFF)
            .ok()
            .flatten()
            .as_deref()
            != Some("1")
    }

    /// Hands YouTube the copy there is, when the switch is on.
    fn take_up(&self) {
        let found = self.on().then(|| YtDlp::find(&self.tools)).flatten();
        self.youtube.set_fallback(found);
    }

    fn version_of(&self, found: &YtDlp) -> Option<String> {
        if let Some((exe, said)) = self.version.lock().unwrap().as_ref()
            && *exe == found.exe
        {
            return Some(said.clone());
        }
        let said = found.version()?;
        *self.version.lock().unwrap() = Some((found.exe.clone(), said.clone()));
        Some(said)
    }

    fn told(&self) -> Value {
        let found = YtDlp::find(&self.tools);
        let fetching = self.fetching.lock().unwrap().clone();
        json!({
            "on": self.on(),
            "found": found.as_ref().filter(|_| fetching.is_none()).map(|found| json!({
                "own": found.own,
                "version": self.version_of(found),
                "runtime": found.runtime(),
            })),
            "fetching": fetching.map(|p| json!({ "what": p.what, "got": p.got, "total": p.total })),
            "trouble": *self.trouble.lock().unwrap(),
            "can_fetch": ytdlp::can_install(),
            // What a fetch would bring: Deno too, when there is no runtime yt-dlp can use.
            "with_deno": ytdlp::runtime_near(Some(&self.tools)).is_none(),
        })
    }

    /// Fetches Stash's own copy on a thread of its own. False when one is being fetched already.
    fn fetch(self: &Arc<Self>) -> bool {
        {
            let mut fetching = self.fetching.lock().unwrap();
            if fetching.is_some() {
                return false;
            }
            *fetching = Some(Progress {
                what: "yt-dlp".to_string(),
                ..Progress::default()
            });
        }
        *self.trouble.lock().unwrap() = None;
        let fallback = Arc::clone(self);
        std::thread::spawn(move || {
            let with_deno = ytdlp::runtime_near(Some(&fallback.tools)).is_none();
            let told = |progress: Progress| *fallback.fetching.lock().unwrap() = Some(progress);
            let fetched = ytdlp::install(&fallback.tools, with_deno, &told);
            *fallback.version.lock().unwrap() = None;
            if let Err(why) = fetched {
                *fallback.trouble.lock().unwrap() = Some(why);
            }
            fallback.take_up();
            *fallback.fetching.lock().unwrap() = None;
        });
        true
    }
}

pub(crate) fn route(state: &State, request: &mut Request, path: &str) -> Result<Option<Reply>> {
    if !path.starts_with("/api/youtube/ytdlp") {
        return Ok(None);
    }
    let Some(fallback) = state
        .youtube_fallback()
        .filter(|_| state.youtube_on().is_some())
    else {
        return Ok(Some(refuse(404, "YouTube Music is switched off")));
    };
    let reply = match (request.method(), path) {
        (Method::Get, "/api/youtube/ytdlp") => reply_json(200, fallback.told()),
        (Method::Post, "/api/youtube/ytdlp") => {
            let on = body_json(request)?
                .get("on")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            fallback
                .library
                .lock()
                .unwrap()
                .set_setting(OFF, if on { "0" } else { "1" })?;
            fallback.take_up();
            reply_json(200, fallback.told())
        }
        (Method::Post, "/api/youtube/ytdlp/install") => {
            if !ytdlp::can_install() {
                return Ok(Some(refuse(404, "yt-dlp has no build for this computer")));
            }
            if offline(state) {
                return Ok(Some(refuse(
                    409,
                    "Stash is offline: go online to fetch yt-dlp",
                )));
            }
            fallback.fetch();
            reply_json(202, fallback.told())
        }
        _ => return Ok(None),
    };
    Ok(Some(reply))
}
