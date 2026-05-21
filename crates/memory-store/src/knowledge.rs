//! Knowledge storage, retrieval, and semantic search.
//!
//! Provides explicit knowledge storage with capacity enforcement,
//! content size limits, and text-based search (with fallback when
//! sqlite-vec is unavailable).

use chrono::Utc;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use std::collections::HashMap;
use tracing::info;

use common::errors::StoreError;
use common::types::KnowledgeId;

/// Maximum content size per knowledge entry (10,000 characters).
pub const MAX_CONTENT_SIZE: usize = 10_000;

/// Default maximum number of knowledge entries.
pub const DEFAULT_MAX_ENTRIES: u64 = 100_000;

/// A result from semantic/text search over knowledge entries.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The knowledge entry ID.
    pub id: KnowledgeId,
    /// The entry title.
    pub title: String,
    /// The entry content.
    pub content: String,
    /// Tags associated with the entry.
    pub tags: Vec<String>,
    /// Metadata key-value pairs.
    pub metadata: HashMap<String, String>,
    /// Relevance score (0.0 to 1.0, higher is more relevant).
    pub relevance_score: f32,
}

/// Knowledge store providing storage, deletion, and search over knowledge entries.
#[derive(Debug, Clone)]
pub struct KnowledgeStore {
    pool: SqlitePool,
    max_entries: u64,
}

impl KnowledgeStore {
    /// Create a new KnowledgeStore with the given connection pool and max entry limit.
    pub fn new(pool: SqlitePool, max_entries: u64) -> Self {
        Self { pool, max_entries }
    }

    /// Create a new KnowledgeStore with the default max entries limit.
    pub fn with_defaults(pool: SqlitePool) -> Self {
        Self::new(pool, DEFAULT_MAX_ENTRIES)
    }

    /// Store a new knowledge entry.
    ///
    /// Returns the ID of the newly created entry.
    ///
    /// # Errors
    /// - `StoreError::ContentTooLarge` if content exceeds 10,000 characters
    /// - `StoreError::CapacityFull` if the store has reached its configured max entries
    pub async fn store_knowledge(
        &self,
        title: &str,
        content: &str,
        tags: &[String],
        metadata: &HashMap<String, String>,
    ) -> Result<KnowledgeId, StoreError> {
        // Validate content size
        if content.len() > MAX_CONTENT_SIZE {
            return Err(StoreError::ContentTooLarge {
                max: MAX_CONTENT_SIZE,
                actual: content.len(),
            });
        }

        // Check capacity
        let count = self.entry_count().await?;
        if count >= self.max_entries {
            return Err(StoreError::CapacityFull);
        }

        let id = KnowledgeId::new();
        let now = Utc::now().to_rfc3339();
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
        let metadata_json = serde_json::to_string(metadata).unwrap_or_else(|_| "{}".to_string());

        sqlx::query(
            "INSERT INTO knowledge (id, title, content, tags, metadata, embedding, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, NULL, ?, ?)",
        )
        .bind(id.0.to_string())
        .bind(title)
        .bind(content)
        .bind(&tags_json)
        .bind(&metadata_json)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|_| StoreError::Unavailable)?;

        info!(id = %id, title = %title, "Knowledge entry stored");

