//! Agent status endpoint.
//!
//! GET /api/v1/status — Get current agent status

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use agent_core::types::{AgentMode, AgentStatus};

use crate::state::AppState;

/// Agent status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub mode: String,
    pub current_task: Option<String>,
    pub uptime_seconds: u64,
    pub cpu_percent: f32,
    pub memory_percent: f32,
}

/// GET /api/v1/status — Get the current agent status.
async fn get_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let uptime_seconds = state.started_at.elapsed().as_secs();

    let (status_str, current_task, mode_str) = if let Some(ref handle) = state.agent_core {
        let agent_status = handle.0.status().await;
        let agent_mode = handle.0.mode().await;

        let (s, task) = match agent_status {
            AgentStatus::Idle => ("idle".to_string(), None),
            AgentStatus::Working { task, .. } => ("working".to_string(), Some(task)),
            AgentStatus::Error { message } => ("error".to_string(), Some(message)),
            AgentStatus::ShuttingDown => ("shutting_down".to_string(), None),
            AgentStatus::Starting => ("starting".to_string(), None),
        };

        let m = match agent_mode {
            AgentMode::General => "general".to_string(),
            AgentMode::Coding { plan_only: true, .. } => "plan".to_string(),
            AgentMode::Coding { plan_only: false, .. } => "code".to_string(),
        };

        (s, task, m)
    } else {
        ("idle".to_string(), None, "general".to_string())
    };

    let metrics = *state.metrics.read().await;

    Json(StatusResponse {
        status: status_str,
        mode: mode_str,
        current_task,
        uptime_seconds,
        cpu_percent: metrics.cpu_percent,
        memory_percent: metrics.memory_percent,
    })
}

/// Build status routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/status", get(get_status))
}
