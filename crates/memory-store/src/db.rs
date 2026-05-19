//! SQLite database initialization and schema management.
//!
//! Provides database setup including table creation, index creation,
//! and optional sqlite-vec extension loading for vector search.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;
use tracing::{info, warn};

/// Errors that can occur during database initialization.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to connect to database: {0}")]
    Connection(#[from] sqlx::Error),

    #[error("failed to run migrations: {0}")]
    Migration(String),
}

/// Initialize the SQLite database at the given path.
///
/// This function:
/// - Creates the database file if it doesn't exist
/// - Creates all required tables
/// - Creates all performance indexes
/// - Attempts to load the sqlite-vec extension (logs warning if unavailable)
///
/// Returns a connection pool ready for use.
pub async fn init_database(path: &Path) -> Result<SqlitePool, DbError> {
    let db_url = format!("sqlite:{}?mode=rwc", path.display());

    let options = SqliteConnectOptions::from_str(&db_url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Enable WAL mode and foreign keys
    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await?;

    // Create all tables
    create_tables(&pool).await?;

    // Create all indexes
    create_indexes(&pool).await?;

    // Attempt to load sqlite-vec extension
    load_sqlite_vec(&pool).await;

    info!("Database initialized successfully at {}", path.display());

    Ok(pool)
}

/// Create all database tables.
async fn create_tables(pool: &SqlitePool) -> Result<(), DbError> {
    // Users table (must be created before sessions and api_keys due to FK references)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_login TEXT
        )",
    )
    .execute(pool)
    .await?;

    // Sessions table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            mode TEXT NOT NULL DEFAULT 'general',
            created_at TEXT NOT NULL,
            last_activity TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Messages table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id),
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            tool_calls TEXT,
            tool_results TEXT,
            timestamp TEXT NOT NULL,
            token_count INTEGER NOT NULL DEFAULT 0,
            embedding BLOB
        )",
    )
    .execute(pool)
    .await?;

    // Knowledge store
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS knowledge (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            tags TEXT,
            metadata TEXT,
            embedding BLOB,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Tasks table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            trigger_type TEXT NOT NULL,
            trigger_config TEXT NOT NULL,
            action_type TEXT NOT NULL,
            action_config TEXT NOT NULL,
            timeout_seconds INTEGER NOT NULL DEFAULT 300,
            dependencies TEXT,
            retry_max INTEGER NOT NULL DEFAULT 3,
            retry_base_interval INTEGER NOT NULL DEFAULT 10,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Task runs table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_runs (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL REFERENCES tasks(id),
            start_time TEXT NOT NULL,
            end_time TEXT,
            duration_ms INTEGER,
            status TEXT NOT NULL,
            error TEXT,
            attempt INTEGER NOT NULL DEFAULT 1
        )",
    )
    .execute(pool)
    .await?;

    // API keys table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS api_keys (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            key_hash TEXT NOT NULL,
            name TEXT NOT NULL,
            rate_limit INTEGER NOT NULL DEFAULT 100,
            created_at TEXT NOT NULL,
            last_used TEXT
        )",
    )
    .execute(pool)
    .await?;

    // Messaging identities table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messaging_identities (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            platform TEXT NOT NULL,
            platform_user_id TEXT NOT NULL,
            verified INTEGER NOT NULL DEFAULT 0,
            UNIQUE(platform, platform_user_id)
        )",
    )
    .execute(pool)
    .await?;

    // Audit log table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS audit_log (
            id TEXT PRIMARY KEY,
            timestamp TEXT NOT NULL,
            source_ip TEXT,
            user_id TEXT,
            action TEXT NOT NULL,
            outcome TEXT NOT NULL,
            details TEXT
        )",
    )
    .execute(pool)
    .await?;

    // File changes table (coding module)
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS file_changes (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            operation TEXT NOT NULL,
            before_content TEXT,
            after_content TEXT,
            diff TEXT,
            timestamp TEXT NOT NULL,
            sequence_number INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Session state persistence table
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS session_state (
            session_id TEXT PRIMARY KEY,
            state_json TEXT NOT NULL,
            persisted_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    info!("All database tables created successfully");
    Ok(())
}

/// Create all performance indexes.
async fn create_indexes(pool: &SqlitePool) -> Result<(), DbError> {
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, timestamp)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_runs_task ON task_runs(task_id, start_time)",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp)")
        .execute(pool)
        .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_file_changes_session ON file_changes(session_id, sequence_number)",
    )
    .execute(pool)
    .await?;

    info!("All database indexes created successfully");
    Ok(())
}

