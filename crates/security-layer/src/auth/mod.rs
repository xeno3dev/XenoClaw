//! Authentication subsystem for the VPS AI Agent Platform.
//!
//! Provides:
//! - API key validation and hashing
//! - Password hashing and verification (bcrypt)
//! - Session token generation and validation with inactivity timeout
//! - Uniform error responses that don't reveal resource existence

pub mod api_key;
pub mod password;
pub mod session;

pub use api_key::ApiKeyAuthenticator;
pub use password::PasswordAuthenticator;
pub use session::SessionManager;

use common::errors::SecurityError;

/// Credentials that can be presented for authentication.
#[derive(Debug, Clone)]
pub enum Credentials {
    /// A raw API key string.
    ApiKey(String),
    /// Username and password pair.
    UsernamePassword { username: String, password: String },
    /// An existing session token.
    SessionToken(String),
}

/// The result of a successful authentication — always returns the same error
/// type on failure regardless of the reason, to avoid leaking information
/// about whether a user or key exists.
pub type AuthResult<T> = Result<T, SecurityError>;
