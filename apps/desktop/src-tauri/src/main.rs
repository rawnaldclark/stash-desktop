//! The desktop window: a shell around the UI, with the core running inside the same program.
//!
//! The shell does two things. It starts the core's local API on a free port of this computer, and it opens a window
//! on the built UI, telling the page where the core is. The page talks to the core over that API and nothing else, so
//! this shell could be replaced by another one without touching the core or the UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod media_controls;

use std::path::PathBuf;

use anyhow::{Context, Result};
use stashd::serve::{Core, Options};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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

    let mut shell = tauri::Builder::default();
    if own_library {
        // First, so a second start ends before anything else of it runs (its core is never opened).
        shell = shell.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }
    if !measuring {
        // The window opens where it was closed, at the size it had (a measurement keeps the one size it is run at).
        shell = shell.plugin(tauri_plugin_window_state::Builder::default().build());
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
            std::thread::Builder::new()
                .name("stash-core".into())
                .spawn(move || {
                    if let Err(e) = core.run() {
                        eprintln!("the core stopped: {e:#}");
                    }
                })?;
            // Set before any of the page's own script runs.
            let script = format!("window.__STASH_CORE__ = 'http://127.0.0.1:{port}'; window.__STASH_BENCH__ = {measuring};");
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Stash")
                .inner_size(1400.0, 900.0)
                .min_inner_size(640.0, 400.0)
                .initialization_script(&script)
                .build()?;
            if !measuring {
                media_controls::start(app.handle().clone(), &window, port);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("running the window")?;
    Ok(())
}
