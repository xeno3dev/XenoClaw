use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::http::{Request, Uri};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::{connect_async, WebSocketStream};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{ClientCommand, ServerEvent};
use crate::app::{AgentStatus, ConnectionState};

/// How long to wait between reconnect attempts (with backoff).
const BASE_RECONNECT_DELAY: Duration = Duration::from_secs(5);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const MAX_RECONNECT_ATTEMPTS: u8 = 6;
/// How often to send a ping to keep the connection alive.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// Spawn both client tasks and return command senders and event receiver.
pub fn spawn_client_tasks(
    http_client: crate::client::http::ApiClient,
    ws_url: String,
    api_key: String,
    session_id: Uuid,
) -> (
    mpsc::Sender<ClientCommand>,
    mpsc::Sender<ClientCommand>,
    mpsc::Receiver<ServerEvent>,
) {
    let (http_cmd_tx, http_cmd_rx) = mpsc::channel::<ClientCommand>(64);
    let (ws_cmd_tx, ws_cmd_rx) = mpsc::channel::<ClientCommand>(64);
    let (event_tx, event_rx) = mpsc::channel::<ServerEvent>(64);

    tokio::spawn(http_task_loop(
        http_client,
        session_id,
        http_cmd_rx,
        event_tx.clone(),
    ));
    tokio::spawn(ws_task_loop(
        ws_url, api_key, session_id, ws_cmd_rx, event_tx,
    ));

    (http_cmd_tx, ws_cmd_tx, event_rx)
}

