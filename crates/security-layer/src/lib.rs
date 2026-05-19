//! Security Layer — handles authentication, authorization, sandboxing, and audit logging.
//!
//! Responsibilities:
//! - Authenticate requests (API keys, username/password)
//! - Enforce RBAC policies
//! - Manage sandbox boundaries (filesystem, network, resources)
//! - Rate limiting and brute-force protection
//! - Audit logging of all security events

pub mod audit;
pub mod auth;
pub mod brute_force;
pub mod rate_limit;
pub mod rbac;
pub mod sandbox;

pub use audit::{AuditEvent, AuditLogger, AuditLoggerConfig, AuditOutcome};
pub use brute_force::{BruteForceConfig, BruteForceProtection};
pub use rate_limit::{RateLimitConfig, RateLimiter, DEFAULT_RATE_LIMIT};
pub use sandbox::{validate_command, validate_network, validate_path, ResourceEnforcer};
