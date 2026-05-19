//! Application state shared across all API handlers.
//!
//! Contains references to the security layer components (rate limiter,
//! API key authenticator), WebSocket state, and platform version information.

use std::sync::Arc;

use security_layer::auth::ApiKeyAuthenticator;
use security_layer::rate_limit::{RateLimitConfig, RateLimiter};

use common::models::ApiKey;

use crate::routes::ws::WsState;

/// Shared application state available to all route handlers.
#[derive(Debug, Clone)]
pub struct AppState {
    /// API key authenticator for validating Bearer tokens.
    pub authenticator: Arc<ApiKeyAuthenticator>,
    /// Per-key rate limiter.
    pub rate_limiter: Arc<RateLimiter>,
    /// Registered API keys (in production, loaded from DB).
    pub api_keys: Arc<Vec<ApiKey>>,
    /// Platform version string.
    pub version: String,
    /// WebSocket connection state for chat and event streaming.
    pub ws_state: WsState,
}

impl AppState {
    /// Create a new AppState with the given configuration.
    pub fn new(api_keys: Vec<ApiKey>, rate_limit_config: RateLimitConfig) -> Self {
        Self {
            authenticator: Arc::new(ApiKeyAuthenticator::new()),
            rate_limiter: Arc::new(RateLimiter::new(rate_limit_config)),
            api_keys: Arc::new(api_keys),
            version: crate::PLATFORM_VERSION.to_string(),
            ws_state: WsState::default(),
        }
    }

    /// Create a default AppState for testing or development.
    pub fn default_with_keys(api_keys: Vec<ApiKey>) -> Self {
        Self::new(api_keys, RateLimitConfig::default())
    }
}
