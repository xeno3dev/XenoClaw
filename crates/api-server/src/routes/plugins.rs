//! Plugin management endpoints.
//!
//! GET  /api/v1/plugins              — List plugins with enabled state
//! POST /api/v1/plugins/:name/toggle — Toggle a plugin on/off (loads or unloads)
//! POST /api/v1/plugins/:name/reload — Reload a specific plugin
//! POST /api/v1/plugins/reload       — Reload all plugins

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::{save_plugin_state, AppState};

/// Plugin info for listing.
#[derive(Debug, Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub status: String,
    pub description: String,
    pub enabled: bool,
}

/// Plugin list response.
#[derive(Debug, Serialize)]
pub struct PluginListResponse {
    pub plugins: Vec<PluginInfo>,
}

/// Plugin reload response.
#[derive(Debug, Serialize)]
pub struct PluginReloadResponse {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Plugin toggle response.
#[derive(Debug, Serialize)]
pub struct PluginToggleResponse {
    pub name: String,
    pub enabled: bool,
}

/// GET /api/v1/plugins — List all loaded plugins with their enabled state.
async fn list_plugins(State(state): State<AppState>) -> Json<PluginListResponse> {
    let toggles = state.plugin_states.read().await.clone();

    let mut plugins: Vec<PluginInfo> = if let Some(ref handle) = state.plugin_manager {
        let mgr = handle.0.read().await;
        let live = mgr.list_plugins().await;
        live.into_iter()
            .map(|p| {
                let name = p.manifest.name.clone();
                let enabled = toggles.get(&name).copied().unwrap_or(true);
                let status = match p.status {
                    plugin_system::PluginStatus::Active => "loaded",
                    plugin_system::PluginStatus::Failed { .. } => "failed",
                    plugin_system::PluginStatus::Reloading => "reloading",
                };
                PluginInfo {
                    name,
                    version: p.manifest.version.to_string(),
                    status: status.to_string(),
                    description: p.manifest.description.clone(),
                    enabled,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Surface any plugins that the user has toggled off (and thus aren't in the
    // live manager's list) so the UI can still show + re-enable them.
    let live_names: std::collections::HashSet<_> = plugins.iter().map(|p| p.name.clone()).collect();
    for (name, enabled) in &toggles {
        if !live_names.contains(name) {
            plugins.push(PluginInfo {
                name: name.clone(),
                version: "unknown".to_string(),
                status: if *enabled { "unloaded" } else { "disabled" }.to_string(),
                description: String::new(),
                enabled: *enabled,
            });
        }
    }

    Json(PluginListResponse { plugins })
}

/// POST /api/v1/plugins/:name/toggle — Flip a plugin between enabled and disabled.
/// When disabled, the plugin is unloaded from the live PluginManager. When
/// re-enabled, it's reloaded from disk. State is persisted to SQLite so the
/// choice survives a restart.
async fn toggle_plugin(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(name): Path<String>,
) -> Result<Json<PluginToggleResponse>, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Plugin name must not be empty.",
            req_id.0,
        ));
    }

    let new_enabled = {
        let mut states = state.plugin_states.write().await;
        let current = states.get(&name).copied().unwrap_or(true);
        let next = !current;
        states.insert(name.clone(), next);
        next
    };

    // Persist (best-effort; warns on failure but doesn't fail the request)
    save_plugin_state(state.db_pool.as_ref(), &name, new_enabled).await;

    // Apply to the live manager
    if let Some(ref handle) = state.plugin_manager {
        let mgr = handle.0.read().await;
        if new_enabled {
            if let Err(e) = mgr.reload_plugin(&name).await {
                tracing::warn!(plugin = %name, error = %e, "Failed to load plugin on enable");
            }
        } else if let Err(e) = mgr.unload_plugin(&name).await {
            tracing::warn!(plugin = %name, error = %e, "Failed to unload plugin on disable");
        }
    }

    Ok(Json(PluginToggleResponse {
        name,
        enabled: new_enabled,
    }))
}

/// POST /api/v1/plugins/reload — Reload all plugins from disk.
async fn reload_all_plugins(State(state): State<AppState>) -> Json<PluginReloadResponse> {
    if let Some(ref handle) = state.plugin_manager {
        let mgr = handle.0.read().await;
        let results = mgr.load_all().await;
        let failures = results.iter().filter(|r| !r.success).count();
        let status = if failures == 0 { "reloaded" } else { "partial" };
        return Json(PluginReloadResponse {
            name: "*".to_string(),
            status: status.to_string(),
            error: if failures == 0 {
                None
            } else {
                Some(format!("{failures} plugin(s) failed to load"))
            },
        });
    }
    Json(PluginReloadResponse {
        name: "*".to_string(),
        status: "unavailable".to_string(),
        error: Some("plugin_manager not attached".to_string()),
    })
}

/// POST /api/v1/plugins/:name/reload — Reload a specific plugin.
async fn reload_plugin(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(name): Path<String>,
) -> Result<Json<PluginReloadResponse>, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Plugin name must not be empty.",
            req_id.0,
        ));
    }
    if let Some(ref handle) = state.plugin_manager {
        let mgr = handle.0.read().await;
        return match mgr.reload_plugin(&name).await {
            Ok(_) => Ok(Json(PluginReloadResponse {
                name,
                status: "reloaded".to_string(),
                error: None,
            })),
            Err(e) => Ok(Json(PluginReloadResponse {
                name,
                status: "failed".to_string(),
                error: Some(e.to_string()),
            })),
        };
    }
    Ok(Json(PluginReloadResponse {
        name,
        status: "unavailable".to_string(),
        error: Some("plugin_manager not attached".to_string()),
    }))
}

/// Build plugin routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/plugins", get(list_plugins))
        // /reload (no name) must come before /:name/* to avoid shadowing
        .route("/api/v1/plugins/reload", post(reload_all_plugins))
        .route("/api/v1/plugins/{name}/toggle", post(toggle_plugin))
        .route("/api/v1/plugins/{name}/reload", post(reload_plugin))
}