/// HTTP task loop: polls status/tasks periodically, handles commands.
async fn http_task_loop(
    client: crate::client::http::ApiClient,
    _session_id: Uuid,
    mut cmd_rx: mpsc::Receiver<ClientCommand>,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    let mut status_interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = status_interval.tick() => {
                match client.status().await {
                    Ok(val) => {
                        let status_str = val.get("status").and_then(|s| s.as_str()).unwrap_or("idle");
                        let task = val.get("current_task").and_then(|t| t.as_str());

                        let agent_status = match status_str {
                            "working" | "busy" => AgentStatus::Working {
                                task: task.unwrap_or("unknown").to_string(),
                                progress: None,
                            },
                            "error" => AgentStatus::Error {
                                message: task.unwrap_or("unknown error").to_string(),
                            },
                            _ => AgentStatus::Idle,
                        };
                        let _ = event_tx.send(ServerEvent::Status {
                            status: agent_status,
                            cpu: val.get("cpu").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                            memory_mb: val.get("memory_mb").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                            active_tasks: Vec::new(),
                        }).await;
                    }
                    Err(e) => {
                        debug!("Status poll error: {e}");
                    }
                }

                match client.tasks().await {
                    Ok(tasks) => {
                        if !tasks.is_empty() {
                            let _ = event_tx.send(ServerEvent::Status {
                                status: AgentStatus::Idle,
                                cpu: 0.0,
                                memory_mb: 0.0,
                                active_tasks: tasks,
                            }).await;
                        }
                    }
                    Err(e) => {
                        debug!("Tasks poll error: {e}");
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ClientCommand::Shutdown) | None => {
                        info!("HTTP task shutting down");
                        break;
                    }
                    Some(ClientCommand::SendMessage { content }) => {
                        match client.send_message(&_session_id.to_string(), &content).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = event_tx.send(ServerEvent::Error {
                                    message: format!("Failed to send message: {e}"),
                                }).await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// WebSocket task loop: connects, reads frames, handles reconnect and commands.
async fn ws_task_loop(
    ws_url: String,
    api_key: String,
    session_id: Uuid,
    mut cmd_rx: mpsc::Receiver<ClientCommand>,
    event_tx: mpsc::Sender<ServerEvent>,
) {
    let mut reconnect_attempts: u8 = 0;

    loop {
        match connect_ws(&ws_url, &api_key).await {
            Ok(mut ws) => {
                reconnect_attempts = 0;
                let _ = event_tx
                    .send(ServerEvent::Connection(ConnectionState::Connected))
                    .await;

                if let Err(()) = ws_read_loop(&mut ws, &mut cmd_rx, &session_id, &event_tx).await {
                    // Connection lost — will reconnect
                }

                let _ = event_tx
                    .send(ServerEvent::Connection(ConnectionState::Disconnected {
                        since: std::time::Instant::now(),
                        reconnect_attempts: 0,
                    }))
                    .await;
            }
            Err(e) => {
                let msg = format!("WebSocket connection failed: {e}");
                warn!("{msg}");
                let _ = event_tx.send(ServerEvent::Error { message: msg }).await;
            }
        }

        // Reconnect with backoff
        reconnect_attempts += 1;
        if reconnect_attempts > MAX_RECONNECT_ATTEMPTS {
            let _ = event_tx
                .send(ServerEvent::Error {
                    message: "Max reconnect attempts reached. Use /reconnect to try again."
                        .to_string(),
                })
                .await;
            wait_for_reconnect_or_shutdown(&mut cmd_rx, &event_tx).await;
            reconnect_attempts = 0;
            continue;
        }

        let delay = BASE_RECONNECT_DELAY
            .saturating_mul(reconnect_attempts as u32)
            .min(MAX_RECONNECT_DELAY);

        let _ = event_tx
            .send(ServerEvent::Connection(ConnectionState::Reconnecting {
                attempt: reconnect_attempts,
            }))
            .await;

        let delay_fut = tokio::time::sleep(delay);
        tokio::pin!(delay_fut);
        tokio::select! {
            _ = &mut delay_fut => {}
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ClientCommand::Shutdown) | None => {
                        info!("WS task shutting down during reconnect wait");
                        return;
                    }
                    Some(ClientCommand::Reconnect) => {}
                    _ => {}
                }
            }
        }
    }
}

/// Connect to the WebSocket server with authentication.
async fn connect_ws(
    ws_url: &str,
    api_key: &str,
) -> Result<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, String> {
    let uri: Uri = ws_url
        .parse()
        .map_err(|e: <Uri as std::str::FromStr>::Err| format!("Invalid WebSocket URL: {e}"))?;
    let auth_value = format!("Bearer {}", api_key);

    let request = Request::builder()
        .uri(uri)
        .header("Authorization", auth_value)
        .body(())
        .map_err(|e| format!("Failed to build WebSocket request: {e}"))?;

    let (ws, _response) = connect_async(request)
        .await
        .map_err(|e| format!("WebSocket connection failed: {e}"))?;
    Ok(ws)
}

/// Read loop: processes frames and handles client commands.
async fn ws_read_loop(
    ws: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    cmd_rx: &mut mpsc::Receiver<ClientCommand>,
    session_id: &Uuid,
    event_tx: &mpsc::Sender<ServerEvent>,
) -> Result<(), ()> {
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);

    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_ws_text(text.to_string(), event_tx).await;
                    }
                    Some(Ok(Message::Ping(_))) => {}
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => {
                        debug!("WebSocket closed");
                        return Err(());
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {e}");
                        return Err(());
                    }
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                if ws.send(Message::Ping(vec![])).await.is_err() {
                    return Err(());
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ClientCommand::SendMessage { content }) => {
                        let msg = serde_json::json!({
                            "type": "message",
                            "session_id": session_id.to_string(),
                            "content": content,
                        });
                        if ws.send(Message::Text(msg.to_string())).await.is_err() {
                            return Err(());
                        }
                    }
                    Some(ClientCommand::CancelGeneration) => {
                        let _ = event_tx.send(ServerEvent::Error {
                            message: "Cancellation not supported by server.".to_string(),
                        }).await;
                    }
                    Some(ClientCommand::Reconnect) => {
                        return Err(());
                    }
                    Some(ClientCommand::Shutdown) | None => {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Parse an incoming WebSocket text message and emit the appropriate event.
async fn handle_ws_text(text: String, event_tx: &mpsc::Sender<ServerEvent>) {
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(val) => {
            let msg_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match msg_type {
                "token" => {
                    let content = val.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    let _ = event_tx
                        .send(ServerEvent::AssistantToken {
                            content: content.to_string(),
                        })
                        .await;
                }
                "done" => {
                    let _ = event_tx.send(ServerEvent::AssistantDone).await;
                }
                "error" => {
                    let message = val
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    let _ = event_tx
                        .send(ServerEvent::Error {
                            message: message.to_string(),
                        })
                        .await;
                }
                "authenticated" => {
                    info!("WebSocket authenticated");
                }
                "pong" => {}
                _ => {
                    debug!("Unknown WS message type: {msg_type}");
                }
            }
        }
        Err(e) => {
            warn!("Failed to parse WS message: {e}");
            let _ = event_tx
                .send(ServerEvent::Error {
                    message: format!("Failed to parse server message: {e}"),
                })
                .await;
        }
    }
}

/// Wait until the user triggers a reconnect or sends shutdown.
async fn wait_for_reconnect_or_shutdown(
    cmd_rx: &mut mpsc::Receiver<ClientCommand>,
    _event_tx: &mpsc::Sender<ServerEvent>,
) {
    loop {
        match cmd_rx.recv().await {
            Some(ClientCommand::Reconnect) => break,
            Some(ClientCommand::Shutdown) | None => return,
            _ => {}
        }
    }
}
