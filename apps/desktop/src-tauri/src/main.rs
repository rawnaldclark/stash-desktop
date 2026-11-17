//! The desktop window: a shell around the UI, with the core running inside the same program.
//!
//! The shell does two things. It starts the core's local API on a free port of this computer, and it opens a window
//! on the built UI, telling the page where the core is. The page talks to the core over that API and nothing else, so
//! this shell could be replaced by another one without touching the core or the UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod media_controls;
mod shortcuts;
mod sign_in;
mod tray;
mod windows;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use stashd::serve::{Core, Options};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// A key for this run, given to the window and to no one else: the core answers the window's origin only with it, as
/// a browser gives that origin to any page at tauri.localhost.
fn run_key() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no random numbers: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The library to open: the one `STASH_DB` names, else `library.db` in this user's Stash data folder.
fn library_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("STASH_DB") {
        return Ok(PathBuf::from(path));
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
    };
    let folder = base.context("this user has no data folder")?.join("Stash");
    std::fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    Ok(folder.join("library.db"))
}

fn main() -> Result<()> {
    // `STASH_BENCH_OUT` asks the page to measure itself and the core to write the numbers there.
    let bench_out = std::env::var_os("STASH_BENCH_OUT").map(PathBuf::from);
    let measuring = bench_out.is_some();

    // One Stash on the listener's own library: opening it again brings the open window to the front instead of a
    // second program playing over the first. A library named by `STASH_DB` (a test, a measurement) runs on its own.
    let own_library = std::env::var_os("STASH_DB").is_none();
    let db = library_path()?;
    // Where Google's sign-in for YouTube keeps its browsing data while it is open (beside the library, apart from the
    // main window's).
    let sign_in_folder = db
        .parent()
        .map(|folder| folder.join("youtube-sign-in"))
        .unwrap_or_else(|| PathBuf::from("youtube-sign-in"));

    let mut shell = tauri::Builder::default();
    if own_library {
        // First, so a second start ends before anything else of it runs (its core is never opened).
        shell = shell.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::full(app);
        }));
    }
    if !measuring && own_library {
        // The window opens where it was closed, at the size it had. A measurement keeps the one size it is run at, and
        // a library of its own (a test) leaves the listener's window where it was.
        shell = shell.plugin(tauri_plugin_window_state::Builder::default().build());
    }
    if !measuring {
        // Shortcuts that work in any app (the listener's choice, off unless switched on in Settings).
        shell = shell.plugin(shortcuts::plugin());
        // Links the core opens in the browser (a station's page on Last.fm). The window is granted none of it: only
        // the core asks, through `with_opener`.
        shell = shell.plugin(tauri_plugin_opener::init());
    }
    shell
        .setup(move |app| {
            let core = Core::bind(Options {
                db,
                ui: None,
                port: 0,
                bench_out,
                audio: Some(stash_core::audio::Output::Device {
                    exclusive: std::env::var_os("STASH_EXCLUSIVE").is_some_and(|v| v == "1"),
                }),
                mpv: None,
                play_counts_between_seconds: None,
            })?;
            let port = core.port();
            let key = run_key()?;
            let core = core.with_key(key.clone());
            // Radio and autoplay that knows what is like what, when this build was given the address of Stash's
            // Last.fm proxy (it holds the key; see `stashd::radio`). Never in the source, and never shown.
            let core = match option_env!("STASH_LASTFM_PROXY").filter(|p| !p.trim().is_empty()) {
                Some(proxy) => core.with_similar(proxy),
                None => core,
            };
            // YouTube Music, built in: there before the listener adds any add-on. Left out of a measurement, and
            // when a check of the app asks for it (STASH_NO_YOUTUBE=1), so that neither asks YouTube anything.
            let core = if measuring || std::env::var_os("STASH_NO_YOUTUBE").is_some_and(|v| v == "1") {
                core
            } else {
                core.with_youtube(Default::default(), stash_core::reach::Reach::Internet)
            };
            // The page may ask for the mini player (a measurement keeps the one window it measures).
            let core = if measuring {
                core
            } else {
                let handle = app.handle().clone();
                let opener = app.handle().clone();
                core.with_window(Arc::new(move |view| windows::show(&handle, view)))
                    .with_sign_in(sign_in::shell(app.handle().clone(), sign_in_folder.clone()))
                    .with_shortcuts(shortcuts::keys_shell(app.handle().clone(), port))
                    .with_opener(Arc::new(move |link: &str| {
                        use tauri_plugin_opener::OpenerExt;
                        // Only Last.fm's pages, whatever asks.
                        link.starts_with("https://www.last.fm/")
                            && opener.opener().open_url(link, None::<&str>).is_ok()
                    }))
            };
            std::thread::Builder::new()
                .name("stash-core".into())
                .spawn(move || {
                    if let Err(e) = core.run() {
                        eprintln!("the core stopped: {e:#}");
                    }
                })?;
            // Set before any of the page's own script runs.
            let script = format!("window.__STASH_CORE__ = 'http://127.0.0.1:{port}'; window.__STASH_KEY__ = '{key}'; window.__STASH_BENCH__ = {measuring};");
            let window = WebviewWindowBuilder::new(app, windows::MAIN, WebviewUrl::App("index.html".into()))
                .title("Stash")
                .inner_size(1400.0, 900.0)
                .min_inner_size(640.0, 400.0)
                .initialization_script(&script)
                .build()?;
            app.manage(windows::PageScript(script));
            if !measuring {
                media_controls::start(app.handle().clone(), &window, port);
                tray::start(app.handle(), port);
            }
            Ok(())
        })
        // Closing the window ends Stash, unless the listener chose to keep it playing in the tray (and there is one).
        // Closing the mini player brings the full window back.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event
                && window.label() == windows::MINI
            {
                windows::show_main(window.app_handle());
            }
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == windows::MAIN
            {
                let in_tray = window.app_handle().tray_by_id(tray::ID).is_some();
                if in_tray && tray::keeps_playing() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .context("running the window")?;
    Ok(())
}