        Ok(id)
    }

    /// Permanently delete a knowledge entry.
    ///
    /// # Errors
    /// - `StoreError::EntryNotFound` if no entry with the given ID exists
    pub async fn delete_knowledge(&self, id: KnowledgeId) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM knowledge WHERE id = ?")
            .bind(id.0.to_string())
            .execute(&self.pool)
            .await
            .map_err(|_| StoreError::Unavailable)?;

        if result.rows_affected() == 0 {
            return Err(StoreError::EntryNotFound {
                id: id.0.to_string(),
            });
        }

        info!(id = %id, "Knowledge entry deleted");

        Ok(())
    }

    /// Search knowledge entries using text-based matching.
    ///
    /// Uses LIKE queries as a fallback when sqlite-vec is unavailable.
    /// Results are ranked by a simple relevance heuristic based on
    /// match location (title matches score higher than content matches).
    ///
    /// # Arguments
    /// - `query` - The search query string
    /// - `limit` - Maximum number of results to return
    pub async fn search_knowledge(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SearchResult>, StoreError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_pattern = format!("%{}%", query);

        // Search across title, content, and tags using LIKE
        // Order by relevance: title matches first, then content matches
        let rows = sqlx::query(
            "SELECT id, title, content, tags, metadata,
                    CASE
                        WHEN title LIKE ?1 AND content LIKE ?1 THEN 1.0
                        WHEN title LIKE ?1 THEN 0.9
                        WHEN tags LIKE ?1 THEN 0.7
                        ELSE 0.5
                    END as relevance
             FROM knowledge
             WHERE title LIKE ?1 OR content LIKE ?1 OR tags LIKE ?1
             ORDER BY relevance DESC, created_at DESC
             LIMIT ?2",
        )
        .bind(&search_pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| StoreError::Unavailable)?;

        let results = rows
            .iter()
            .map(|row| {
                let id_str: String = row.get("id");
                let title: String = row.get("title");
                let content: String = row.get("content");
                let tags_json: Option<String> = row.get("tags");
                let metadata_json: Option<String> = row.get("metadata");
                let relevance: f64 = row.get("relevance");

                let tags: Vec<String> = tags_json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default();

                let metadata: HashMap<String, String> = metadata_json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default();

                let uuid = uuid::Uuid::parse_str(&id_str).unwrap_or_else(|_| uuid::Uuid::nil());

                SearchResult {
                    id: KnowledgeId(uuid),
                    title,
                    content,
                    tags,
                    metadata,
                    relevance_score: relevance as f32,
                }
            })
            .collect();

        Ok(results)
    }

    /// Get the current number of knowledge entries in the store.
    pub async fn entry_count(&self) -> Result<u64, StoreError> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM knowledge")
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StoreError::Unavailable)?;

        let count: i64 = row.get("count");
        Ok(count as u64)
    }

    /// Get the configured maximum number of entries.
    pub fn max_entries(&self) -> u64 {
        self.max_entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_database;
    use tempfile::TempDir;

    async fn setup_store() -> (KnowledgeStore, TempDir) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test_knowledge.db");
        let pool = init_database(&db_path).await.expect("failed to init db");
        let store = KnowledgeStore::with_defaults(pool);
        (store, tmp_dir)
    }

    async fn setup_store_with_capacity(max_entries: u64) -> (KnowledgeStore, TempDir) {
        let tmp_dir = TempDir::new().expect("failed to create temp dir");
        let db_path = tmp_dir.path().join("test_knowledge.db");
        let pool = init_database(&db_path).await.expect("failed to init db");
        let store = KnowledgeStore::new(pool, max_entries);
        (store, tmp_dir)
    }

    #[tokio::test]
    async fn test_store_and_retrieve_knowledge() {
        let (store, _tmp) = setup_store().await;

        let tags = vec!["rust".to_string(), "programming".to_string()];
        let mut metadata = HashMap::new();
        metadata.insert("source".to_string(), "manual".to_string());

        let id = store
            .store_knowledge(
                "Rust Ownership",
                "Rust uses ownership for memory safety.",
                &tags,
                &metadata,
            )
            .await
            .unwrap();

        // Verify we can find it via search
        let results = store.search_knowledge("Rust", 10).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].id, id);
        assert_eq!(results[0].title, "Rust Ownership");
        assert_eq!(results[0].content, "Rust uses ownership for memory safety.");
        assert_eq!(results[0].tags, tags);
        assert_eq!(
            results[0].metadata.get("source"),
            Some(&"manual".to_string())
        );
    }

    #[tokio::test]
    async fn test_content_too_large_rejected() {
        let (store, _tmp) = setup_store().await;

        let large_content = "x".repeat(MAX_CONTENT_SIZE + 1);
        let result = store
            .store_knowledge("Too Large", &large_content, &[], &HashMap::new())
            .await;

        assert!(
            matches!(result, Err(StoreError::ContentTooLarge { max: 10_000, actual }) if actual == MAX_CONTENT_SIZE + 1)
        );
    }

    #[tokio::test]
    async fn test_content_at_max_size_accepted() {
        let (store, _tmp) = setup_store().await;

        let max_content = "x".repeat(MAX_CONTENT_SIZE);
        let result = store
            .store_knowledge("Max Size", &max_content, &[], &HashMap::new())
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_capacity_enforcement() {
        let (store, _tmp) = setup_store_with_capacity(3).await;

        // Fill to capacity
        for i in 0..3 {
            store
                .store_knowledge(&format!("Entry {}", i), "content", &[], &HashMap::new())
                .await
                .unwrap();
        }

        // Next entry should be rejected
        let result = store
            .store_knowledge("Overflow", "content", &[], &HashMap::new())
            .await;

        assert!(matches!(result, Err(StoreError::CapacityFull)));
    }

    #[tokio::test]
    async fn test_capacity_after_deletion_allows_new_entries() {
        let (store, _tmp) = setup_store_with_capacity(2).await;

        let id1 = store
            .store_knowledge("Entry 1", "content 1", &[], &HashMap::new())
            .await
            .unwrap();
        store
            .store_knowledge("Entry 2", "content 2", &[], &HashMap::new())
            .await
            .unwrap();

        // At capacity
        let result = store
            .store_knowledge("Entry 3", "content 3", &[], &HashMap::new())
            .await;
        assert!(matches!(result, Err(StoreError::CapacityFull)));

        // Delete one entry
        store.delete_knowledge(id1).await.unwrap();

        // Now we can add again
        let result = store
            .store_knowledge("Entry 3", "content 3", &[], &HashMap::new())
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_knowledge() {
        let (store, _tmp) = setup_store().await;

        let id = store
            .store_knowledge("To Delete", "will be removed", &[], &HashMap::new())
            .await
            .unwrap();

        // Verify it exists
        let results = store.search_knowledge("Delete", 10).await.unwrap();
        assert_eq!(results.len(), 1);

        // Delete it
        store.delete_knowledge(id).await.unwrap();

        // Verify it's gone
        let results = store.search_knowledge("Delete", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_entry() {
        let (store, _tmp) = setup_store().await;

        let fake_id = KnowledgeId::new();
        let result = store.delete_knowledge(fake_id).await;

        assert!(matches!(result, Err(StoreError::EntryNotFound { .. })));
    }

    #[tokio::test]
    async fn test_search_empty_query_returns_empty() {
        let (store, _tmp) = setup_store().await;

        store
            .store_knowledge("Test", "content", &[], &HashMap::new())
            .await
            .unwrap();

        let results = store.search_knowledge("", 10).await.unwrap();
        assert!(results.is_empty());

        let results = store.search_knowledge("   ", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_no_matches() {
        let (store, _tmp) = setup_store().await;

        store
            .store_knowledge("Rust Guide", "Learn Rust programming", &[], &HashMap::new())
            .await
            .unwrap();

        let results = store.search_knowledge("Python", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_by_title() {
        let (store, _tmp) = setup_store().await;

        store
            .store_knowledge(
                "Rust Ownership",
                "Memory safety concept",
                &[],
                &HashMap::new(),
            )
            .await
            .unwrap();
        store
            .store_knowledge(
                "Python Basics",
                "Dynamic typing language",
                &[],
                &HashMap::new(),
            )
            .await
            .unwrap();

        let results = store.search_knowledge("Rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust Ownership");
    }

    #[tokio::test]
    async fn test_search_by_content() {
        let (store, _tmp) = setup_store().await;

        store
            .store_knowledge(
                "Guide",
                "Learn about borrow checker in Rust",
                &[],
                &HashMap::new(),
            )
            .await
            .unwrap();

        let results = store.search_knowledge("borrow checker", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Guide");
    }

    #[tokio::test]
    async fn test_search_by_tags() {
        let (store, _tmp) = setup_store().await;

        let tags = vec!["networking".to_string(), "tcp".to_string()];
        store
            .store_knowledge("Sockets", "How to use sockets", &tags, &HashMap::new())
            .await
            .unwrap();

        let results = store.search_knowledge("networking", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Sockets");
    }

    #[tokio::test]
    async fn test_search_respects_limit() {
        let (store, _tmp) = setup_store().await;

        for i in 0..10 {
            store
                .store_knowledge(
                    &format!("Rust Topic {}", i),
                    "Rust content",
                    &[],
                    &HashMap::new(),
                )
                .await
                .unwrap();
        }

        let results = store.search_knowledge("Rust", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_search_title_match_ranks_higher() {
        let (store, _tmp) = setup_store().await;

        // Entry with "Rust" only in content
        store
            .store_knowledge("Programming Guide", "Learn Rust here", &[], &HashMap::new())
            .await
            .unwrap();

        // Entry with "Rust" in title
        store
            .store_knowledge(
                "Rust Ownership",
                "Memory safety concept",
                &[],
                &HashMap::new(),
            )
            .await
            .unwrap();

        let results = store.search_knowledge("Rust", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        // Title match should rank higher
        assert_eq!(results[0].title, "Rust Ownership");
    }

    #[tokio::test]
    async fn test_entry_count() {
        let (store, _tmp) = setup_store().await;

        assert_eq!(store.entry_count().await.unwrap(), 0);

        store
            .store_knowledge("Entry 1", "content", &[], &HashMap::new())
            .await
            .unwrap();
        assert_eq!(store.entry_count().await.unwrap(), 1);

        store
            .store_knowledge("Entry 2", "content", &[], &HashMap::new())
            .await
            .unwrap();
        assert_eq!(store.entry_count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_deleted_entry_not_retrievable() {
        let (store, _tmp) = setup_store().await;

        let id = store
            .store_knowledge(
                "Secret",
                "This should be permanently deleted",
                &["secret".to_string()],
                &HashMap::new(),
            )
            .await
            .unwrap();

        store.delete_knowledge(id).await.unwrap();

        // Not findable by any search term
        let results = store.search_knowledge("Secret", 10).await.unwrap();
        assert!(results.is_empty());

        let results = store
            .search_knowledge("permanently deleted", 10)
            .await
            .unwrap();
        assert!(results.is_empty());

        let results = store.search_knowledge("secret", 10).await.unwrap();
        assert!(results.is_empty());
    }
}
