//! Conversation history persistence.
//!
//! Provides functions to store and retrieve conversation messages
//! with session-scoped isolation. Messages are stored in SQLite with
//! tool_calls and tool_results serialized as JSON.

use chrono::{DateTime, Utc};
use common::{Message, MessageId, MessageRole, SessionId, StoreError, ToolCall, ToolResult};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use uuid::Uuid;

/// Store a message in the database.
///
/// The message's `session_id` determines which session it belongs to.
/// Tool calls and tool results are serialized as JSON text columns.
///
/// Returns the `MessageId` of the stored message.
pub async fn store_message(pool: &SqlitePool, message: &Message) -> Result<MessageId, StoreError> {
    let id_str = message.id.0.to_string();
    let session_id_str = message.session_id.0.to_string();
    let role_str = role_to_str(message.role);
    let timestamp_str = message.timestamp.to_rfc3339();

    let tool_calls_json = message
        .tool_calls
        .as_ref()
        .map(|tc| serde_json::to_string(tc))
        .transpose()
        .map_err(|e| StoreError::CorruptedData {
            details: format!("Failed to serialize tool_calls: {}", e),
        })?;

    let tool_results_json = message
        .tool_results
        .as_ref()
        .map(|tr| serde_json::to_string(tr))
        .transpose()
        .map_err(|e| StoreError::CorruptedData {
            details: format!("Failed to serialize tool_results: {}", e),
        })?;

    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, tool_calls, tool_results, timestamp, token_count)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id_str)
    .bind(&session_id_str)
    .bind(role_str)
    .bind(&message.content)
    .bind(&tool_calls_json)
    .bind(&tool_results_json)
    .bind(&timestamp_str)
    .bind(message.token_count as i64)
    .execute(pool)
    .await
    .map_err(|_| StoreError::Unavailable)?;

    Ok(message.id)
}

/// Retrieve conversation history for a session, ordered by timestamp ascending.
///
/// Returns at most `limit` of the most recent messages for the given session.
/// Messages from other sessions are never included (session isolation).
pub async fn get_history(
    pool: &SqlitePool,
    session_id: SessionId,
    limit: usize,
) -> Result<Vec<Message>, StoreError> {
    let session_id_str = session_id.0.to_string();

    // We select the most recent `limit` messages, then return them in ascending order.
    let rows = sqlx::query(
        "SELECT id, session_id, role, content, tool_calls, tool_results, timestamp, token_count
         FROM messages
         WHERE session_id = ?
         ORDER BY timestamp DESC
         LIMIT ?",
    )
    .bind(&session_id_str)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|_| StoreError::Unavailable)?;

    let mut messages = Vec::with_capacity(rows.len());

    for row in &rows {
        let id_str: String = row.get("id");
        let role_str: String = row.get("role");
        let content: String = row.get("content");
        let tool_calls_json: Option<String> = row.get("tool_calls");
        let tool_results_json: Option<String> = row.get("tool_results");
        let timestamp_str: String = row.get("timestamp");
        let token_count: i64 = row.get("token_count");

        let id = Uuid::parse_str(&id_str).map_err(|e| StoreError::CorruptedData {
            details: format!("Invalid message UUID: {}", e),
        })?;

        let role = str_to_role(&role_str).ok_or_else(|| StoreError::CorruptedData {
            details: format!("Invalid role: {}", role_str),
        })?;

        let tool_calls: Option<Vec<ToolCall>> = tool_calls_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| StoreError::CorruptedData {
                details: format!("Failed to deserialize tool_calls: {}", e),
            })?;

        let tool_results: Option<Vec<ToolResult>> = tool_results_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| StoreError::CorruptedData {
                details: format!("Failed to deserialize tool_results: {}", e),
            })?;

        let timestamp: DateTime<Utc> = DateTime::parse_from_rfc3339(&timestamp_str)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| StoreError::CorruptedData {
                details: format!("Invalid timestamp: {}", e),
            })?;

        messages.push(Message {
            id: MessageId(id),
            session_id,
            role,
            content,
            tool_calls,
            tool_results,
            timestamp,
            token_count: token_count as u32,
        });
    }

    // Reverse to get ascending timestamp order (oldest first)
    messages.reverse();

    Ok(messages)
}

