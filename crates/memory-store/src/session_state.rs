//! Session state persistence and recovery.
//!
//! Provides full session state serialization (task queue, conversation history,
//! scheduler config) with persistence to SQLite and fallback to in-memory
//! storage when the database is unavailable.
//!
//! Implements Requirements 2.3 (persist conversation history and learned context
//! across restarts) and 14.7 (continue operating using in-memory context when
//! Memory_Store is unavailable, retrying every 30 seconds).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{error, info, warn};

use common::config::SchedulerConfig;
use common::errors::StoreError;
use common::types::{SessionId, TaskId};

/// Retry interval for persistence when the store is unavailable.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Full session state that is persisted across restarts.
///
/// Captures everything needed to restore a session to its previous state:
/// - Session metadata (ID, user, timestamps)
/// - Active task queue (ordered list of pending task IDs)
/// - Conversation context summary (compressed representation of history)
/// - Scheduler configuration snapshot at time of persistence
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    /// The session this state belongs to.
    pub session_id: SessionId,

    /// Ordered list of task IDs in the active queue.
    pub task_queue: Vec<TaskId>,

    /// Summarized conversation context for continuity across restarts.
    pub conversation_context: String,

    /// Snapshot of the scheduler configuration at persistence time.
    pub scheduler_config: SchedulerConfig,

    /// Timestamp when this state was last persisted.
    pub persisted_at: DateTime<Utc>,

    /// Additional metadata for extensibility.
    pub metadata: HashMap<String, String>,
}

impl SessionState {
    /// Create a new SessionState with the given parameters.
    pub fn new(
        session_id: SessionId,
        task_queue: Vec<TaskId>,
        conversation_context: String,
        scheduler_config: SchedulerConfig,
    ) -> Self {
        Self {
            session_id,
            task_queue,
            conversation_context,
            scheduler_config,
            persisted_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }
}

/// Create the session_state table in the database.
///
/// This should be called during database initialization.
pub async fn create_session_state_table(pool: &SqlitePool) -> Result<(), StoreError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS session_state (
            session_id TEXT PRIMARY KEY,
            state_json TEXT NOT NULL,
            persisted_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(|_| StoreError::Unavailable)?;

    Ok(())
}

/// Persist a session state to the database.
///
/// Serializes the state as JSON and upserts it into the `session_state` table.
/// If a state already exists for this session, it is replaced.
pub async fn save_session_state(
    pool: &SqlitePool,
    state: &SessionState,
) -> Result<(), StoreError> {
    let state_json =
        serde_json::to_string(state).map_err(|e| StoreError::CorruptedData {
            details: format!("failed to serialize session state: {e}"),
        })?;

    let session_id_str = state.session_id.0.to_string();
    let persisted_at_str = state.persisted_at.to_rfc3339();

    sqlx::query(
        "INSERT OR REPLACE INTO session_state (session_id, state_json, persisted_at)
         VALUES (?, ?, ?)",
    )
    .bind(&session_id_str)
    .bind(&state_json)
    .bind(&persisted_at_str)
    .execute(pool)
    .await
    .map_err(|_| StoreError::Unavailable)?;

    Ok(())
}

/// Restore a session state from the database.
///
/// Returns `None` if no persisted state exists for the given session ID.
pub async fn restore_session_state(
    pool: &SqlitePool,
    session_id: SessionId,
) -> Result<Option<SessionState>, StoreError> {
    let session_id_str = session_id.0.to_string();

    let row = sqlx::query("SELECT state_json FROM session_state WHERE session_id = ?")
        .bind(&session_id_str)
        .fetch_optional(pool)
        .await
        .map_err(|_| StoreError::Unavailable)?;

    match row {
        Some(row) => {
            let state_json: String = row.get("state_json");
            let state: SessionState =
                serde_json::from_str(&state_json).map_err(|e| StoreError::CorruptedData {
                    details: format!("failed to deserialize session state: {e}"),
                })?;
            Ok(Some(state))
        }
        None => Ok(None),
    }
}

/// Fallback session state manager that operates in-memory when the store is unavailable.
///
/// When the database is unreachable, states are held in memory and a background
/// task retries persistence every 30 seconds until the store becomes available.
pub struct SessionStateFallback {
    /// The database pool (may be unavailable).
    pool: SqlitePool,

