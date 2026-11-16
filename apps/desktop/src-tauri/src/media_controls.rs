//! The system's own media controls: the keyboard's play, next and previous keys, and the media overlay of the
//! system (on Windows the one that shows over the volume, on Linux the desktop's player applet, on macOS Now
//! Playing). They show what Stash plays, with its cover, and their buttons drive it.
//!
//! The shell follows the core's player over the same local API the window uses, and sends the buttons to it the
//! same way: the core does not know these controls exist.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use tauri::{AppHandle, Manager, WebviewWindow};

/// How often the player is looked at.
const LOOK: Duration = Duration::from_millis(500);
/// How far the seek buttons move, when the system does not say.
const SEEK_STEP: f64 = 10.0;
/// While a track plays, the system is told its place this often (it does not count on by itself everywhere).
const PLACE_EVERY: Duration = Duration::from_secs(2);

/// Starts the controls for `window` and a thread that keeps them up to date. Without controls on this system
/// (no session bus, an old Windows), it says why and leaves it: Stash works without them.
pub fn start(app: AppHandle, window: &WebviewWindow, port: u16) {
    #[cfg(windows)]
    let hwnd = match window.hwnd() {
        Ok(hwnd) => Some(hwnd.0),
        Err(e) => {
            eprintln!("no media controls: the window has no handle ({e})");
            return;
        }
    };
    #[cfg(not(windows))]
    let hwnd = {
        let _ = window;
        None
    };
    let config = PlatformConfig {
        display_name: "Stash",
        dbus_name: "stash_desktop",
        hwnd,
    };
    let mut controls = match MediaControls::new(config) {
        Ok(controls) => controls,
        Err(e) => {
            eprintln!("no media controls: {e}");
            return;
        }
    };

    let base = format!("http://127.0.0.1:{port}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    // What the player was doing at the last look: a play/pause key toggles from it, a seek key moves from it.
    let last = Arc::new(Mutex::new((String::from("idle"), 0.0_f64)));

    let attached = {
        let (agent, base, last) = (agent.clone(), base.clone(), Arc::clone(&last));
        controls.attach(move |event| {
            let post = |path: &str, body: Value| {
                let _ = agent
                    .post(format!("{base}/api/player/{path}"))
                    .send_json(body);
            };
            let (status, seconds) = last.lock().unwrap().clone();
            let seek_by = |by: f64| post("seek", json!({ "seconds": (seconds + by).max(0.0) }));
            match event {
                MediaControlEvent::Play => post("pause", json!({ "paused": false })),
                // The stop button pauses: the queue is the listener's, and a key should not throw it away.
                MediaControlEvent::Pause | MediaControlEvent::Stop => {
                    post("pause", json!({ "paused": true }))
                }
                MediaControlEvent::Toggle => {
                    post("pause", json!({ "paused": status == "playing" }))
                }
                MediaControlEvent::Next => post("next", json!({})),
                MediaControlEvent::Previous => post("previous", json!({})),
                MediaControlEvent::Seek(SeekDirection::Forward) => seek_by(SEEK_STEP),
                MediaControlEvent::Seek(SeekDirection::Backward) => seek_by(-SEEK_STEP),
                MediaControlEvent::SeekBy(SeekDirection::Forward, by) => seek_by(by.as_secs_f64()),
                MediaControlEvent::SeekBy(SeekDirection::Backward, by) => {
                    seek_by(-by.as_secs_f64())
                }
                MediaControlEvent::SetPosition(MediaPosition(at)) => {
                    post("seek", json!({ "seconds": at.as_secs_f64() }))
                }
                MediaControlEvent::Raise => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            }
        })
    };
    if let Err(e) = attached {
        eprintln!("no media controls: {e}");
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("stash-media-controls".into())
        .spawn(move || follow(controls, agent, base, last));
    if let Err(e) = spawned {
        eprintln!("no media controls: {e}");
    }
}

/// Looks at the player and tells the system what changed: the track (title, artist, album, cover, length), and
/// whether it plays and where.
fn follow(
    mut controls: MediaControls,
    agent: ureq::Agent,
    base: String,
    last: Arc<Mutex<(String, f64)>>,
) {
    let mut shown: Option<String> = None;
    let mut told: Option<(String, Instant, f64)> = None;
    loop {
        std::thread::sleep(LOOK);
        let Ok(mut answer) = agent.get(format!("{base}/api/player")).call() else {
            continue;
        };
        let Ok(state) = answer.body_mut().read_json::<Value>() else {
            continue;
        };
        let status = state["status"].as_str().unwrap_or("idle").to_string();
        let seconds = state["position_ms"].as_f64().unwrap_or(0.0) / 1000.0;
        *last.lock().unwrap() = (status.clone(), seconds);

        let track = &state["track"];
        let length_ms = state["duration_ms"]
            .as_u64()
            .filter(|ms| *ms > 0)
            .or_else(|| track["duration_ms"].as_u64());
        // The track by its place in the queue, its words and its length (a length found once it opens is news).
        let which = (!track.is_null()).then(|| {
            format!(
                "{}|{}|{}|{}",
                track["qid"],
                track["title"],
                track["artist"],
                length_ms.unwrap_or(0)
            )
        });
        if which != shown {
            let text = |name: &str| track[name].as_str().filter(|s| !s.is_empty());
            // A cover on a server is asked of the core, which fetches it held to where the add-on that named it may
            // send Stash: the system would fetch it as it is, redirects and all.
            let art = text("art").map(|art| {
                if art.starts_with('/') {
                    format!("{base}{art}")
                } else {
                    let escaped: String = art
                        .bytes()
                        .map(|b| match b {
                            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                                (b as char).to_string()
                            }
                            _ => format!("%{b:02X}"),
                        })
                        .collect();
                    format!("{base}/api/art/remote?u={escaped}")
                }
            });
            let metadata = MediaMetadata {
                title: text("title"),
                artist: text("artist"),
                album: text("album"),
                cover_url: art.as_deref().filter(|a| a.starts_with("http")),
                duration: length_ms.map(Duration::from_millis),
            };
            if controls.set_metadata(metadata).is_ok() {
                shown = which;
                told = None;
            }
        }

        let progress = Some(MediaPosition(Duration::from_secs_f64(seconds.max(0.0))));
        let playback = match status.as_str() {
            "playing" | "loading" => MediaPlayback::Playing { progress },
            "paused" => MediaPlayback::Paused { progress },
            _ => MediaPlayback::Stopped,
        };
        // Told again when it starts or stops, every few seconds while it plays, and when the place jumped (a seek).
        let due = match &told {
            None => true,
            Some((was, at, from)) => {
                let expected = if was == "playing" {
                    from + at.elapsed().as_secs_f64()
                } else {
                    *from
                };
                *was != status
                    || (status == "playing" && at.elapsed() >= PLACE_EVERY)
                    || (seconds - expected).abs() > 1.5
            }
        };
        if due && controls.set_playback(playback).is_ok() {
            told = Some((status, Instant::now(), seconds));
        }
    }
}
