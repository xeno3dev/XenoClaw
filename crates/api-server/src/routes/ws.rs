//! WebSocket endpoints for real-time communication.
//!
//! GET /api/v1/ws/chat — Real-time chat streaming via WebSocket
//! GET /api/v1/ws/events — System event stream via WebSocket
//!
//! ## Authentication
//!
//! WebSocket connections authenticate via a `token` query parameter since
//! browsers cannot set custom headers on WebSocket upgrade requests:
//!
//! ```text
//! ws://host/api/v1/ws/chat?token=<api-key>
//! ws://host/api/v1/ws/events?token=<api-key>
//! ```
//!
//! ## Chat Protocol
//!
//! Client sends JSON messages:
//! ```json
//! { "type": "message", "session_id": "uuid", "content": "Hello" }
//! ```
//!
//! Server streams back token-by-token:
//! ```json
//! { "type": "token", "session_id": "uuid", "content": "Hi" }
//! { "type": "token", "session_id": "uuid", "content": " there" }
//! { "type": "done", "session_id": "uuid", "message_id": "uuid" }
//! ```
//!
//! ## Events Protocol
//!
//! Server pushes system events:
//! ```json
//! { "type": "event", "event_type": "task_completed", "payload": {...}, "timestamp": "..." }
//! ```
//!
//! Client can send a `last_event_id` on connect to receive missed events:
//! ```json
//! { "type": "replay_from", "last_event_id": "uuid" }
//! ```

use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::state::AppState;

// --- Types ---

/// Query parameters for WebSocket authentication.
#[derive(Debug, Deserialize)]
pub struct WsAuthQuery {
    /// API key for authentication.
    pub token: Option<String>,
}

/// Inbound message from a chat WebSocket client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatClientMessage {
    /// Send a message to the agent. `attachments` are workspace-relative paths
    /// (e.g. `uploads/<session>/cat.png`) the client uploaded beforehand via
    /// `POST /api/v1/uploads/<session>`.
    Message {
        session_id: String,
        content: String,
        #[serde(default)]
        attachments: Vec<String>,
    },
    /// Ping to keep connection alive.
    Ping,
}

/// Outbound message to a chat WebSocket client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatServerMessage {
    /// A streamed token from the LLM response.
    Token { session_id: String, content: String },
    /// Indicates the response is complete.
    Done {
        session_id: String,
        message_id: String,
    },
    /// An error occurred processing the message.
    Error {
        session_id: Option<String>,
        error_code: String,
        message: String,
    },
    /// Authentication result.
    Authenticated { user_id: String },
    /// Pong response to client ping.
    Pong,
}

/// Inbound message from an events WebSocket client.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventsClientMessage {
    /// Request replay of events since a given event ID.
    ReplayFrom { last_event_id: String },
    /// Ping to keep connection alive.
    Ping,
}

/// Outbound system event message.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventsServerMessage {
    /// A system event.
    Event {
        id: String,
        event_type: String,
        payload: serde_json::Value,
        timestamp: String,
    },
    /// Authentication result.
    Authenticated { user_id: String },
    /// An error occurred.
    Error { error_code: String, message: String },
    /// Pong response.
    Pong,
}

/// A buffered event for replay support.
#[derive(Debug, Clone)]
struct BufferedEvent {
    id: Uuid,
    event_type: String,
    payload: serde_json::Value,
    timestamp: String,
}

/// Shared state for WebSocket connection management.
#[derive(Debug, Clone)]
pub struct WsState {
    /// Broadcast channel for system events.
    event_tx: broadcast::Sender<EventsServerMessage>,
    /// Recent events buffer for reconnection replay (bounded ring buffer).
    event_buffer: Arc<RwLock<VecDeque<BufferedEvent>>>,
    /// Maximum number of events to buffer for replay.
    max_buffer_size: usize,
    /// Active connection count.
    active_connections: Arc<RwLock<ConnectionTracker>>,
}

/// Tracks active WebSocket connections.
#[derive(Debug, Default)]
pub struct ConnectionTracker {
    pub chat_connections: usize,
    pub event_connections: usize,
}

