//! Middleware for authentication and rate limiting.
//!
//! Provides an axum middleware layer that:
//! 1. Extracts the Bearer token from the Authorization header
//! 2. Validates the API key (minimum 32 chars, matches a stored key)
//! 3. Checks rate limits for the authenticated key
//! 4. Injects the authenticated ApiKeyId into request extensions

pub mod auth;

pub use auth::AuthLayer;
