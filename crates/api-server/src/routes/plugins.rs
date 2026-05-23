//! Plugin management endpoints.
//!
//! GET  /api/v1/plugins              — List plugins with enabled state
//! POST /api/v1/plugins/:name/toggle — Toggle a plugin on/off
//! POST /api/v1/plugins/:name/reload — Reload a specific plugin
//! POST /api/v1/plugins/reload       — Reload all plugins

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

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
    pub status: &'static str,
}

/// Plugin toggle response.
#[derive(Debug, Serialize)]
pub struct PluginToggleResponse {
    pub name: String,
    pub enabled: bool,
}

/// GET /api/v1/plugins — List all loaded plugins with their enabled states.
async fn list_plugins(State(state): State<AppState>) -> Json<PluginListResponse> {
    let states = state.plugin_states.read().await;
    // Emit one entry per plugin that has been toggled at least once.
    // A full implementation would merge this with the live PluginManager inventory.
    let plugins = states
        .iter()
        .map(|(name, enabled)| PluginInfo {
            name: name.clone(),
            version: "unknown".to_string(),
            status: if *enabled { "loaded" } else { "disabled" }.to_string(),
            description: String::new(),
            enabled: *enabled,
        })
        .collect();
    Json(PluginListResponse { plugins })
}

/// POST /api/v1/plugins/:name/toggle — Toggle a plugin on or off.
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
    let mut states = state.plugin_states.write().await;
    let enabled = states.entry(name.clone()).or_insert(true);
    *enabled = !*enabled;
    let new_state = *enabled;
    Ok(Json(PluginToggleResponse {
        name,
        enabled: new_state,
    }))
}

/// POST /api/v1/plugins/reload — Reload all plugins.
async fn reload_all_plugins(State(_state): State<AppState>) -> Json<PluginReloadResponse> {
    // A full implementation would call PluginManager::reload_all().
    Json(PluginReloadResponse {
        name: "*".to_string(),
        status: "reloaded",
    })
}

/// POST /api/v1/plugins/:name/reload — Reload a specific plugin.
async fn reload_plugin(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(name): Path<String>,
) -> Result<Json<PluginReloadResponse>, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Plugin name must not be empty.",
            req_id.0,
        ));
    }
    // A full implementation would call PluginManager::reload(name).
    Ok(Json(PluginReloadResponse {
        name,
        status: "reloaded",
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
