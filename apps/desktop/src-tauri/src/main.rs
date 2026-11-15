//! The desktop window: a shell around the UI, with the core running inside the same program.
//!
//! The shell does two things. It starts the core's local API on a free port of this computer, and it opens a window
//! on the built UI, telling the page where the core is. The page talks to the core over that API and nothing else, so
//! this shell could be replaced by another one without touching the core or the UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::{Context, Result};
use stashd::serve::{Core, Options};
use tauri::{WebviewUrl, WebviewWindowBuilder};

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

    let core = Core::bind(Options {
        db: library_path()?,
        ui: None,
        port: 0,
        bench_out,
    })?;
    let port = core.port();
    std::thread::Builder::new()
        .name("stash-core".into())
        .spawn(move || {
            if let Err(e) = core.run() {
                eprintln!("the core stopped: {e:#}");
            }
        })?;

    tauri::Builder::default()
        .setup(move |app| {
            // Set before any of the page's own script runs.
            let script = format!("window.__STASH_CORE__ = 'http://127.0.0.1:{port}'; window.__STASH_BENCH__ = {measuring};");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Stash")
                .inner_size(1400.0, 900.0)
                .min_inner_size(640.0, 400.0)
                .initialization_script(&script)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("running the window")?;
    Ok(())
}
