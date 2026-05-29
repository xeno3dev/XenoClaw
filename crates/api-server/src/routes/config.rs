//! Configuration management endpoints.
//!
//! GET /api/v1/config — Get current configuration
//! PUT /api/v1/config — Update configuration (mode, system_prompt, log_level)

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
    pub system_prompt: Option<String>,
    pub log_level: String,
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
    /// Per-key warnings — e.g. unknown keys or failed log-level parses.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// GET /api/v1/config — Get current configuration.
async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let mode = if let Some(ref handle) = state.agent_core {
        match handle.0.mode().await {
            AgentMode::General => "general".to_string(),
            AgentMode::Coding {
                plan_only: true, ..
            } => "plan".to_string(),
            AgentMode::Coding {
                plan_only: false, ..
            } => "code".to_string(),
        }
    } else {
        "general".to_string()
    };

    let system_prompt = match state.agent_core {
        Some(ref handle) => handle.0.system_prompt().await,
        None => None,
    };

    let log_level = state.current_log_level.read().await.clone();

    Json(ConfigResponse {
        version: state.version.clone(),
        mode,
        rate_limit_default: state.rate_limiter.default_limit(),
        system_prompt,
        log_level,
    })
}

/// PUT /api/v1/config — Update configuration.
async fn update_config(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    let obj = body.settings.as_object().ok_or_else(|| {
        ApiError::bad_request("Settings must be a JSON object.", req_id.0.clone())
    })?;

    if obj.is_empty() {
        return Err(ApiError::bad_request(
            "Settings object must not be empty.",
            req_id.0,
        ));
    }

    let mut applied: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Apply mode change
    if let Some(mode_val) = obj.get("mode") {
        if let Some(mode_str) = mode_val.as_str() {
            if let Some(ref handle) = state.agent_core {
                let new_mode = match mode_str {
                    // "plan" and "code" are both Coding mode; plan_only filters
                    // out destructive tools. "coding" kept as a back-compat alias.
                    "plan" => AgentMode::Coding {
                        workspace: state.workspace_dir.clone(),
                        plan_only: true,
                    },
                    "code" | "coding" => AgentMode::Coding {
                        workspace: state.workspace_dir.clone(),
                        plan_only: false,
                    },
                    _ => AgentMode::General,
                };
                // AgentBusy is the only expected error — silently ignore so the
                // UI toggle doesn't surface an error mid-stream.
                let _ = handle.0.set_mode(new_mode).await;
                applied.push("mode".to_string());
            } else {
                warnings.push("mode: agent_core not attached".to_string());
            }
        }
    }

    // Apply system_prompt change — accepts string (sets) or null (clears).
    if let Some(prompt_val) = obj.get("system_prompt") {
        if let Some(ref handle) = state.agent_core {
            let new_prompt = if prompt_val.is_null() {
                None
            } else if let Some(s) = prompt_val.as_str() {
                Some(s.to_string())
            } else {
                warnings.push("system_prompt: must be string or null".to_string());
                None
            };
            handle.0.set_system_prompt(new_prompt).await;
            applied.push("system_prompt".to_string());
        } else {
            warnings.push("system_prompt: agent_core not attached".to_string());
        }
    }

    // Apply rate_limit_default change — atomic swap inside RateLimiter, no rebuild.
    if let Some(rate_val) = obj.get("rate_limit_default") {
        if let Some(n) = rate_val.as_u64() {
            if n == 0 || n > u32::MAX as u64 {
                warnings.push(format!(
                    "rate_limit_default: out of range (1..={})",
                    u32::MAX
                ));
            } else {
                state.rate_limiter.set_default_limit(n as u32);
                applied.push("rate_limit_default".to_string());
            }
        } else {
            warnings.push("rate_limit_default: must be a positive integer".to_string());
        }
    }

    // Apply log_level change — passes the string straight to the tracing reload handle.
    if let Some(level_val) = obj.get("log_level") {
        if let Some(level_str) = level_val.as_str() {
            if let Some(ref setter) = state.log_level_setter {
                match setter.call(level_str) {
                    Ok(()) => {
                        *state.current_log_level.write().await = level_str.to_string();
                        applied.push("log_level".to_string());
                    }
                    Err(e) => {
                        warnings.push(format!("log_level: {e}"));
                    }
                }
            } else {
                warnings.push("log_level: tracing reload handle not installed".to_string());
            }
        } else {
            warnings.push("log_level: must be a string".to_string());
        }
    }

    // Anything we didn't recognise gets a warning so the client knows.
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "mode" | "system_prompt" | "log_level" | "rate_limit_default"
        ) {
            warnings.push(format!("unknown setting: {key}"));
        }
    }

    Ok(Json(UpdateConfigResponse {
        status: "applied",
        applied_settings: applied,
        warnings,
    }))
}

/// Build config routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/config", get(get_config).put(update_config))
}
