//! Process Supervisor — manages agent lifecycle, health monitoring, and automatic recovery.
//!
//! Responsibilities:
//! - Detect unexpected agent termination and restart within 10 seconds
//! - Auto-start agent within 30 seconds of OS reaching default run level
//! - Restore previous session state on restart (task queue, history, scheduler config)
//! - Restart agent if unresponsive for more than 60 seconds
//! - Maintain uptime and restart history statistics
//!
//! # Architecture
//!
//! The ProcessSupervisor manages the AgentCore within the same process via health
//! checks and restart logic. It runs a background monitoring loop that:
//! - Pings the agent every 15 seconds
//! - Triggers restart if no response for 60 seconds
//! - On unexpected termination, restarts within 10 seconds
//! - Logs all restart events
//!
//! Requirements: 2.2, 2.7, 18.4

pub mod health;
pub mod stats;
pub mod supervisor;
pub mod types;

pub use supervisor::ProcessSupervisor;
pub use types::{
    HealthStatus, RestartEvent, RestartReason, SupervisorConfig, SupervisorError, SupervisorStats,
};
