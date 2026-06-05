//! Local-backend (sidecar) management.
//!
//! In "local" mode the desktop app runs the XenoClaw backend itself as a
//! bundled sidecar (`binaries/xenoclaw-<target-triple>`). We pick a free port,
//! spawn `xenoclaw serve` bound to `127.0.0.1:<port>` (via the
//! `XENOCLAW_API_PORT` / `XENOCLAW_API_HOST` env overrides the backend honours),
//! stream its stdout/stderr to the frontend as `local-backend-log` events, and
//! expose start/stop/status commands. The actual readiness probe (hitting
//! `/api/v1/health`) is done on the frontend connection poller, so this module
//! stays HTTP-client-free.

use std::net::TcpListener;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

/// Managed state holding the running sidecar handle (if any).
#[derive(Default)]
pub struct LocalBackend(pub Mutex<LocalBackendState>);

#[derive(Default)]
pub struct LocalBackendState {
    child: Option<CommandChild>,
    port: Option<u16>,
}

/// Snapshot returned to the frontend.
#[derive(Serialize, Clone)]
pub struct BackendStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
}

impl LocalBackendState {
    /// Take ownership of the running child handle (e.g. to kill it on app exit).
    pub fn child_take(&mut self) -> Option<CommandChild> {
        self.port = None;
        self.child.take()
    }

    fn status(&self) -> BackendStatus {
        BackendStatus {
            running: self.child.is_some(),
            port: self.port,
            url: self.port.map(|p| format!("http://127.0.0.1:{p}")),
        }
    }
}

/// Ask the OS for a free TCP port by binding to :0 and reading it back.
fn pick_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}

/// Start the bundled backend sidecar. If already running, returns the current
/// status unchanged. `port` lets the caller pin a port; otherwise a free one is
/// chosen automatically.
#[tauri::command]
pub fn local_backend_start(
    app: AppHandle,
    state: State<'_, LocalBackend>,
    port: Option<u16>,
) -> Result<BackendStatus, String> {
    {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if guard.child.is_some() {
            return Ok(guard.status());
        }
    }

    let port = match port {
        Some(p) => p,
        None => pick_free_port()?,
    };

    let sidecar = app
        .shell()
        .sidecar("xenoclaw")
        .map_err(|e| format!("sidecar binary not found: {e}"))?
        .args(["serve"])
        .env("XENOCLAW_API_HOST", "127.0.0.1")
        .env("XENOCLAW_API_PORT", port.to_string());

    let (mut rx, child) = sidecar
        .spawn()
        .map_err(|e| format!("failed to spawn backend: {e}"))?;

    {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        guard.child = Some(child);
        guard.port = Some(port);
    }

    // Relay the sidecar's output to the frontend and clear state on exit.
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    let _ = app_for_task
                        .emit("local-backend-log", String::from_utf8_lossy(&line).to_string());
                }
                CommandEvent::Stderr(line) => {
                    let _ = app_for_task
                        .emit("local-backend-log", String::from_utf8_lossy(&line).to_string());
                }
                CommandEvent::Terminated(payload) => {
                    if let Some(b) = app_for_task.try_state::<LocalBackend>() {
                        if let Ok(mut guard) = b.0.lock() {
                            guard.child = None;
                            guard.port = None;
                        }
                    }
                    let _ = app_for_task.emit("local-backend-exit", payload.code);
                    break;
                }
                _ => {}
            }
        }
    });

    let guard = state.0.lock().map_err(|e| e.to_string())?;
    Ok(guard.status())
}

/// Stop the running sidecar (if any).
#[tauri::command]
pub fn local_backend_stop(state: State<'_, LocalBackend>) -> Result<BackendStatus, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(child) = guard.child.take() {
        let _ = child.kill();
    }
    guard.port = None;
    Ok(guard.status())
}

/// Current sidecar status.
#[tauri::command]
pub fn local_backend_status(state: State<'_, LocalBackend>) -> Result<BackendStatus, String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    Ok(guard.status())
}
