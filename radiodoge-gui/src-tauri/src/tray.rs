//! System tray icon and menu for RadioDoge.
//!
//! The tray icon:
//!   - Shows a Dogecoin-branded icon in the system tray
//!   - Displays connection status in the tooltip and the Status menu row
//!   - Left-click shows/hides the main window
//!   - Closing the window hides to the tray (keeps the radio session alive)
//!   - Menu has "Open RadioDoge", a live Status line, and "Quit" (Quit is the real exit)

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, Runtime,
};

/// Identifier for the tray this module builds.
///
/// Shared by [`setup_tray`] and [`update_tray_status`] so the lookup cannot
/// drift from the registration again.
const TRAY_ID: &str = "radiodoge-main-tray";

/// Managed handle for the non-clickable Status row so [`update_tray_status`]
/// can rewrite its label on connect/disconnect (tooltip alone was easy to miss).
struct TrayStatusItem<R: Runtime>(MenuItem<R>);

/// Build and register the system tray icon with its context menu.
pub fn setup_tray<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Open RadioDoge", true, None::<&str>)?;
    let status_item = MenuItem::with_id(app, "status", "Disconnected", false, None::<&str>)?;
    let separator = tauri::menu::PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&open_item, &status_item, &separator, &quit_item])?;

    // Tray icon is optional - gracefully skip if the icon resource isn't bundled
    // (can happen in CI / dev builds before icon generation runs).
    let Some(icon) = app.default_window_icon().cloned() else {
        log::warn!("[tray] no window icon available; skipping tray setup");
        return Ok(());
    };

    // Keep the Status item so connect/disconnect can update its label. MenuItem
    // is Arc-backed, so cloning is cheap.
    app.manage(TrayStatusItem(status_item.clone()));

    // v0.4.2 - An explicit id. `TrayIconBuilder::new()` generates one, and
    // `update_tray_status` looked the tray up by the empty string, which can
    // never match - so the tooltip never changed and the tray permanently read
    // whatever it was built with, on every connect and disconnect.
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("RadioDoge - Wireless Dogecoin")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                // Left-click: show/focus main window
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// Update the tray Status menu row and tooltip.
/// Call this whenever the connection state changes.
pub fn update_tray_status<R: Runtime>(
    app: &tauri::AppHandle<R>,
    connected: bool,
    port: Option<&str>,
) {
    let status_label = if connected {
        format!("Connected ({})", port.unwrap_or("?"))
    } else {
        "Disconnected".to_string()
    };
    let tooltip = if connected {
        format!("RadioDoge - Connected ({})", port.unwrap_or("?"))
    } else {
        "RadioDoge - Disconnected".to_string()
    };

    if let Some(item) = app.try_state::<TrayStatusItem<R>>() {
        if let Err(e) = item.0.set_text(&status_label) {
            log::debug!("[tray] status set_text failed: {e}");
        }
    }

    match app.tray_by_id(TRAY_ID) {
        Some(tray) => {
            let _ = tray.set_tooltip(Some(&tooltip));
        }
        // Not fatal - the tray is optional (see setup_tray) - but silence here
        // is what let the broken lookup go unnoticed.
        None => log::debug!("[tray] no tray with id {} - tooltip not updated", TRAY_ID),
    }
}
