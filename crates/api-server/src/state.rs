//! Application state shared across all API handlers.
//!
//! Contains references to the security layer components (rate limiter,
//! API key authenticator), WebSocket state, session tracking, and platform
//! version information.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::sqlite::SqlitePool;
use tokio::sync::RwLock;

use security_layer::auth::ApiKeyAuthenticator;
use security_layer::rate_limit::{RateLimitConfig, RateLimiter};

use common::config::SessionSource;
use common::models::{AgentMode, ApiKey};
use common::types::SessionId;

use crate::routes::ws::WsState;

/// Information about an active session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    /// Unique session identifier.
    pub session_id: SessionId,
    /// Where the session originated from.
    pub source: SessionSource,
    /// The agent mode for this session.
    pub mode: AgentMode,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session last had activity.
    pub last_activity: DateTime<Utc>,
}

/// In-memory session store, periodically persisted to SQLite.
pub type SessionStore = Arc<RwLock<HashMap<SessionId, SessionInfo>>>;

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
    /// Admin username for password login.
    pub admin_username: String,
    /// Bcrypt hash of admin password (empty = password login disabled).
    pub admin_password_hash: String,
    /// In-memory session store for active sessions.
    pub sessions: SessionStore,
    /// Optional SQLite pool for session persistence.
    pub db_pool: Option<SqlitePool>,
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
            admin_username: "admin".to_string(),
            admin_password_hash: String::new(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            db_pool: None,
        }
    }

    /// Create an AppState with admin credentials configured.
    pub fn with_admin_credentials(
        api_keys: Vec<ApiKey>,
        rate_limit_config: RateLimitConfig,
        admin_username: String,
        admin_password_hash: String,
    ) -> Self {
        Self {
            authenticator: Arc::new(ApiKeyAuthenticator::new()),
            rate_limiter: Arc::new(RateLimiter::new(rate_limit_config)),
            api_keys: Arc::new(api_keys),
            version: crate::PLATFORM_VERSION.to_string(),
            ws_state: WsState::default(),
            admin_username,
            admin_password_hash,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            db_pool: None,
        }
    }

    /// Create a default AppState for testing or development.
    pub fn default_with_keys(api_keys: Vec<ApiKey>) -> Self {
        Self::new(api_keys, RateLimitConfig::default())
    }

    /// Set the SQLite pool for session persistence.
    pub fn with_db_pool(mut self, pool: SqlitePool) -> Self {
        self.db_pool = Some(pool);
        self
    }
}
