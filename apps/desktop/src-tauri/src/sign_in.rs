//! Google's sign-in for YouTube, in a window of its own, the way Stash for Android signs in to YouTube
//! (`YouTubeLoginWebView`): the listener signs in on Google's own pages, and once the session's cookies are set
//! (`SAPISID`, and `LOGIN_INFO`), they go to the core, the window's browsing data is wiped and the window closes.
//!
//! The window is Google's page and nothing else: it is given no script, no key to the core and no permission (see
//! `capabilities/`), and it keeps its data in a folder apart from the main window's, deleted before it opens and after
//! it closes, so each sign-in starts fresh and nothing of it stays behind but the cookies the core keeps.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use stashd::serve::{SignInShell, SignedIn};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "youtube-sign-in";

/// YouTube Music's own "Sign in": Google's sign-in for YouTube, then YouTube's (which sets `LOGIN_INFO`), then YouTube
/// Music.
const START: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&passive=true&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3Dhttps%253A%252F%252Fmusic.youtube.com%252F";
const MUSIC: &str = "https://music.youtube.com/";
/// How often the window's cookies are looked at.
const LOOK_EVERY: Duration = Duration::from_millis(800);
/// Once `SAPISID` is set, how long `LOGIN_INFO` is waited for before the session is taken without it (the core's
/// requests need only the first; yt-dlp also wants the second).
const LOGIN_INFO_WAIT: Duration = Duration::from_secs(8);
/// How long a sign-in may take before the window gives up.
const GIVE_UP: Duration = Duration::from_secs(20 * 60);

/// The shell's part of signing in: opens the window (or brings the open one forward) and hands the core the session
/// when the listener has signed in. The window keeps its data in `folder`.
pub fn shell(app: AppHandle, folder: PathBuf) -> SignInShell {
    Arc::new(move |done: SignedIn| {
        // One window: asked again, it comes forward, and the first ask's answer is the one that comes.
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.unminimize();
            let _ = window.set_focus();
            return true;
        }
        let (opened, told) = mpsc::channel();
        let (handle, fresh) = (app.clone(), folder.clone());
        let asked = app.run_on_main_thread(move || {
            // Wiped before it opens: no one else's session, and none half-made from last time.
            let _ = std::fs::remove_dir_all(&fresh);
            let start = START.parse().expect("the sign-in address is well-formed");
            let built = WebviewWindowBuilder::new(&handle, LABEL, WebviewUrl::External(start))
                .title("Sign in to YouTube Music")
                .inner_size(480.0, 720.0)
                .min_inner_size(380.0, 520.0)
                .data_directory(fresh)
                .build();
            let _ = opened.send(built.is_ok());
        });
        let opened = asked.is_ok() && told.recv_timeout(Duration::from_secs(20)).unwrap_or(false);
        if !opened {
            return false;
        }
        let (app, folder) = (app.clone(), folder.clone());
        std::thread::Builder::new()
            .name("youtube-sign-in".into())
            .spawn(move || {
                done(watch(&app));
                forget(&folder);
            })
            .is_ok()
    })
}

/// Deletes the window's folder once it has closed: the browser behind it lets go of its files a moment after the
/// window goes, so it is tried a few times. What is left (a browser still holding on) goes before the next sign-in.
fn forget(folder: &Path) {
    for wait in [2, 5, 15] {
        std::thread::sleep(Duration::from_secs(wait));
        if std::fs::remove_dir_all(folder).is_ok() || !folder.exists() {
            return;
        }
    }
}

/// The session's cookies once the listener has signed in (`Cookie` header form); nothing when the window was closed
/// first, or it took too long. The window is wiped and closed either way (and its folder deleted after: `forget`).
fn watch(app: &AppHandle) -> Option<String> {
    let music: Url = MUSIC
        .parse()
        .expect("YouTube Music's address is well-formed");
    let began = Instant::now();
    let mut first_seen: Option<Instant> = None;
    loop {
        std::thread::sleep(LOOK_EVERY);
        // Closed by the listener: nothing.
        let window = app.get_webview_window(LABEL)?;
        // Read off the main thread (on Windows it would wait on itself).
        let cookies = window.cookies_for_url(music.clone()).unwrap_or_default();
        let has = |name: &str| {
            cookies
                .iter()
                .any(|c| c.name() == name && !c.value().is_empty())
        };
        let signed_in = has("SAPISID") || has("__Secure-3PAPISID");
        let session = signed_in.then(|| {
            cookies
                .iter()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect::<Vec<_>>()
                .join("; ")
        });
        if signed_in {
            first_seen.get_or_insert_with(Instant::now);
        }
        let ready = signed_in
            && (has("LOGIN_INFO") || first_seen.is_some_and(|at| at.elapsed() > LOGIN_INFO_WAIT));
        let gave_up = began.elapsed() > GIVE_UP;
        if ready || gave_up {
            let _ = window.clear_all_browsing_data();
            let _ = window.close();
            return session.filter(|_| ready);
        }
    }
}
