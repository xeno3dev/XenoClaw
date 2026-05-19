//! Agent Core — the central runtime that orchestrates all agent operations.
//!
//! Responsibilities:
//! - Manage agent lifecycle (startup, shutdown, restart)
//! - Process incoming messages and route to LLM
//! - Coordinate tool execution via Tool Registry
//! - Manage session state and context windows
//! - Emit events to the Event Bus
//!
//! # Architecture
//!
//! The Agent Core sits at the center of the platform, receiving messages from
//! the API layer and coordinating between the LLM Router (for completions),
//! the Tool Registry (for tool execution), and the Memory Store (for persistence).
//!
//! ```text
//! API Server → Agent Core → LLM Router → Provider
//!                  ↕              ↕
//!            Tool Registry   Memory Store
//! ```
//!
//! # Agent Modes
//!
//! The agent supports two operational modes:
//! - **General**: Base tools only (search, knowledge, task management)
//! - **Coding**: Additional development tools (file ops, terminal, git, LSP)

pub mod event_bus;
pub mod runtime;
pub mod session_manager;
pub mod tool_registry;
pub mod types;

pub use event_bus::{Event, EventBus, EventReceiver, EventRecvError, EventType};
pub use runtime::AgentCore;
pub use session_manager::{
    KnowledgeEntry, SessionContext, SessionError, SessionManager, SessionManagerConfig,
};
pub use tool_registry::{Tool, ToolRegistry};
pub use types::{
    AgentCoreConfig, AgentMode, AgentStatus, ModeError, ResponseChunk, ResponseStream,
    ShutdownError,
};
