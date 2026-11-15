//! The two ways Stash shows itself: the full window, and the mini player, a small window that stays above the others
//! with the song, its cover, where it is, and the controls. The page asks for one or the other through the core
//! (`POST /api/window`), so it talks to nothing but the core; the core hands the ask here.
//!
//! Only one shows at a time: the mini player hides the full window, and the full window closes the mini player. The
//! mini player has no frame of its own; it is moved by its background, and it opens where it was last left.

use stashd::serve::View;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MAIN: &str = "main";
pub const MINI: &str = "mini";

/// What every window runs before its page: where the core is.
pub struct PageScript(pub String);

/// Shows `view`, from any thread (the core asks from its own). Says whether the ask was taken.
pub fn show(app: &AppHandle, view: View) -> bool {
    let handle = app.clone();
    app.run_on_main_thread(move || match view {
        View::Full => full(&handle),
        View::Mini => {
            if let Err(e) = mini(&handle) {
                eprintln!("the mini player did not open: {e}");
                full(&handle);
            }
        }
    })
    .is_ok()
}

/// The full window, back from wherever it went (behind the mini player, in the tray, minimized).
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The full window, and the mini player closed.
pub fn full(app: &AppHandle) {
    show_main(app);
    if let Some(mini) = app.get_webview_window(MINI) {
        // Closed, not destroyed: where it was is kept for next time.
        let _ = mini.close();
    }
}

fn mini(app: &AppHandle) -> tauri::Result<()> {
    let window = match app.get_webview_window(MINI) {
        Some(window) => window,
        None => {
            let script = app.state::<PageScript>().0.clone();
            WebviewWindowBuilder::new(app, MINI, WebviewUrl::App("index.html?view=mini".into()))
                .title("Stash")
                .inner_size(400.0, 120.0)
                .min_inner_size(300.0, 112.0)
                .decorations(false)
                .always_on_top(true)
                .resizable(true)
                .initialization_script(&script)
                .build()?
        }
    };
    window.show()?;
    window.set_focus()?;
    if let Some(main) = app.get_webview_window(MAIN) {
        main.hide()?;
    }
    Ok(())
}