impl WsState {
    /// Create a new WebSocket state with the given event buffer capacity.
    pub fn new(buffer_capacity: usize) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            event_tx,
            event_buffer: Arc::new(RwLock::new(VecDeque::with_capacity(buffer_capacity))),
            max_buffer_size: buffer_capacity,
            active_connections: Arc::new(RwLock::new(ConnectionTracker::default())),
        }
    }

    /// Publish a system event to all connected event stream clients.
    pub async fn publish_event(&self, event_type: String, payload: serde_json::Value) {
        let id = Uuid::new_v4();
        let timestamp = Utc::now().to_rfc3339();

        let buffered = BufferedEvent {
            id,
            event_type: event_type.clone(),
            payload: payload.clone(),
            timestamp: timestamp.clone(),
        };

        // Buffer the event for replay
        {
            let mut buffer = self.event_buffer.write().await;
            if buffer.len() >= self.max_buffer_size {
                buffer.pop_front();
            }
            buffer.push_back(buffered);
        }

        // Broadcast to connected clients
        let msg = EventsServerMessage::Event {
            id: id.to_string(),
            event_type,
            payload,
            timestamp,
        };

        // Ignore send errors (no receivers connected)
        let _ = self.event_tx.send(msg);
    }

    /// Get events after a given event ID for replay.
    async fn get_events_after(&self, last_event_id: &str) -> Vec<EventsServerMessage> {
        let target_id = match Uuid::parse_str(last_event_id) {
            Ok(id) => id,
            Err(_) => return Vec::new(),
        };

        let buffer = self.event_buffer.read().await;

        // Find the position of the last seen event
        let start_pos = buffer
            .iter()
            .position(|e| e.id == target_id)
            .map(|pos| pos + 1)
            .unwrap_or(0);

        buffer
            .iter()
            .skip(start_pos)
            .map(|e| EventsServerMessage::Event {
                id: e.id.to_string(),
                event_type: e.event_type.clone(),
                payload: e.payload.clone(),
                timestamp: e.timestamp.clone(),
            })
            .collect()
    }
}

impl Default for WsState {
    fn default() -> Self {
        Self::new(1000)
    }
}

// --- Handlers ---

/// GET /api/v1/ws/chat — WebSocket upgrade for real-time chat streaming.
async fn ws_chat_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(auth): Query<WsAuthQuery>,
) -> Response {
    ws.on_upgrade(move |socket| handle_chat_connection(socket, state, auth))
}

/// GET /api/v1/ws/events — WebSocket upgrade for system event streaming.
async fn ws_events_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(auth): Query<WsAuthQuery>,
) -> Response {
    ws.on_upgrade(move |socket| handle_events_connection(socket, state, auth))
}

