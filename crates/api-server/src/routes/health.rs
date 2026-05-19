//! Health check endpoint.
//!
//! GET /api/v1/health — responds within 2 seconds with platform version
//! and healthy/unhealthy status. This endpoint does NOT require authentication.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

/// Health check response body.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: String,
}

/// GET /api/v1/health
///
/// Returns the platform version and health status.
/// Responds within 2 seconds. No authentication required.
async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    // In a full implementation, this would check backing services
    // (database connectivity, LLM provider reachability, etc.)
    // For now, if the server is responding, it's healthy.
    Json(HealthResponse {
        status: "healthy",
        version: state.version.clone(),
    })
}

/// Build health check routes.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/health", get(health_check))
}
