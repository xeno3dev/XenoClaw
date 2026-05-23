//! Configuration management endpoints.
//!
//! GET /api/v1/config — Get current configuration
//! PUT /api/v1/config — Update configuration

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use agent_core::types::AgentMode;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

/// Configuration response (subset of platform config safe to expose).
#[derive(Debug, Serialize)]
pub struct ConfigResponse {
    pub version: String,
    pub mode: String,
    pub rate_limit_default: u32,
}

/// Request body for updating configuration.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    /// Configuration fields to update (partial update).
    pub settings: serde_json::Value,
}

/// Response after updating configuration.
#[derive(Debug, Serialize)]
pub struct UpdateConfigResponse {
    pub status: &'static str,
    pub applied_settings: Vec<String>,
}

/// GET /api/v1/config — Get current configuration.
async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let mode = if let Some(ref handle) = state.agent_core {
        match handle.0.mode().await {
            AgentMode::General => "general".to_string(),
            AgentMode::Coding { .. } => "coding".to_string(),
        }
    } else {
        "general".to_string()
    };

    Json(ConfigResponse {
        version: state.version.clone(),
        mode,
        rate_limit_default: security_layer::DEFAULT_RATE_LIMIT,
    })
}

/// PUT /api/v1/config — Update configuration.
async fn update_config(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    // Validate that settings is a non-empty object
    let obj = body.settings.as_object().ok_or_else(|| {
        ApiError::bad_request("Settings must be a JSON object.", req_id.0.clone())
    })?;

    if obj.is_empty() {
        return Err(ApiError::bad_request(
            "Settings object must not be empty.",
            req_id.0,
        ));
    }

    // Apply mode change if present
    if let Some(mode_val) = obj.get("mode") {
        if let Some(mode_str) = mode_val.as_str() {
            if let Some(ref handle) = state.agent_core {
                let new_mode = match mode_str {
                    "coding" => AgentMode::Coding {
                        workspace: state.workspace_dir.clone(),
                    },
                    _ => AgentMode::General,
                };
                // AgentBusy is the only expected error — silently ignore it so the
                // UI toggle doesn't surface an error while a message is streaming.
                let _ = handle.0.set_mode(new_mode).await;
            }
        }
    }

    let applied: Vec<String> = obj.keys().cloned().collect();

    Ok(Json(UpdateConfigResponse {
        status: "applied",
        applied_settings: applied,
    }))
}

/// Build config routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/config", get(get_config).put(update_config))
}
