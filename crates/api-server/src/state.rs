//! Application state shared across all API handlers.
//!
//! Contains references to the security layer components (rate limiter,
//! API key authenticator), WebSocket state, session tracking, and platform
//! version information.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::sqlite::SqlitePool;
use tokio::sync::RwLock;

use agent_core::AgentCore;
use security_layer::auth::ApiKeyAuthenticator;
use security_layer::rate_limit::{RateLimitConfig, RateLimiter};

use common::config::SessionSource;
use common::models::{AgentMode, ApiKey};
use common::types::{ApiKeyId, SessionId};

use crate::routes::ws::WsState;

/// Newtype so `Arc<AgentCore>` can be stored in a `#[derive(Debug, Clone)]` struct.
/// `AgentCore` itself does not implement `Debug`, so we provide a stub impl here.
#[derive(Clone)]
pub struct AgentCoreHandle(pub Arc<AgentCore>);

impl std::fmt::Debug for AgentCoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AgentCoreHandle")
    }
}

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

/// Tokens issued by the password-login endpoint, valid until server restart.
pub type LoginTokenStore = Arc<RwLock<HashSet<String>>>;

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
    /// Tokens issued by the /auth/login endpoint (password-based login).
    pub login_tokens: LoginTokenStore,
    /// A stable ApiKeyId used to represent admin password-login sessions in the
    /// auth middleware (needed for the AuthenticatedKey extension).
    pub admin_session_key_id: ApiKeyId,
    /// Optional SQLite pool for session persistence.
    pub db_pool: Option<SqlitePool>,
    /// Which third-party messaging bridges have credentials configured. Used
    /// by the Settings UI to render connection status without ever exposing
    /// the actual bot tokens over the wire.
    pub messaging_status: MessagingStatus,
    /// Live reference to the agent runtime for status queries and mode changes.
    pub agent_core: Option<AgentCoreHandle>,
    /// When the server started — used to compute uptime_seconds in /api/v1/status.
    pub started_at: Instant,
    /// Default workspace directory used when switching to Coding mode.
    pub workspace_dir: PathBuf,
    /// Per-plugin enabled/disabled state (in-memory; reflects UI toggles).
    pub plugin_states: Arc<RwLock<HashMap<String, bool>>>,
}

/// Public-safe view of which messaging providers are configured.
/// Never carries the actual tokens or phone number.
#[derive(Debug, Clone, Copy, Default)]
pub struct MessagingStatus {
    pub telegram_configured: bool,
    pub discord_configured: bool,
    pub whatsapp_configured: bool,
}

fn default_workspace_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".xenoclaw")
        .join("workspace")
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
            login_tokens: Arc::new(RwLock::new(HashSet::new())),
            admin_session_key_id: ApiKeyId::new(),
            db_pool: None,
            messaging_status: MessagingStatus::default(),
            agent_core: None,
            started_at: Instant::now(),
            workspace_dir: default_workspace_dir(),
            plugin_states: Arc::new(RwLock::new(HashMap::new())),
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
            login_tokens: Arc::new(RwLock::new(HashSet::new())),
            admin_session_key_id: ApiKeyId::new(),
            db_pool: None,
            messaging_status: MessagingStatus::default(),
            agent_core: None,
            started_at: Instant::now(),
            workspace_dir: default_workspace_dir(),
            plugin_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Replace the messaging-provider status (used by the runtime at startup
    /// after reading the on-disk config). Builder-style for ergonomic chaining.
    pub fn with_messaging_status(mut self, status: MessagingStatus) -> Self {
        self.messaging_status = status;
        self
    }

    /// Attach the live AgentCore so status/config endpoints reflect real state.
    pub fn with_agent_core(mut self, core: Arc<AgentCore>) -> Self {
        self.agent_core = Some(AgentCoreHandle(core));
        self
    }

    /// Override the default workspace directory used when switching to Coding mode.
    pub fn with_workspace_dir(mut self, dir: PathBuf) -> Self {
        self.workspace_dir = dir;
        self
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
