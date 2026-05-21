//! WebSocket connection management (deprecated — use `client::ws` module).
//!
//! Retained for backward compatibility. The new client architecture
//! lives in `crate::client::ws` and `crate::client::http`.

pub use crate::client::ws::spawn_client_tasks;
pub use crate::client::ClientCommand;
pub use crate::client::ServerEvent;

/// Legacy connection status — use `crate::app::ConnectionState` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    Reconnecting,
    Failed,
}
