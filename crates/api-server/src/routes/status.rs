//! Agent status endpoint.
//!
//! GET /api/v1/status — Get current agent status

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

/// Agent status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub mode: String,
    pub current_task: Option<String>,
    pub uptime_seconds: u64,
}

/// GET /api/v1/status — Get the current agent status.
async fn get_status(State(_state): State<AppState>) -> Json<StatusResponse> {
    // In a full implementation, this would query the Agent Core.
    Json(StatusResponse {
        status: "idle".to_string(),
        mode: "general".to_string(),
        current_task: None,
        uptime_seconds: 0,
    })
}

/// Build status routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/status", get(get_status))
}