    /// In-memory state cache, keyed by session ID.
    cache: Arc<RwLock<HashMap<SessionId, SessionState>>>,

    /// Whether the store is currently available.
    store_available: Arc<RwLock<bool>>,
}

impl SessionStateFallback {
    /// Create a new fallback manager.
    ///
    /// Immediately checks store availability and starts the retry loop
    /// if the store is unavailable.
    pub async fn new(pool: SqlitePool) -> Self {
        let available = Self::check_store_available(&pool).await;

        let fallback = Self {
            pool,
            cache: Arc::new(RwLock::new(HashMap::new())),
            store_available: Arc::new(RwLock::new(available)),
        };

        if !available {
            warn!("Memory store unavailable, operating in fallback mode");
            fallback.start_retry_loop();
        }

        fallback
    }

    /// Save session state, falling back to in-memory if the store is unavailable.
    pub async fn save(&self, state: &SessionState) -> Result<(), StoreError> {
        // Always update the in-memory cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(state.session_id, state.clone());
        }

        // Try to persist to the database
        let available = *self.store_available.read().await;
        if available {
            match save_session_state(&self.pool, state).await {
                Ok(()) => Ok(()),
                Err(_) => {
                    // Store became unavailable
                    warn!("Store became unavailable during save, switching to fallback mode");
                    *self.store_available.write().await = false;
                    self.start_retry_loop();
                    Ok(()) // State is in memory, not lost
                }
            }
        } else {
            // Operating in fallback mode, state is in memory
            Ok(())
        }
    }

