//! Session management endpoints.
//!
//! POST   /api/v1/sessions      — Create a new session
//! GET    /api/v1/sessions      — List active sessions
//! GET    /api/v1/sessions/:id  — Get session details
//! DELETE /api/v1/sessions/:id  — Close a session
//! POST   /api/v1/sessions/:id/switch — Mark a session active (touches last_activity)

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use common::config::SessionSource;
use common::models::AgentMode;
use common::types::SessionId;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::{AppState, SessionInfo};

/// Request body for creating a session.
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    /// Source of the session (e.g. "web", "api", "tui", "claude_code").
    pub source: String,
    /// Agent mode for the session (e.g. "general", "coding").
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "general".to_string()
}

/// Response after creating a session.
#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub source: String,
    pub mode: String,
    pub created_at: DateTime<Utc>,
}

/// Session summary for listing and detail responses.
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub source: String,
    pub mode: String,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    /// True when this is the session the user most recently sent a message in.
    pub active: bool,
}

/// Session list response.
#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionSummary>,
}

/// Delete session response.
#[derive(Debug, Serialize)]
pub struct DeleteSessionResponse {
    pub deleted: bool,
}

/// Parse a source string into a SessionSource enum variant.
fn parse_source(source: &str) -> Option<SessionSource> {
    match source {
        "web" => Some(SessionSource::Web),
        "api" => Some(SessionSource::Api),
        "tui" => Some(SessionSource::Tui),
        "claude_code" => Some(SessionSource::ClaudeCode),
        "copilot_cli" => Some(SessionSource::CopilotCli),
        _ => None,
    }
}

/// Convert a SessionSource to its string representation.
fn source_to_string(source: &SessionSource) -> String {
    match source {
        SessionSource::Web => "web".to_string(),
        SessionSource::Api => "api".to_string(),
        SessionSource::Tui => "tui".to_string(),
        SessionSource::ClaudeCode => "claude_code".to_string(),
        SessionSource::CopilotCli => "copilot_cli".to_string(),
        SessionSource::Telegram { chat_id } => format!("telegram:{}", chat_id),
        SessionSource::Discord { channel_id } => format!("discord:{}", channel_id),
        SessionSource::WhatsApp { phone } => format!("whatsapp:{}", phone),
    }
}

/// Convert an AgentMode to its string representation.
fn mode_to_string(mode: &AgentMode) -> String {
    match mode {
        AgentMode::General => "general".to_string(),
        AgentMode::Coding {
            plan_only: true, ..
        } => "plan".to_string(),
        AgentMode::Coding {
            plan_only: false, ..
        } => "code".to_string(),
    }
}

/// Parse a mode string into an AgentMode enum variant.
fn parse_mode(mode: &str) -> Option<AgentMode> {
    match mode {
        "general" => Some(AgentMode::General),
        // "plan" and "code" are both Coding; "coding" is a back-compat alias.
        "plan" => Some(AgentMode::Coding {
            workspace: std::path::PathBuf::from("."),
            plan_only: true,
        }),
        "code" | "coding" => Some(AgentMode::Coding {
            workspace: std::path::PathBuf::from("."),
            plan_only: false,
        }),
        _ => None,
    }
}

