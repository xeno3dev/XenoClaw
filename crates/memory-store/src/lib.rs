//! Memory Store — persistent storage for conversations, knowledge, and context.
//!
//! Responsibilities:
//! - Store and retrieve conversation history
//! - Semantic search over stored content (sqlite-vec)
//! - Explicit knowledge storage and management
//! - Session state persistence and recovery

pub mod conversation;
pub mod db;
pub mod knowledge;
pub mod session_state;

pub use conversation::{get_history, store_message};
pub use db::{init_database, DbError};
pub use knowledge::{KnowledgeStore, SearchResult, DEFAULT_MAX_ENTRIES, MAX_CONTENT_SIZE};
pub use session_state::{
    create_session_state_table, restore_session_state, save_session_state, SessionState,
    SessionStateFallback,
};
