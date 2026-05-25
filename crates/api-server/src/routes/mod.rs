//! API route definitions for all REST and WebSocket endpoints.
//!
//! Routes are organized by resource:
//! - `/api/v1/messages` — Conversation messages
//! - `/api/v1/status` — Agent status
//! - `/api/v1/tasks` — Task management
//! - `/api/v1/sessions` — Session management
//! - `/api/v1/health` — Health check (unauthenticated)
//! - `/api/v1/config` — Configuration management
//! - `/api/v1/memory` — Memory store operations
//! - `/api/v1/plugins` — Plugin management
//! - `/api/v1/ws/chat` — Real-time chat streaming (WebSocket)
//! - `/api/v1/ws/events` — System event stream (WebSocket)

pub mod auth;
pub mod config;
pub mod health;
pub mod memory;
pub mod messages;
pub mod messaging;
pub mod plugins;
pub mod sessions;
pub mod skills;
pub mod status;
pub mod tasks;
pub mod uploads;
pub mod ws;

use axum::middleware;
use axum::Router;

use crate::middleware::auth::auth_middleware;
use crate::state::AppState;

/// Build the complete API router with all routes and middleware.
pub fn build_routes(state: AppState) -> Router {
    // Health check and auth login are unauthenticated
    let health_routes = Router::new().merge(health::routes()).merge(auth::routes());

    // WebSocket routes handle their own authentication via query parameters
    // (browsers cannot set custom headers on WebSocket upgrade requests)
    let ws_routes = Router::new().merge(ws::routes());

    // All other routes require authentication via Bearer token
    let authenticated_routes = Router::new()
        .merge(messages::routes())
        .merge(messaging::routes())
        .merge(status::routes())
        .merge(tasks::routes())
        .merge(sessions::routes())
        .merge(config::routes())
        .merge(memory::routes())
        .merge(plugins::routes())
        .merge(uploads::routes())
        .merge(skills::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .merge(health_routes)
        .merge(ws_routes)
        .merge(authenticated_routes)
        .with_state(state)
}
