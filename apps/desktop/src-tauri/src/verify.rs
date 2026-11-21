//! A Mornye extension's verification page (or its sign-in page), in a window of its own, the way Lokal opens it: the
//! person passes the page on the service's own site; when the page sends the window to the extension's callback
//! (`spotiflac://session-grant?…`, or a page that carries the grant), the window stops there, hands the address to
//! the core (which exchanges the grant for the extension's session), and closes. A cloud provider's sign-in opens the
//! same way, and stops at the provider's redirect (`db-<key>://…`, `com.stash.app://oauth/onedrive`); so does Stash's
//! Tidal's quick check, which stops at its page's hand-back (`<the add-on>/register/done#turnstile=…`).
//!
//! The window is the service's page and nothing else: it is given no script, no key to the core and no permission,
//! opens only `https`, `about` and `data` addresses, and keeps its data in a folder apart from the main window's,
//! deleted before it opens and after it closes. The callback's scheme is never registered with the system: it is
//! caught here, so it cannot reach Mornye or another app that answers it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use stashd::serve::{VerifyShell, VerifyWindow};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "mornye-verify";

/// The shell's part: opens the window (closing one left open for another extension).
pub fn shell(app: AppHandle, folder: PathBuf) -> VerifyShell {
    Arc::new(move |asked: VerifyWindow| {
        let VerifyWindow {
            url,
            title,
            catches,
            caught,
            closed,
        } = asked;
        let Ok(start) = url.parse::<tauri::Url>() else {
            return false;
        };
        if start.scheme() != "https" {
            return false;
        }
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.destroy();
        }
        let closed = Arc::new(Mutex::new(Some(closed)));
        let (opened, told) = mpsc::channel();
        let (handle, fresh) = (app.clone(), folder.clone());
        let asked = app.run_on_main_thread(move || {
            let _ = std::fs::remove_dir_all(&fresh);
            let navigating = {
                let handle = handle.clone();
                move |to: &tauri::Url| -> bool {
                    if catches(to.as_str()) {
                        let (caught, to) = (Arc::clone(&caught), to.to_string());
                        let handle = handle.clone();
                        std::thread::spawn(move || {
                            caught(to);
                            if let Some(window) = handle.get_webview_window(LABEL) {
                                let _ = window.clear_all_browsing_data();
                                let _ = window.close();
                            }
                        });
                        return false;
                    }
                    matches!(to.scheme(), "https" | "about" | "data")
                }
            };
            let built = WebviewWindowBuilder::new(&handle, LABEL, WebviewUrl::External(start))
                .title(if title.is_empty() {
                    "Stash"
                } else {
                    title.as_str()
                })
                .inner_size(520.0, 760.0)
                .min_inner_size(380.0, 520.0)
                .data_directory(fresh.clone())
                .on_navigation(navigating)
                .build();
            match built {
                Ok(window) => {
                    let (closed, fresh) = (Arc::clone(&closed), fresh.clone());
                    window.on_window_event(move |event| {
                        if let tauri::WindowEvent::Destroyed = event {
                            if let Some(closed) = closed.lock().unwrap().take() {
                                closed();
                            }
                            let fresh = fresh.clone();
                            std::thread::spawn(move || forget(&fresh));
                        }
                    });
                    let _ = opened.send(true);
                }
                Err(_) => {
                    let _ = opened.send(false);
                }
            }
        });
        asked.is_ok() && told.recv_timeout(Duration::from_secs(20)).unwrap_or(false)
    })
}

/// Deletes the window's folder once the browser behind it lets go of its files.
fn forget(folder: &Path) {
    for wait in [2, 5, 15] {
        std::thread::sleep(Duration::from_secs(wait));
        if std::fs::remove_dir_all(folder).is_ok() || !folder.exists() {
            return;
        }
    }
}
