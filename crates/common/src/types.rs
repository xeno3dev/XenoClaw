//! Core type definitions for the VPS AI Agent Platform.
//!
//! These types are used across all crates in the workspace to ensure
//! consistent identification and type safety.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a conversation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

/// Unique identifier for a scheduled or triggered task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub Uuid);

/// Unique identifier for a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub Uuid);

/// Unique identifier for a platform user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub Uuid);

/// Unique identifier for a stored knowledge entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KnowledgeId(pub Uuid);

/// Unique identifier for an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ApiKeyId(pub Uuid);

// --- Constructors ---

impl SessionId {
    /// Create a new random SessionId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskId {
    /// Create a new random TaskId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageId {
    /// Create a new random MessageId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl UserId {
    /// Create a new random UserId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeId {
    /// Create a new random KnowledgeId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for KnowledgeId {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyId {
    /// Create a new random ApiKeyId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ApiKeyId {
    fn default() -> Self {
        Self::new()
    }
}

// --- Display implementations ---

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for KnowledgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for ApiKeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_creation() {
        let id = SessionId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_task_id_creation() {
        let id = TaskId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_message_id_creation() {
        let id = MessageId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_user_id_creation() {
        let id = UserId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_knowledge_id_creation() {
        let id = KnowledgeId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_api_key_id_creation() {
        let id = ApiKeyId::new();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_ids_are_unique() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_id_serialization() {
        let id = SessionId::new();
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }
}
