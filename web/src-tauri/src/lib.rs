//! XenoClaw desktop shell.
//!
//! Wraps the existing XenoClaw React frontend in a Tauri 2 window with native
//! chrome (custom titlebar via `decorations: false`), a system tray, native
//! notifications, window-state persistence, an auto-updater, and commands to
//! run the backend locally as a managed sidecar (see [`sidecar`]).

mod sidecar;

use sidecar::LocalBackend;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Bring the main window to the foreground.
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// ---- Window control commands (the custom titlebar calls these) -------------

#[tauri::command]
fn win_minimize(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

#[tauri::command]
fn win_toggle_maximize(window: tauri::Window) -> Result<(), String> {
    let maximized = window.is_maximized().map_err(|e| e.to_string())?;
    if maximized {
        window.unmaximize().map_err(|e| e.to_string())
    } else {
        window.maximize().map_err(|e| e.to_string())
    }
}

/// Closing the titlebar's "X" hides to the tray rather than quitting; use the
/// tray "Quit" item to exit fully.
#[tauri::command]
fn win_close(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
fn win_is_maximized(window: tauri::Window) -> Result<bool, String> {
    window.is_maximized().map_err(|e| e.to_string())
}

// ---- Misc commands ----------------------------------------------------------

/// Send a native OS notification (used when a response completes while the
/// window is unfocused).
#[tauri::command]
fn notify(app: AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn host_platform() -> String {
    std::env::consts::OS.to_string()
}

/// Check the configured updater endpoint. Returns the available version (if
/// any). Errors are returned as strings for the UI to surface.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<String>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => Ok(Some(update.version)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Build the system tray (show / hide / quit).
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show XenoClaw", true, None::<&str>)?;
    let hide_i = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show_i, &hide_i, &sep, &quit_i])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("XenoClaw")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "hide" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(LocalBackend::default())
        .setup(|app| {
            setup_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides to the tray instead of exiting, keeping
            // any local backend and notifications alive in the background.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            win_minimize,
            win_toggle_maximize,
            win_close,
            win_is_maximized,
            notify,
            app_version,
            host_platform,
            check_update,
            sidecar::local_backend_start,
            sidecar::local_backend_stop,
            sidecar::local_backend_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building XenoClaw desktop app");

    app.run(|app_handle, event| {
        // Make sure the sidecar dies with the app.
        if let RunEvent::Exit = event {
            if let Some(backend) = app_handle.try_state::<LocalBackend>() {
                if let Ok(mut guard) = backend.0.lock() {
                    if let Some(child) = guard.child_take() {
                        let _ = child.kill();
                    }
                }
            }
        }
    });
}
