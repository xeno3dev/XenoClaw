//! Session router for messaging platform integration.
//!
//! Maps (Platform, user_id/channel_id) pairs to unique SessionIds,
//! ensuring each messaging platform user gets their own isolated session
//! with separate history and context.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info};

use common::types::SessionId;

use crate::Platform;

/// Maps messaging platform users to their agent sessions.
///
/// Each (platform, user_id) pair gets a unique, isolated session.
/// The router is designed to be shared across all platform bots via `Arc<SessionRouter>`.
pub struct SessionRouter {
    /// In-memory mapping of (platform, user_id) → SessionId
    sessions: Arc<RwLock<HashMap<(Platform, String), SessionId>>>,
}

impl SessionRouter {
    /// Create a new empty session router.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a session for the given platform user.
    ///
    /// If a session already exists for this (platform, user_id) pair, returns it.
    /// Otherwise, creates a new SessionId and stores the mapping.
    pub async fn get_or_create_session(
        &self,
        platform: Platform,
        user_id: &str,
    ) -> SessionId {
        let key = (platform, user_id.to_string());

        // Fast path: check if session already exists (read lock)
        {
            let sessions = self.sessions.read().await;
            if let Some(session_id) = sessions.get(&key) {
                debug!(
                    platform = ?platform,
                    user_id = user_id,
                    session_id = %session_id,
                    "Found existing session"
                );
                return *session_id;
            }
        }

        // Slow path: create a new session (write lock)
        let mut sessions = self.sessions.write().await;
        // Double-check after acquiring write lock (another task may have created it)
        if let Some(session_id) = sessions.get(&key) {
            return *session_id;
        }

        let session_id = SessionId::new();
        sessions.insert(key, session_id);
        info!(
            platform = ?platform,
            user_id = user_id,
            session_id = %session_id,
            "Created new session"
        );
        session_id
    }

    /// Get an existing session without creating one.
    ///
    /// Returns `None` if no session exists for this (platform, user_id) pair.
    pub async fn get_session(
        &self,
        platform: Platform,
        user_id: &str,
    ) -> Option<SessionId> {
        let key = (platform, user_id.to_string());
        let sessions = self.sessions.read().await;
        sessions.get(&key).copied()
    }

    /// Remove a session mapping (e.g., when a user is deauthorized).
    ///
    /// Returns the removed SessionId if one existed.
    pub async fn remove_session(
        &self,
        platform: Platform,
        user_id: &str,
    ) -> Option<SessionId> {
        let key = (platform, user_id.to_string());
        let mut sessions = self.sessions.write().await;
        let removed = sessions.remove(&key);
        if let Some(session_id) = removed {
            info!(
                platform = ?platform,
                user_id = user_id,
                session_id = %session_id,
                "Removed session"
            );
        }
        removed
    }

    /// Get all active session mappings (for persistence/debugging).
    pub async fn all_sessions(&self) -> Vec<(Platform, String, SessionId)> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .map(|((platform, user_id), session_id)| (*platform, user_id.clone(), *session_id))
            .collect()
    }

    /// Get the number of active sessions.
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }
}

impl Default for SessionRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_or_create_session_creates_new() {
        let router = SessionRouter::new();
        let session = router
            .get_or_create_session(Platform::Telegram, "user123")
            .await;

        // Should be able to retrieve the same session
        let retrieved = router.get_session(Platform::Telegram, "user123").await;
        assert_eq!(retrieved, Some(session));
    }

    #[tokio::test]
    async fn test_get_or_create_session_returns_existing() {
        let router = SessionRouter::new();
        let session1 = router
            .get_or_create_session(Platform::Telegram, "user123")
            .await;
        let session2 = router
            .get_or_create_session(Platform::Telegram, "user123")
            .await;

        assert_eq!(session1, session2);
    }

    #[tokio::test]
    async fn test_different_users_get_different_sessions() {
        let router = SessionRouter::new();
        let session1 = router
            .get_or_create_session(Platform::Telegram, "user1")
            .await;
        let session2 = router
            .get_or_create_session(Platform::Telegram, "user2")
            .await;

        assert_ne!(session1, session2);
    }

    #[tokio::test]
    async fn test_different_platforms_get_different_sessions() {
        let router = SessionRouter::new();
        let session1 = router
            .get_or_create_session(Platform::Telegram, "user123")
            .await;
        let session2 = router
            .get_or_create_session(Platform::Discord, "user123")
            .await;

        assert_ne!(session1, session2);
    }

    #[tokio::test]
    async fn test_get_session_returns_none_for_unknown() {
        let router = SessionRouter::new();
        let result = router.get_session(Platform::Telegram, "unknown").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_remove_session() {
        let router = SessionRouter::new();
        let session = router
            .get_or_create_session(Platform::Telegram, "user123")
            .await;

        let removed = router
            .remove_session(Platform::Telegram, "user123")
            .await;
        assert_eq!(removed, Some(session));

        // Should no longer exist
        let result = router.get_session(Platform::Telegram, "user123").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_session() {
        let router = SessionRouter::new();
        let removed = router
            .remove_session(Platform::Telegram, "nonexistent")
            .await;
        assert_eq!(removed, None);
    }

    #[tokio::test]
    async fn test_all_sessions() {
        let router = SessionRouter::new();
        router
            .get_or_create_session(Platform::Telegram, "user1")
            .await;
        router
            .get_or_create_session(Platform::Discord, "user2")
            .await;

        let all = router.all_sessions().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_session_count() {
        let router = SessionRouter::new();
        assert_eq!(router.session_count().await, 0);

        router
            .get_or_create_session(Platform::Telegram, "user1")
            .await;
        assert_eq!(router.session_count().await, 1);

        router
            .get_or_create_session(Platform::Discord, "user2")
            .await;
        assert_eq!(router.session_count().await, 2);

        router.remove_session(Platform::Telegram, "user1").await;
        assert_eq!(router.session_count().await, 1);
    }
}
