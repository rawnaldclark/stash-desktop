//! Shortcuts that work in any app. The core keeps the listener's keys and does what each one asks; the shell holds
//! them system-wide and hands each key pressed back to the core as its action, over the core's local API like the
//! tray and the media controls.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::json;
use stashd::serve::KeysShell;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Where the core answers.
static CORE: OnceLock<String> = OnceLock::new();
/// The keys held, by the shortcut's own number: the action each one asks for.
static HELD: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// The plugin that holds keys system-wide. A key pressed is handed to the core off the window's thread, so a busy
/// core never freezes the window.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|_app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let action = HELD
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|held| held.get(&shortcut.id()).cloned());
            let (Some(action), Some(base)) = (action, CORE.get().cloned()) else {
                return;
            };
            std::thread::spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .http_status_as_error(false)
                    .timeout_global(Some(Duration::from_secs(5)))
                    .build()
                    .into();
                let _ = agent
                    .post(format!("{base}/api/shortcut"))
                    .send_json(json!({ "action": action }));
            });
        })
        .build()
}

/// What the core asks to hold: each time, the keys held before are let go of and these are held instead. Answers the
/// keys that could not be, with why, in the listener's words.
pub fn keys_shell<R: Runtime>(app: AppHandle<R>, port: u16) -> KeysShell {
    let _ = CORE.set(format!("http://127.0.0.1:{port}"));
    Arc::new(move |wanted: &[(String, String)]| {
        let shortcuts = app.global_shortcut();
        if let Err(e) = shortcuts.unregister_all() {
            eprintln!("the shortcuts held were not let go of: {e}");
        }
        let mut held = HashMap::new();
        let mut trouble = Vec::new();
        for (action, keys) in wanted {
            let why = match Shortcut::from_str(keys) {
                Err(_) => Some("Stash cannot use those keys"),
                Ok(shortcut) if held.contains_key(&shortcut.id()) => {
                    Some("Another of these shortcuts has those keys")
                }
                Ok(shortcut) => match shortcuts.register(shortcut) {
                    Ok(()) => {
                        held.insert(shortcut.id(), action.clone());
                        None
                    }
                    Err(_) => Some("Another app uses those keys"),
                },
            };
            if let Some(why) = why {
                trouble.push((action.clone(), why.to_string()));
            }
        }
        *HELD.lock().unwrap() = Some(held);
        trouble
    })
}