/// POST /api/v1/sessions — Create a new session.
async fn create_session(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let source = parse_source(&body.source).ok_or_else(|| {
        ApiError::bad_request(
            format!(
                "Invalid source '{}'. Must be one of: web, api, tui, claude_code, copilot_cli.",
                body.source
            ),
            req_id.0.clone(),
        )
    })?;

    let mode = parse_mode(&body.mode).ok_or_else(|| {
        ApiError::bad_request(
            format!(
                "Invalid mode '{}'. Must be one of: general, plan, code.",
                body.mode
            ),
            req_id.0.clone(),
        )
    })?;

    let now = Utc::now();
    let session_id = SessionId::new();

    let info = SessionInfo {
        session_id,
        source: source.clone(),
        mode: mode.clone(),
        created_at: now,
        last_activity: now,
    };

    // Store in the in-memory session map
    {
        let mut sessions = state.sessions.write().await;
        sessions.insert(session_id, info);
    }

    let response = CreateSessionResponse {
        session_id: session_id.0.to_string(),
        source: source_to_string(&source),
        mode: mode_to_string(&mode),
        created_at: now,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /api/v1/sessions — List all active sessions, most-recently-active first.
async fn list_sessions(State(state): State<AppState>) -> Json<SessionListResponse> {
    let active = *state.active_session.read().await;
    let sessions = state.sessions.read().await;

    let mut summaries: Vec<SessionSummary> = sessions
        .values()
        .map(|info| SessionSummary {
            session_id: info.session_id.0.to_string(),
            source: source_to_string(&info.source),
            mode: mode_to_string(&info.mode),
            created_at: info.created_at,
            last_activity: info.last_activity,
            active: active == Some(info.session_id),
        })
        .collect();

    // Most recently active first so the active session sits at the top.
    summaries.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));

    Json(SessionListResponse {
        sessions: summaries,
    })
}

/// GET /api/v1/sessions/:id — Get session details.
async fn get_session(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<SessionSummary>, ApiError> {
    let uuid = Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid session ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    let session_id = SessionId(uuid);
    let active = *state.active_session.read().await;
    let sessions = state.sessions.read().await;

    match sessions.get(&session_id) {
        Some(info) => Ok(Json(SessionSummary {
            session_id: info.session_id.0.to_string(),
            source: source_to_string(&info.source),
            mode: mode_to_string(&info.mode),
            created_at: info.created_at,
            last_activity: info.last_activity,
            active: active == Some(session_id),
        })),
        None => Err(ApiError::not_found(
            format!("Session '{}' not found.", id),
            req_id.0,
        )),
    }
}

/// POST /api/v1/sessions/:id/switch — Make a session the active one.
///
/// Sets the active-session slot to this session and touches `last_activity` so
/// it sorts to the top. This is the manual counterpart to the automatic
/// activation that happens when the user sends a chat message. Returns the
/// updated summary, or 404 if the session is gone.
async fn switch_session(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<SessionSummary>, ApiError> {
    let uuid = Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid session ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    let session_id = SessionId(uuid);
    let mut sessions = state.sessions.write().await;

    match sessions.get_mut(&session_id) {
        Some(info) => {
            info.last_activity = Utc::now();
            let summary = SessionSummary {
                session_id: info.session_id.0.to_string(),
                source: source_to_string(&info.source),
                mode: mode_to_string(&info.mode),
                created_at: info.created_at,
                last_activity: info.last_activity,
                active: true,
            };
            // Drop the sessions lock before taking the active-session lock to
            // keep a consistent lock order across handlers.
            drop(sessions);
            *state.active_session.write().await = Some(session_id);
            Ok(Json(summary))
        }
        None => Err(ApiError::not_found(
            format!("Session '{}' not found.", id),
            req_id.0,
        )),
    }
}

/// DELETE /api/v1/sessions/:id — Close a session.
async fn close_session(
    State(state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(id): Path<String>,
) -> Result<Json<DeleteSessionResponse>, ApiError> {
    let uuid = Uuid::parse_str(&id).map_err(|_| {
        ApiError::bad_request(
            "Invalid session ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    let session_id = SessionId(uuid);
    let mut sessions = state.sessions.write().await;

    match sessions.remove(&session_id) {
        Some(_) => {
            drop(sessions);
            // If we just deleted the active session, clear the slot so the UI
            // doesn't point at a session that no longer exists.
            let mut active = state.active_session.write().await;
            if *active == Some(session_id) {
                *active = None;
            }
            Ok(Json(DeleteSessionResponse { deleted: true }))
        }
        None => Err(ApiError::not_found(
            format!("Session '{}' not found.", id),
            req_id.0,
        )),
    }
}

/// Build session routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/sessions", post(create_session).get(list_sessions))
        .route(
            "/api/v1/sessions/{id}",
            get(get_session).delete(close_session),
        )
        .route("/api/v1/sessions/{id}/switch", post(switch_session))
}
