//! Plugin management endpoints.
//!
//! GET  /api/v1/plugins              — List plugins
//! POST /api/v1/plugins/:name/reload — Reload a plugin

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

/// GET /api/v1/plugins — List all loaded plugins.
async fn list_plugins(State(_state): State<AppState>) -> Json<PluginListResponse> {
    // In a full implementation, this would query the Plugin System.
    Json(PluginListResponse {
        plugins: Vec::new(),
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

    // In a full implementation, this would trigger a reload via Plugin System.
    Ok(Json(PluginReloadResponse {
        name,
        status: "reloaded",
    }))
}

/// Build plugin routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/plugins", get(list_plugins))
        .route("/api/v1/plugins/{name}/reload", post(reload_plugin))
}