    /// Restore session state, checking in-memory cache first, then the database.
    pub async fn restore(
        &self,
        session_id: SessionId,
    ) -> Result<Option<SessionState>, StoreError> {
        // Check in-memory cache first
        {
            let cache = self.cache.read().await;
            if let Some(state) = cache.get(&session_id) {
                return Ok(Some(state.clone()));
            }
        }

        // Try the database
        let available = *self.store_available.read().await;
        if available {
            match restore_session_state(&self.pool, session_id).await {
                Ok(state) => Ok(state),
                Err(_) => {
                    warn!("Store became unavailable during restore, switching to fallback mode");
                    *self.store_available.write().await = false;
                    self.start_retry_loop();
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Check if the store is currently available.
    pub async fn is_store_available(&self) -> bool {
        *self.store_available.read().await
    }

    /// Start the background retry loop that attempts to flush in-memory state
    /// to the database every 30 seconds.
    fn start_retry_loop(&self) {
        let pool = self.pool.clone();
        let cache = Arc::clone(&self.cache);
        let store_available = Arc::clone(&self.store_available);

        tokio::spawn(async move {
            loop {
                time::sleep(RETRY_INTERVAL).await;

                // Check if store is back
                if Self::check_store_available(&pool).await {
                    info!("Memory store is available again, flushing cached states");

                    // Flush all cached states
                    let states: Vec<SessionState> = {
                        let cache_guard = cache.read().await;
                        cache_guard.values().cloned().collect()
                    };

                    let mut all_flushed = true;
                    for state in &states {
                        if let Err(e) = save_session_state(&pool, state).await {
                            error!("Failed to flush session state {}: {e}", state.session_id);
                            all_flushed = false;
                        }
                    }

                    if all_flushed {
                        *store_available.write().await = true;
                        info!("All cached session states flushed successfully");
                        break;
                    }
                } else {
                    warn!("Memory store still unavailable, will retry in 30 seconds");
                }
            }
        });
    }

    /// Check if the database is reachable by executing a simple query.
    async fn check_store_available(pool: &SqlitePool) -> bool {
        sqlx::query("SELECT 1")
            .fetch_one(pool)
            .await
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::types::TaskId;
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::db::init_database;

    /// Helper to create a test database with the session_state table.
    async fn setup_test_db() -> (SqlitePool, TempDir) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test_session_state.db");
        let pool = init_database(&db_path).await.expect("failed to init db");
        create_session_state_table(&pool)
            .await
            .expect("failed to create session_state table");
        (pool, tmp_dir)
    }

    /// Create a sample SessionState for testing.
    fn sample_state(session_id: SessionId) -> SessionState {
        SessionState::new(
            session_id,
            vec![TaskId::new(), TaskId::new(), TaskId::new()],
            "User prefers concise responses. Last discussed deployment configs.".to_string(),
            SchedulerConfig {
                max_concurrent_tasks: 5,
                default_timeout_seconds: 120,
                max_dependency_depth: 8,
            },
        )
    }

    #[tokio::test]
    async fn test_round_trip_persistence() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();
        let state = sample_state(session_id);

        // Save
        save_session_state(&pool, &state).await.unwrap();

        // Restore
        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .expect("state should exist");

        assert_eq!(state.session_id, restored.session_id);
        assert_eq!(state.task_queue, restored.task_queue);
        assert_eq!(state.conversation_context, restored.conversation_context);
        assert_eq!(
            state.scheduler_config.max_concurrent_tasks,
            restored.scheduler_config.max_concurrent_tasks
        );
        assert_eq!(
            state.scheduler_config.default_timeout_seconds,
            restored.scheduler_config.default_timeout_seconds
        );
        assert_eq!(
            state.scheduler_config.max_dependency_depth,
            restored.scheduler_config.max_dependency_depth
        );
    }

    #[tokio::test]
    async fn test_restore_nonexistent_session() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        let result = restore_session_state(&pool, session_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_save_overwrites_existing() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        // Save initial state
        let state1 = SessionState::new(
            session_id,
            vec![TaskId::new()],
            "First context".to_string(),
            SchedulerConfig::default(),
        );
        save_session_state(&pool, &state1).await.unwrap();

        // Save updated state
        let state2 = SessionState::new(
            session_id,
            vec![TaskId::new(), TaskId::new()],
            "Updated context".to_string(),
            SchedulerConfig {
                max_concurrent_tasks: 20,
                ..SchedulerConfig::default()
            },
        );
        save_session_state(&pool, &state2).await.unwrap();

        // Restore should return the latest
        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored.conversation_context, "Updated context");
        assert_eq!(restored.task_queue.len(), 2);
        assert_eq!(restored.scheduler_config.max_concurrent_tasks, 20);
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent() {
        let (pool, _tmp) = setup_test_db().await;

        let session_a = SessionId::new();
        let session_b = SessionId::new();

        let state_a = SessionState::new(
            session_a,
            vec![TaskId::new()],
            "Context A".to_string(),
            SchedulerConfig::default(),
        );
        let state_b = SessionState::new(
            session_b,
            vec![TaskId::new(), TaskId::new()],
            "Context B".to_string(),
            SchedulerConfig {
                max_concurrent_tasks: 3,
                ..SchedulerConfig::default()
            },
        );

        save_session_state(&pool, &state_a).await.unwrap();
        save_session_state(&pool, &state_b).await.unwrap();

        let restored_a = restore_session_state(&pool, session_a)
            .await
            .unwrap()
            .unwrap();
        let restored_b = restore_session_state(&pool, session_b)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored_a.conversation_context, "Context A");
        assert_eq!(restored_b.conversation_context, "Context B");
        assert_eq!(restored_a.task_queue.len(), 1);
        assert_eq!(restored_b.task_queue.len(), 2);
    }

    #[tokio::test]
    async fn test_empty_task_queue_round_trip() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        let state = SessionState::new(
            session_id,
            vec![],
            String::new(),
            SchedulerConfig::default(),
        );
        save_session_state(&pool, &state).await.unwrap();

        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .unwrap();

        assert!(restored.task_queue.is_empty());
        assert!(restored.conversation_context.is_empty());
    }

    #[tokio::test]
    async fn test_metadata_round_trip() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        let mut state = sample_state(session_id);
        state.metadata.insert("version".to_string(), "1.0".to_string());
        state
            .metadata
            .insert("last_model".to_string(), "gpt-4".to_string());

        save_session_state(&pool, &state).await.unwrap();

        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored.metadata.get("version").unwrap(), "1.0");
        assert_eq!(restored.metadata.get("last_model").unwrap(), "gpt-4");
    }

    #[tokio::test]
    async fn test_fallback_save_and_restore_in_memory() {
        let (pool, _tmp) = setup_test_db().await;
        let fallback = SessionStateFallback::new(pool).await;

        let session_id = SessionId::new();
        let state = sample_state(session_id);

        // Save via fallback
        fallback.save(&state).await.unwrap();

        // Restore via fallback (should find in cache)
        let restored = fallback.restore(session_id).await.unwrap().unwrap();
        assert_eq!(restored.session_id, state.session_id);
        assert_eq!(restored.conversation_context, state.conversation_context);
    }

    #[tokio::test]
    async fn test_fallback_store_available_check() {
        let (pool, _tmp) = setup_test_db().await;
        let fallback = SessionStateFallback::new(pool).await;

        // Store should be available with a valid pool
        assert!(fallback.is_store_available().await);
    }

    #[tokio::test]
    async fn test_fallback_restore_from_db_when_not_in_cache() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();
        let state = sample_state(session_id);

        // Save directly to DB (bypassing fallback cache)
        save_session_state(&pool, &state).await.unwrap();

        // Create a fresh fallback (empty cache)
        let fallback = SessionStateFallback::new(pool).await;

        // Should find it in the database
        let restored = fallback.restore(session_id).await.unwrap().unwrap();
        assert_eq!(restored.session_id, session_id);
        assert_eq!(restored.conversation_context, state.conversation_context);
    }

    #[tokio::test]
    async fn test_large_task_queue_round_trip() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        let large_queue: Vec<TaskId> = (0..100).map(|_| TaskId::new()).collect();
        let state = SessionState::new(
            session_id,
            large_queue.clone(),
            "Large queue context".to_string(),
            SchedulerConfig::default(),
        );

        save_session_state(&pool, &state).await.unwrap();

        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored.task_queue.len(), 100);
        // Verify order is preserved
        for (original, restored_id) in large_queue.iter().zip(restored.task_queue.iter()) {
            assert_eq!(original, restored_id);
        }
    }

    #[tokio::test]
    async fn test_special_characters_in_context() {
        let (pool, _tmp) = setup_test_db().await;
        let session_id = SessionId::new();

        let context = r#"User said: "Hello, world!" and asked about 'quotes' & <special> chars.
Also newlines
and tabs	here. Unicode: 日本語 🎉"#;

        let state = SessionState::new(
            session_id,
            vec![],
            context.to_string(),
            SchedulerConfig::default(),
        );

        save_session_state(&pool, &state).await.unwrap();

        let restored = restore_session_state(&pool, session_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored.conversation_context, context);
    }

    #[tokio::test]
    async fn test_session_state_serialization_deterministic() {
        let session_id = SessionId(Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap());
        let task_id = TaskId(Uuid::parse_str("abcdefab-abcd-abcd-abcd-abcdefabcdef").unwrap());

        let state = SessionState {
            session_id,
            task_queue: vec![task_id],
            conversation_context: "test".to_string(),
            scheduler_config: SchedulerConfig::default(),
            persisted_at: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            metadata: HashMap::new(),
        };

        let json1 = serde_json::to_string(&state).unwrap();
        let json2 = serde_json::to_string(&state).unwrap();
        assert_eq!(json1, json2);

        // Verify it deserializes back correctly
        let deserialized: SessionState = serde_json::from_str(&json1).unwrap();
        assert_eq!(deserialized.session_id, state.session_id);
        assert_eq!(deserialized.task_queue, state.task_queue);
    }
}