/// Convert a MessageRole to its database string representation.
fn role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
        MessageRole::Tool => "tool",
    }
}

/// Convert a database string to a MessageRole.
fn str_to_role(s: &str) -> Option<MessageRole> {
    match s {
        "user" => Some(MessageRole::User),
        "assistant" => Some(MessageRole::Assistant),
        "system" => Some(MessageRole::System),
        "tool" => Some(MessageRole::Tool),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_database;
    use tempfile::TempDir;

    /// Helper: create a temp database with required FK parent rows.
    async fn setup_test_db() -> (SqlitePool, TempDir, SessionId, SessionId) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test_conv.db");
        let pool = init_database(&db_path).await.expect("failed to init db");

        // Create a user (FK requirement for sessions)
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at)
             VALUES ('user-1', 'testuser', 'hash', 'admin', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Create two sessions for isolation testing
        let session_a = SessionId::new();
        let session_b = SessionId::new();

        sqlx::query(
            "INSERT INTO sessions (id, user_id, mode, created_at, last_activity)
             VALUES (?, 'user-1', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .bind(session_a.0.to_string())
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, user_id, mode, created_at, last_activity)
             VALUES (?, 'user-1', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .bind(session_b.0.to_string())
        .execute(&pool)
        .await
        .unwrap();

        (pool, tmp_dir, session_a, session_b)
    }

    /// Helper: create a test message.
    fn make_message(
        session_id: SessionId,
        role: MessageRole,
        content: &str,
        ts: DateTime<Utc>,
    ) -> Message {
        Message {
            id: MessageId::new(),
            session_id,
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_results: None,
            timestamp: ts,
            token_count: content.len() as u32,
        }
    }

    #[tokio::test]
    async fn test_store_and_retrieve_single_message() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let msg = make_message(session_a, MessageRole::User, "Hello, agent!", Utc::now());
        let stored_id = store_message(&pool, &msg).await.unwrap();
        assert_eq!(stored_id, msg.id);

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Hello, agent!");
        assert_eq!(history[0].role, MessageRole::User);
        assert_eq!(history[0].session_id, session_a);
        assert_eq!(history[0].token_count, 13);
    }

    #[tokio::test]
    async fn test_session_isolation() {
        let (pool, _tmp, session_a, session_b) = setup_test_db().await;

        let msg_a = make_message(session_a, MessageRole::User, "Message for A", Utc::now());
        let msg_b = make_message(session_b, MessageRole::User, "Message for B", Utc::now());

        store_message(&pool, &msg_a).await.unwrap();
        store_message(&pool, &msg_b).await.unwrap();

        let history_a = get_history(&pool, session_a, 100).await.unwrap();
        let history_b = get_history(&pool, session_b, 100).await.unwrap();

        assert_eq!(history_a.len(), 1);
        assert_eq!(history_b.len(), 1);
        assert_eq!(history_a[0].content, "Message for A");
        assert_eq!(history_b[0].content, "Message for B");

        // Ensure no cross-contamination
        assert!(history_a.iter().all(|m| m.session_id == session_a));
        assert!(history_b.iter().all(|m| m.session_id == session_b));
    }

    #[tokio::test]
    async fn test_message_ordering() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let base_time = Utc::now();
        let msg1 = make_message(
            session_a,
            MessageRole::User,
            "First",
            base_time - chrono::Duration::seconds(2),
        );
        let msg2 = make_message(
            session_a,
            MessageRole::Assistant,
            "Second",
            base_time - chrono::Duration::seconds(1),
        );
        let msg3 = make_message(session_a, MessageRole::User, "Third", base_time);

        // Insert out of order
        store_message(&pool, &msg3).await.unwrap();
        store_message(&pool, &msg1).await.unwrap();
        store_message(&pool, &msg2).await.unwrap();

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "First");
        assert_eq!(history[1].content, "Second");
        assert_eq!(history[2].content, "Third");
    }

    #[tokio::test]
    async fn test_limit_returns_most_recent() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let base_time = Utc::now();
        for i in 0..10 {
            let msg = make_message(
                session_a,
                MessageRole::User,
                &format!("Message {}", i),
                base_time + chrono::Duration::seconds(i),
            );
            store_message(&pool, &msg).await.unwrap();
        }

        // Request only 3 most recent
        let history = get_history(&pool, session_a, 3).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].content, "Message 7");
        assert_eq!(history[1].content, "Message 8");
        assert_eq!(history[2].content, "Message 9");
    }

    #[tokio::test]
    async fn test_tool_calls_persistence() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let msg = Message {
            id: MessageId::new(),
            session_id: session_a,
            role: MessageRole::Assistant,
            content: "Let me search for that.".to_string(),
            tool_calls: Some(vec![ToolCall {
                id: "call_abc123".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust async"}),
            }]),
            tool_results: None,
            timestamp: Utc::now(),
            token_count: 42,
        };

        store_message(&pool, &msg).await.unwrap();

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 1);

        let tc = history[0].tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "call_abc123");
        assert_eq!(tc[0].name, "web_search");
        assert_eq!(tc[0].arguments, serde_json::json!({"query": "rust async"}));
    }

    #[tokio::test]
    async fn test_tool_results_persistence() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let msg = Message {
            id: MessageId::new(),
            session_id: session_a,
            role: MessageRole::Tool,
            content: "".to_string(),
            tool_calls: None,
            tool_results: Some(vec![ToolResult {
                tool_call_id: "call_abc123".to_string(),
                output: "Found 5 results for rust async".to_string(),
                is_error: false,
            }]),
            timestamp: Utc::now(),
            token_count: 15,
        };

        store_message(&pool, &msg).await.unwrap();

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 1);

        let tr = history[0].tool_results.as_ref().unwrap();
        assert_eq!(tr.len(), 1);
        assert_eq!(tr[0].tool_call_id, "call_abc123");
        assert_eq!(tr[0].output, "Found 5 results for rust async");
        assert!(!tr[0].is_error);
    }

    #[tokio::test]
    async fn test_all_roles_stored_correctly() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let roles = [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::System,
            MessageRole::Tool,
        ];

        let base_time = Utc::now();
        for (i, role) in roles.iter().enumerate() {
            let msg = make_message(
                session_a,
                *role,
                &format!("{:?} message", role),
                base_time + chrono::Duration::seconds(i as i64),
            );
            store_message(&pool, &msg).await.unwrap();
        }

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].role, MessageRole::User);
        assert_eq!(history[1].role, MessageRole::Assistant);
        assert_eq!(history[2].role, MessageRole::System);
        assert_eq!(history[3].role, MessageRole::Tool);
    }

    #[tokio::test]
    async fn test_empty_history_for_new_session() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_supports_1000_messages_per_session() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let base_time = Utc::now();
        for i in 0..1000 {
            let msg = make_message(
                session_a,
                MessageRole::User,
                &format!("Message number {}", i),
                base_time + chrono::Duration::milliseconds(i),
            );
            store_message(&pool, &msg).await.unwrap();
        }

        let history = get_history(&pool, session_a, 1000).await.unwrap();
        assert_eq!(history.len(), 1000);
        // Verify ordering: first message should be "Message number 0"
        assert_eq!(history[0].content, "Message number 0");
        assert_eq!(history[999].content, "Message number 999");
    }

    #[tokio::test]
    async fn test_timestamp_preserved_accurately() {
        let (pool, _tmp, session_a, _) = setup_test_db().await;

        let specific_time = DateTime::parse_from_rfc3339("2024-06-15T10:30:45.123456789Z")
            .unwrap()
            .with_timezone(&Utc);

        let msg = make_message(session_a, MessageRole::User, "Timed message", specific_time);
        store_message(&pool, &msg).await.unwrap();

        let history = get_history(&pool, session_a, 100).await.unwrap();
        assert_eq!(history.len(), 1);
        // RFC3339 round-trip may lose sub-second precision depending on chrono formatting,
        // but seconds should be preserved.
        assert_eq!(history[0].timestamp.timestamp(), specific_time.timestamp());
    }
}
