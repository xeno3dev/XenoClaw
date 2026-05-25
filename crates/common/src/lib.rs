//! Common types and utilities shared across the VPS AI Agent Platform.
//!
//! This crate provides the foundational type definitions used by all other
//! crates in the workspace, including unique identifiers, data models,
//! and common error types.

pub mod alerting;
pub mod config;
pub mod errors;
pub mod logging;
pub mod metrics;
pub mod models;
pub mod types;
pub mod uploads;

pub use errors::*;
pub use logging::{LogConfig, LogEntry, LogLevel, StructuredLogger};
pub use models::*;
pub use types::*;

/// Test utilities for serializing access to global Prometheus metrics.
#[cfg(test)]
pub(crate) mod test_utils {
    use std::sync::Mutex;

    /// Global mutex to serialize tests that modify shared Prometheus metric gauges.
    /// Use `lock_metrics()` to acquire the guard.
    pub static METRICS_LOCK: Mutex<()> = Mutex::new(());

    /// Acquire the metrics lock, recovering from poisoned state.
    pub fn lock_metrics() -> std::sync::MutexGuard<'static, ()> {
        METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}