/// Attempt to load the sqlite-vec extension for vector search.
///
/// This is a best-effort operation — if the extension is not available
/// on the system, we log a warning and continue without vector search.
async fn load_sqlite_vec(pool: &SqlitePool) {
    // sqlite-vec is typically loaded as a shared library extension.
    // Common paths vary by system. We try a few known locations.
    let extension_names = [
        "vec0",
        "sqlite_vec",
        "sqlite-vec",
    ];

    for ext_name in &extension_names {
        let result = sqlx::query(&format!(
            "SELECT load_extension('{}')",
            ext_name
        ))
        .execute(pool)
        .await;

        match result {
            Ok(_) => {
                info!("sqlite-vec extension loaded successfully ({})", ext_name);
                return;
            }
            Err(_) => continue,
        }
    }

    warn!(
        "sqlite-vec extension not available — vector search will be disabled. \
         Install sqlite-vec for semantic search capabilities."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper to create a temporary database for testing.
    async fn setup_test_db() -> (SqlitePool, TempDir) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test.db");
        let pool = init_database(&db_path).await.expect("failed to init db");
        (pool, tmp_dir)
    }

    #[tokio::test]
    async fn test_database_creates_file() {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("new_test.db");
        assert!(!db_path.exists());

        let _pool = init_database(&db_path).await.unwrap();
        assert!(db_path.exists());
    }

    #[tokio::test]
    async fn test_all_tables_created() {
        let (pool, _tmp) = setup_test_db().await;

        let expected_tables = [
            "sessions",
            "messages",
            "knowledge",
            "tasks",
            "task_runs",
            "users",
            "api_keys",
            "messaging_identities",
            "audit_log",
            "file_changes",
        ];

        for table_name in &expected_tables {
            let result = sqlx::query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table_name)
            .fetch_optional(&pool)
            .await
            .unwrap();

            assert!(
                result.is_some(),
                "Table '{}' should exist in the database",
                table_name
            );
        }
    }

    #[tokio::test]
    async fn test_all_indexes_created() {
        let (pool, _tmp) = setup_test_db().await;

        let expected_indexes = [
            "idx_messages_session",
            "idx_task_runs_task",
            "idx_audit_timestamp",
            "idx_file_changes_session",
        ];

        for index_name in &expected_indexes {
            let result = sqlx::query(
                "SELECT name FROM sqlite_master WHERE type='index' AND name=?",
            )
            .bind(index_name)
            .fetch_optional(&pool)
            .await
            .unwrap();

            assert!(
                result.is_some(),
                "Index '{}' should exist in the database",
                index_name
            );
        }
    }

    #[tokio::test]
    async fn test_idempotent_initialization() {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("idempotent.db");

        // Initialize twice — should not error
        let pool1 = init_database(&db_path).await.unwrap();
        drop(pool1);

        let _pool2 = init_database(&db_path).await.unwrap();
    }

    #[tokio::test]
    async fn test_foreign_key_enforcement() {
        let (pool, _tmp) = setup_test_db().await;

        // Attempting to insert a session with a non-existent user_id should fail
        let result = sqlx::query(
            "INSERT INTO sessions (id, user_id, mode, created_at, last_activity) \
             VALUES ('sess-1', 'nonexistent-user', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "Foreign key constraint should prevent inserting session with invalid user_id"
        );
    }

    #[tokio::test]
    async fn test_messages_table_schema() {
        let (pool, _tmp) = setup_test_db().await;

        // Insert a user and session first (FK requirements)
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at) \
             VALUES ('user-1', 'testuser', 'hash', 'admin', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO sessions (id, user_id, mode, created_at, last_activity) \
             VALUES ('sess-1', 'user-1', 'general', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert a message with all fields
        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, tool_calls, tool_results, timestamp, token_count, embedding) \
             VALUES ('msg-1', 'sess-1', 'user', 'Hello', '[{\"id\":\"1\",\"name\":\"test\"}]', NULL, '2024-01-01T00:00:01Z', 5, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Verify the message was inserted
        let row: (String,) = sqlx::query_as("SELECT content FROM messages WHERE id = 'msg-1'")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(row.0, "Hello");
    }

    #[tokio::test]
    async fn test_knowledge_table_schema() {
        let (pool, _tmp) = setup_test_db().await;

        sqlx::query(
            "INSERT INTO knowledge (id, title, content, tags, metadata, embedding, created_at, updated_at) \
             VALUES ('k-1', 'Test Knowledge', 'Some content', '[\"tag1\",\"tag2\"]', '{\"key\":\"value\"}', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let row: (String, String) =
            sqlx::query_as("SELECT title, tags FROM knowledge WHERE id = 'k-1'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(row.0, "Test Knowledge");
        assert_eq!(row.1, "[\"tag1\",\"tag2\"]");
    }

    #[tokio::test]
    async fn test_unique_constraints() {
        let (pool, _tmp) = setup_test_db().await;

        // Insert a user
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at) \
             VALUES ('user-1', 'testuser', 'hash', 'admin', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Duplicate username should fail
        let result = sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at) \
             VALUES ('user-2', 'testuser', 'hash2', 'operator', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "Unique constraint on username should prevent duplicate"
        );
    }

    #[tokio::test]
    async fn test_messaging_identities_unique_constraint() {
        let (pool, _tmp) = setup_test_db().await;

        // Insert a user
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at) \
             VALUES ('user-1', 'testuser', 'hash', 'admin', '2024-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert a messaging identity
        sqlx::query(
            "INSERT INTO messaging_identities (id, user_id, platform, platform_user_id, verified) \
             VALUES ('mi-1', 'user-1', 'telegram', 'tg-12345', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Duplicate platform + platform_user_id should fail
        let result = sqlx::query(
            "INSERT INTO messaging_identities (id, user_id, platform, platform_user_id, verified) \
             VALUES ('mi-2', 'user-1', 'telegram', 'tg-12345', 0)",
        )
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "Unique constraint on (platform, platform_user_id) should prevent duplicate"
        );
    }

    #[tokio::test]
    async fn test_sqlite_vec_graceful_failure() {
        // sqlite-vec is likely not installed in test environments.
        // This test verifies that init_database succeeds even without it.
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("no_vec.db");

        let pool = init_database(&db_path).await;
        assert!(
            pool.is_ok(),
            "Database initialization should succeed even without sqlite-vec"
        );
    }
}
