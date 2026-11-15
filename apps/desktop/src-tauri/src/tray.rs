//! The tray icon (by the clock on Windows, in the menu bar on macOS, in the desktop's tray on Linux): its tip says
//! what plays, a click brings the window back, and its menu has play/pause, next, previous, the window, the mini player
//! and quit.
//!
//! When the listener chose it (Settings › Appearance), closing the window leaves Stash playing here instead of
//! ending it. Like the media controls, the tray drives the player over the core's local API.

use std::time::Duration;

use serde_json::{Value, json};
use tauri::AppHandle;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::windows;

/// The tray icon's id.
pub const ID: &str = "stash";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into()
}

fn read(agent: &ureq::Agent, base: &str, path: &str) -> Option<Value> {
    agent
        .get(format!("{base}{path}"))
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()
}

fn post(agent: &ureq::Agent, base: &str, path: &str, body: Value) {
    let _ = agent.post(format!("{base}{path}")).send_json(body);
}

/// Whether the listener chose to keep Stash in the tray when the window is closed.
pub fn keeps_playing(port: u16) -> bool {
    read(
        &agent(),
        &format!("http://127.0.0.1:{port}"),
        "/api/settings/window",
    )
    .and_then(|v| v["tray"].as_bool())
    .unwrap_or(false)
}

/// Puts the icon in the tray and keeps its tip on what plays. Without a tray on this system, it says why and
/// leaves it: closing the window then always ends Stash.
pub fn start(app: &AppHandle, port: u16) {
    if let Err(e) = build(app, port) {
        eprintln!("no tray icon: {e}");
    }
}

fn build(app: &AppHandle, port: u16) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Play / pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let previous = MenuItem::with_id(app, "previous", "Previous", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "Show Stash", true, None::<&str>)?;
    let mini = MenuItem::with_id(app, "mini", "Mini player", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Stash", true, None::<&str>)?;
    let line = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&toggle, &next, &previous, &line, &show_item, &mini, &quit],
    )?;
    let base = format!("http://127.0.0.1:{port}");
    let mut builder = TrayIconBuilder::with_id(ID)
        .tooltip("Stash")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event({
            let base = base.clone();
            move |app, event| {
                let agent = agent();
                match event.id().as_ref() {
                    "toggle" => {
                        let playing = read(&agent, &base, "/api/player")
                            .is_some_and(|p| p["status"] == "playing");
                        post(
                            &agent,
                            &base,
                            "/api/player/pause",
                            json!({ "paused": playing }),
                        );
                    }
                    "next" => post(&agent, &base, "/api/player/next", json!({})),
                    "previous" => post(&agent, &base, "/api/player/previous", json!({})),
                    "show" => windows::full(app),
                    "mini" => {
                        windows::show(app, stashd::serve::View::Mini);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::full(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;

    // The tip follows the song: "Title · Artist", or just "Stash".
    std::thread::Builder::new()
        .name("stash-tray".into())
        .spawn(move || {
            let agent = agent();
            let mut said = String::new();
            loop {
                let tip = read(&agent, &base, "/api/player")
                    .and_then(|p| {
                        let track = p.get("track")?;
                        let title = track["title"].as_str()?;
                        let artist = track["artist"].as_str().unwrap_or_default();
                        Some(format!("{title} · {artist}"))
                    })
                    .map(|now| now.chars().take(120).collect::<String>())
                    .unwrap_or_else(|| "Stash".into());
                if tip != said {
                    let _ = tray.set_tooltip(Some(&tip));
                    said = tip;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })?;
    Ok(())
}