/// Handle a chat WebSocket connection.
async fn handle_chat_connection(mut socket: WebSocket, state: AppState, auth: WsAuthQuery) {
    // Authenticate via token query parameter
    let _key_id = match authenticate_ws(&state, &auth).await {
        Ok(key_id) => {
            // Send authentication success
            let msg = ChatServerMessage::Authenticated {
                user_id: key_id.to_string(),
            };
            if send_chat_message(&mut socket, &msg).await.is_err() {
                return;
            }
            key_id
        }
        Err(err_msg) => {
            let msg = ChatServerMessage::Error {
                session_id: None,
                error_code: "AUTHENTICATION_FAILED".to_string(),
                message: err_msg,
            };
            let _ = send_chat_message(&mut socket, &msg).await;
            let _ = socket.close().await;
            return;
        }
    };

    // Track connection
    {
        let mut tracker = state.ws_state.active_connections.write().await;
        tracker.chat_connections += 1;
    }
    info!("Chat WebSocket connected");

    // Per-connection conversation history, fed back to the agent each turn so
    // it has context. Bounded by the agent's own context window logic.
    let mut history: Vec<common::models::Message> = Vec::new();

    // Main message loop
    loop {
        tokio::select! {
            msg = recv_message(&mut socket) => {
                match msg {
                    Some(Ok(text)) => {
                        match serde_json::from_str::<ChatClientMessage>(&text) {
                            Ok(ChatClientMessage::Message { session_id, content, attachments }) => {
                                handle_chat_message(&mut socket, &state, &session_id, &content, &attachments, &mut history).await;
                            }
                            Ok(ChatClientMessage::Ping) => {
                                let _ = send_chat_message(&mut socket, &ChatServerMessage::Pong).await;
                            }
                            Err(e) => {
                                let msg = ChatServerMessage::Error {
                                    session_id: None,
                                    error_code: "INVALID_MESSAGE".to_string(),
                                    message: format!("Invalid message format: {}", e),
                                };
                                if send_chat_message(&mut socket, &msg).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Err(_)) => break,
                    None => break, // Connection closed
                }
            }
        }
    }

    // Track disconnection
    {
        let mut tracker = state.ws_state.active_connections.write().await;
        tracker.chat_connections = tracker.chat_connections.saturating_sub(1);
    }
    info!("Chat WebSocket disconnected");
}

/// Handle a system events WebSocket connection.
async fn handle_events_connection(mut socket: WebSocket, state: AppState, auth: WsAuthQuery) {
    // Authenticate via token query parameter
    let _key_id = match authenticate_ws(&state, &auth).await {
        Ok(key_id) => {
            let msg = EventsServerMessage::Authenticated {
                user_id: key_id.to_string(),
            };
            if send_events_message(&mut socket, &msg).await.is_err() {
                return;
            }
            key_id
        }
        Err(err_msg) => {
            let msg = EventsServerMessage::Error {
                error_code: "AUTHENTICATION_FAILED".to_string(),
                message: err_msg,
            };
            let _ = send_events_message(&mut socket, &msg).await;
            let _ = socket.close().await;
            return;
        }
    };

    // Track connection
    {
        let mut tracker = state.ws_state.active_connections.write().await;
        tracker.event_connections += 1;
    }
    info!("Events WebSocket connected");

    // Subscribe to the event broadcast channel
    let mut event_rx = state.ws_state.event_tx.subscribe();

    // Main loop: forward events to client, handle client messages
    loop {
        tokio::select! {
            // Receive system events and forward to client
            event = event_rx.recv() => {
                match event {
                    Ok(msg) => {
                        if send_events_message(&mut socket, &msg).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "Events WebSocket client lagged, missed events");
                        // Continue — client will need to request replay if needed
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Handle client messages (replay requests, pings)
            msg = recv_message(&mut socket) => {
                match msg {
                    Some(Ok(text)) => {
                        match serde_json::from_str::<EventsClientMessage>(&text) {
                            Ok(EventsClientMessage::ReplayFrom { last_event_id }) => {
                                let missed = state.ws_state.get_events_after(&last_event_id).await;
                                for event_msg in missed {
                                    if send_events_message(&mut socket, &event_msg).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Ok(EventsClientMessage::Ping) => {
                                let _ = send_events_message(&mut socket, &EventsServerMessage::Pong).await;
                            }
                            Err(e) => {
                                let msg = EventsServerMessage::Error {
                                    error_code: "INVALID_MESSAGE".to_string(),
                                    message: format!("Invalid message format: {}", e),
                                };
                                if send_events_message(&mut socket, &msg).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Some(Err(_)) => break,
                    None => break,
                }
            }
        }
    }

    // Track disconnection
    {
        let mut tracker = state.ws_state.active_connections.write().await;
        tracker.event_connections = tracker.event_connections.saturating_sub(1);
    }
    info!("Events WebSocket disconnected");
}

/// Process a chat message: forward to the agent core (with any uploaded-file
/// context appended) and stream the response back.
async fn handle_chat_message(
    socket: &mut WebSocket,
    state: &AppState,
    session_id: &str,
    content: &str,
    attachments: &[String],
    history: &mut Vec<common::models::Message>,
) {
    use common::models::{Message, MessageRole};
    use common::types::{MessageId, SessionId};

    // Validate session_id format
    let session_uuid = match Uuid::parse_str(session_id) {
        Ok(u) => u,
        Err(_) => {
            let msg = ChatServerMessage::Error {
                session_id: Some(session_id.to_string()),
                error_code: "INVALID_SESSION_ID".to_string(),
                message: "session_id must be a valid UUID".to_string(),
            };
            let _ = send_chat_message(socket, &msg).await;
            return;
        }
    };

    if content.trim().is_empty() && attachments.is_empty() {
        let msg = ChatServerMessage::Error {
            session_id: Some(session_id.to_string()),
            error_code: "EMPTY_CONTENT".to_string(),
            message: "Message content must not be empty".to_string(),
        };
        let _ = send_chat_message(socket, &msg).await;
        return;
    }

    debug!(
        session_id = session_id,
        attachments = attachments.len(),
        "Processing chat message via WebSocket"
    );

    // Compose the message content, appending a note about uploaded files so the
    // agent knows where they are and which tool to use.
    let full_content = compose_content_with_attachments(content, attachments);

    let agent = match &state.agent_core {
        Some(handle) => handle.0.clone(),
        None => {
            let msg = ChatServerMessage::Error {
                session_id: Some(session_id.to_string()),
                error_code: "AGENT_UNAVAILABLE".to_string(),
                message: "Agent core is not attached to this server.".to_string(),
            };
            let _ = send_chat_message(socket, &msg).await;
            return;
        }
    };

    let user_message = Message {
        id: MessageId::new(),
        session_id: SessionId(session_uuid),
        role: MessageRole::User,
        content: full_content.clone(),
        tool_calls: None,
        tool_results: None,
        timestamp: Utc::now(),
        token_count: 0,
    };

    // Run the agent. process_message returns a stream that yields the final
    // response chunk (the runtime does the full tool loop internally).
    let mut stream = agent
        .process_message(
            SessionId(session_uuid),
            user_message.clone(),
            history.clone(),
        )
        .await;

    let mut assistant_text = String::new();
    {
        use futures::StreamExt;
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => {
                    if let Some(text) = chunk.content {
                        if !text.is_empty() {
                            assistant_text.push_str(&text);
                            let msg = ChatServerMessage::Token {
                                session_id: session_id.to_string(),
                                content: text,
                            };
                            if send_chat_message(socket, &msg).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg = ChatServerMessage::Error {
                        session_id: Some(session_id.to_string()),
                        error_code: "AGENT_ERROR".to_string(),
                        message: format!("Agent failed to process the message: {e}"),
                    };
                    let _ = send_chat_message(socket, &msg).await;
                    return;
                }
            }
        }
    }

    // Record this turn in the connection's history.
    history.push(user_message);
    history.push(Message {
        id: MessageId::new(),
        session_id: SessionId(session_uuid),
        role: MessageRole::Assistant,
        content: assistant_text,
        tool_calls: None,
        tool_results: None,
        timestamp: Utc::now(),
        token_count: 0,
    });

    let msg = ChatServerMessage::Done {
        session_id: session_id.to_string(),
        message_id: Uuid::new_v4().to_string(),
    };
    let _ = send_chat_message(socket, &msg).await;
}

/// Append a note describing uploaded files to the user's message so the agent
/// knows where to find them and which tool to use.
fn compose_content_with_attachments(content: &str, attachments: &[String]) -> String {
    if attachments.is_empty() {
        return content.to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut has_image = false;
    for path in attachments {
        let is_img = common::uploads::is_image_filename(path);
        has_image |= is_img;
        lines.push(format!(
            "- {} {}",
            path,
            if is_img { "(image)" } else { "(file)" }
        ));
    }
    let mut tool_hint = String::from("Use `file_read` to read text files");
    if has_image {
        tool_hint.push_str(" and `view_image` to view images");
    }
    tool_hint.push('.');

    let note = format!(
        "\n\n[The user attached the following file(s), saved in the workspace:\n{}\n{}]",
        lines.join("\n"),
        tool_hint
    );
    format!("{}{}", content, note)
}

// --- Helper functions ---

/// Authenticate a WebSocket connection using the token query parameter.
///
/// Accepts both pre-configured API keys and session tokens issued by the
/// `/auth/login` endpoint — mirroring `auth_middleware` for HTTP requests, so
/// the password-login web UI can open WebSocket connections.
async fn authenticate_ws(state: &AppState, auth: &WsAuthQuery) -> Result<Uuid, String> {
    let token = auth.token.as_deref().ok_or_else(|| {
        "Missing token query parameter. Connect with ?token=<api-key>".to_string()
    })?;

    // First try pre-configured API keys.
    if let Ok(key_id) = state.authenticator.authenticate(token, &state.api_keys) {
        return Ok(key_id.0);
    }

    // Fall back to login-issued session tokens.
    if state.login_tokens.read().await.contains(token) {
        return Ok(state.admin_session_key_id.0);
    }

    Err("Invalid API key".to_string())
}

/// Send a serialized chat message over the WebSocket.
async fn send_chat_message(socket: &mut WebSocket, msg: &ChatServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|e| {
        error!("Failed to serialize chat message: {}", e);
    })?;
    socket.send(Message::Text(text.into())).await.map_err(|e| {
        debug!("Failed to send chat message: {}", e);
    })
}

/// Send a serialized events message over the WebSocket.
async fn send_events_message(socket: &mut WebSocket, msg: &EventsServerMessage) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|e| {
        error!("Failed to serialize events message: {}", e);
    })?;
    socket.send(Message::Text(text.into())).await.map_err(|e| {
        debug!("Failed to send events message: {}", e);
    })
}

/// Receive a text message from the WebSocket, handling control frames.
///
/// Returns `None` when the connection is closed, `Some(Err(()))` on error,
/// and `Some(Ok(text))` for text messages. Non-text frames are skipped
/// by returning an empty string which callers should ignore.
async fn recv_message(socket: &mut WebSocket) -> Option<Result<String, ()>> {
    use futures::StreamExt;

    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let text_str = text.to_string();
                if text_str.is_empty() {
                    continue; // Skip empty text frames
                }
                return Some(Ok(text_str));
            }
            Some(Ok(Message::Close(_))) => return None,
            Some(Ok(Message::Ping(_))) => {
                // Axum handles pong automatically, continue receiving
                continue;
            }
            Some(Ok(Message::Pong(_))) => continue,
            Some(Ok(Message::Binary(_))) => continue, // Skip binary frames
            Some(Err(_)) => return Some(Err(())),
            None => return None,
        }
    }
}

/// Build WebSocket routes.
///
/// Note: WebSocket routes handle their own authentication via query parameters
/// since browsers cannot set custom headers on WebSocket upgrade requests.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/ws/chat", get(ws_chat_handler))
        .route("/api/v1/ws/events", get(ws_events_handler))
}
