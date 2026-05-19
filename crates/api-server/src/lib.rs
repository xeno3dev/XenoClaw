//! API Server — HTTP/WebSocket server exposing the platform's capabilities.
//!
//! Responsibilities:
//! - RESTful HTTP API for task submission, status queries, and configuration
//! - WebSocket endpoint for real-time streaming of agent responses
//! - Authentication and rate limiting enforcement
//! - Structured JSON error responses

pub mod error;
pub mod middleware;
pub mod routes;
pub mod state;

pub use error::ApiError;
pub use state::AppState;

use axum::Router;

/// Platform version string for health check responses.
pub const PLATFORM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build the API router with all routes and middleware.
pub fn build_router(state: AppState) -> Router {
    routes::build_routes(state)
}
