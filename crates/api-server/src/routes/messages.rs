//! Message endpoints for conversation management.
//!
//! POST /api/v1/messages — Send a message to the agent
//! GET  /api/v1/messages/:session — Get conversation history

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use common::types::SessionId;

use crate::error::ApiError;
use crate::middleware::auth::RequestId;
use crate::state::AppState;

/// Request body for sending a message.
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// The session to send the message in. If omitted, creates a new session.
    pub session_id: Option<String>,
    /// The message content.
    pub content: String,
}

/// Response after sending a message.
#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub message_id: String,
    pub session_id: String,
    pub status: &'static str,
}

/// A message in conversation history.
#[derive(Debug, Serialize)]
pub struct MessageEntry {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

/// Response for conversation history.
#[derive(Debug, Serialize)]
pub struct ConversationHistoryResponse {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
}

/// POST /api/v1/messages — Send a message to the agent.
async fn send_message(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Json(body): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), ApiError> {
    // Validate request body
    if body.content.trim().is_empty() {
        return Err(ApiError::bad_request(
            "Message content must not be empty.",
            req_id.0,
        ));
    }

    // Validate session_id format if provided
    let session_id = match &body.session_id {
        Some(id) => {
            Uuid::parse_str(id).map_err(|_| {
                ApiError::bad_request(
                    "Invalid session_id format. Must be a valid UUID.",
                    req_id.0.clone(),
                )
            })?;
            id.clone()
        }
        None => SessionId::new().to_string(),
    };

    // In a full implementation, this would route to the Agent Core.
    // For now, return a stub response indicating the message was accepted.
    let response = SendMessageResponse {
        message_id: Uuid::new_v4().to_string(),
        session_id,
        status: "accepted",
    };

    Ok((StatusCode::ACCEPTED, Json(response)))
}

/// GET /api/v1/messages/:session — Get conversation history for a session.
async fn get_history(
    State(_state): State<AppState>,
    Extension(req_id): Extension<RequestId>,
    Path(session): Path<String>,
) -> Result<Json<ConversationHistoryResponse>, ApiError> {
    // Validate session ID format
    Uuid::parse_str(&session).map_err(|_| {
        ApiError::bad_request(
            "Invalid session ID format. Must be a valid UUID.",
            req_id.0.clone(),
        )
    })?;

    // In a full implementation, this would query the Memory Store.
    Ok(Json(ConversationHistoryResponse {
        session_id: session,
        messages: Vec::new(),
    }))
}

/// Build message routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/messages", post(send_message))
        .route("/api/v1/messages/{session}", get(get_history))
}
