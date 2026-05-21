//! Task Scheduler — manages scheduled and event-driven task execution.
//!
//! Responsibilities:
//! - Parse and evaluate cron expressions
//! - Monitor filesystem events, webhooks, and time conditions
//! - Manage task dependencies and execution ordering
//! - Handle retries with exponential backoff
//! - Maintain task history log

pub mod cron_parser;
pub mod dependency;
pub mod file_watcher;
pub mod retry;
pub mod scheduler;
pub mod time_condition;
pub mod webhook;

pub use cron_parser::CronSchedule;
pub use dependency::{DependencyGraph, MAX_DEPENDENCY_DEPTH};
pub use file_watcher::FileWatcher;
pub use retry::{
    calculate_backoff, cleanup_old_history, execute_with_retry, DEFAULT_RETENTION_DAYS,
};
pub use scheduler::{Scheduler, SchedulerConfig, TaskExecutor};
pub use time_condition::TimeCondition;
pub use webhook::WebhookRegistry;
