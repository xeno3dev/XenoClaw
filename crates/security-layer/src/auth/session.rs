//! Session token generation and validation.
//!
//! Sessions are created after successful authentication and have a configurable
//! inactivity timeout (default 30 minutes). Session tokens are cryptographically
//! random hex strings.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{Duration, Utc};
use rand::Rng;

use common::errors::SecurityError;
use common::models::{AgentMode, Session};
use common::types::{SessionId, UserId};

/// Length of generated session tokens in bytes (64 hex characters).
const SESSION_TOKEN_BYTES: usize = 32;

/// Manages session creation, validation, and expiration.
#[derive(Debug, Clone)]
pub struct SessionManager {
    /// Active sessions indexed by token for fast lookup.
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    /// Session inactivity timeout in minutes.
    timeout_minutes: u32,
}

impl SessionManager {
    /// Create a new session manager with the specified inactivity timeout.
    pub fn new(timeout_minutes: u32) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            timeout_minutes,
        }
    }

    /// Create a new session for an authenticated user.
    ///
    /// Generates a cryptographically random session token and stores the session.
    pub fn create_session(&self, user_id: UserId) -> Session {
        let token = generate_session_token();
        let now = Utc::now();

        let session = Session {
            id: SessionId::new(),
            user_id,
            created_at: now,
            last_activity: now,
            token: token.clone(),
            mode: AgentMode::default(),
        };

        let mut sessions = self.sessions.write().unwrap();
        sessions.insert(token, session.clone());

        session
    }

    /// Validate a session token and update its last activity timestamp.
    ///
    /// Returns the session if valid and not expired, or a uniform error.
    /// The error does not reveal whether the token exists or is expired.
    pub fn validate_session(&self, token: &str) -> Result<Session, SecurityError> {
        let mut sessions = self.sessions.write().unwrap();

        let session = sessions.get(token);

        match session {
            Some(session) => {
                let now = Utc::now();
                let timeout = Duration::minutes(self.timeout_minutes as i64);

                if now - session.last_activity > timeout {
                    // Session expired — remove it and return error.
                    sessions.remove(token);
                    return Err(SecurityError::SessionExpired);
                }

                // Update last activity (touch the session).
                let mut updated = session.clone();
                updated.last_activity = now;
                sessions.insert(token.to_string(), updated.clone());

                Ok(updated)
            }
            None => {
                // Token not found — return the same error as expired
                // to avoid revealing whether the token ever existed.
                Err(SecurityError::SessionExpired)
            }
        }
    }

    /// Invalidate (destroy) a session by its token.
    ///
    /// Returns `Ok(())` regardless of whether the session existed,
    /// to avoid information leakage.
    pub fn invalidate_session(&self, token: &str) -> Result<(), SecurityError> {
        let mut sessions = self.sessions.write().unwrap();
        sessions.remove(token);
        Ok(())
    }

    /// Remove all expired sessions from the store.
    ///
    /// This should be called periodically to prevent memory leaks.
    pub fn cleanup_expired(&self) -> usize {
        let mut sessions = self.sessions.write().unwrap();
        let now = Utc::now();
        let timeout = Duration::minutes(self.timeout_minutes as i64);

        let before_count = sessions.len();
        sessions.retain(|_, session| now - session.last_activity <= timeout);
        before_count - sessions.len()
    }

    /// Get the number of active (non-expired) sessions.
    pub fn active_session_count(&self) -> usize {
        let sessions = self.sessions.read().unwrap();
        let now = Utc::now();
        let timeout = Duration::minutes(self.timeout_minutes as i64);

        sessions
            .values()
            .filter(|s| now - s.last_activity <= timeout)
            .count()
    }

    /// Check if a session token exists and is valid without updating activity.
    ///
    /// Useful for read-only checks where you don't want to extend the session.
    pub fn is_session_valid(&self, token: &str) -> bool {
        let sessions = self.sessions.read().unwrap();
        match sessions.get(token) {
            Some(session) => {
                let now = Utc::now();
                let timeout = Duration::minutes(self.timeout_minutes as i64);
                now - session.last_activity <= timeout
            }
            None => false,
        }
    }

    /// Get the configured timeout in minutes.
    pub fn timeout_minutes(&self) -> u32 {
        self.timeout_minutes
    }
}

/// Generate a cryptographically random session token as a hex string.
fn generate_session_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..SESSION_TOKEN_BYTES).map(|_| rng.gen()).collect();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session_returns_valid_session() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);

        assert_eq!(session.user_id, user_id);
        assert!(!session.token.is_empty());
        assert_eq!(session.token.len(), SESSION_TOKEN_BYTES * 2); // hex encoding
    }

    #[test]
    fn test_create_session_generates_unique_tokens() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session1 = manager.create_session(user_id);
        let session2 = manager.create_session(user_id);

        assert_ne!(session1.token, session2.token);
        assert_ne!(session1.id, session2.id);
    }

    #[test]
    fn test_validate_session_succeeds_for_active_session() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        let result = manager.validate_session(&session.token);

        assert!(result.is_ok());
        let validated = result.unwrap();
        assert_eq!(validated.user_id, user_id);
    }

    #[test]
    fn test_validate_session_updates_last_activity() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        let original_activity = session.last_activity;

        // Small sleep to ensure time difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        let validated = manager.validate_session(&session.token).unwrap();
        assert!(validated.last_activity >= original_activity);
    }

    #[test]
    fn test_validate_session_fails_for_unknown_token() {
        let manager = SessionManager::new(30);

        let result = manager.validate_session("nonexistent-token");
        assert!(matches!(result, Err(SecurityError::SessionExpired)));
    }

    #[test]
    fn test_validate_session_fails_for_expired_session() {
        // Use a 0-minute timeout so sessions expire immediately.
        let manager = SessionManager::new(0);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);

        // Even with 0-minute timeout, the session was just created so
        // the duration check (now - last_activity > 0 minutes) needs
        // at least a tiny bit of time to pass.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let result = manager.validate_session(&session.token);
        assert!(matches!(result, Err(SecurityError::SessionExpired)));
    }

    #[test]
    fn test_invalidate_session_removes_session() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        assert!(manager.validate_session(&session.token).is_ok());

        manager.invalidate_session(&session.token).unwrap();
        assert!(manager.validate_session(&session.token).is_err());
    }

    #[test]
    fn test_invalidate_session_succeeds_for_unknown_token() {
        let manager = SessionManager::new(30);
        // Should not error even if token doesn't exist.
        assert!(manager.invalidate_session("nonexistent").is_ok());
    }

    #[test]
    fn test_cleanup_expired_removes_old_sessions() {
        let manager = SessionManager::new(0);
        let user_id = UserId::new();

        manager.create_session(user_id);
        manager.create_session(user_id);

        std::thread::sleep(std::time::Duration::from_millis(10));

        let removed = manager.cleanup_expired();
        assert_eq!(removed, 2);
    }

    #[test]
    fn test_cleanup_expired_keeps_active_sessions() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        manager.create_session(user_id);
        manager.create_session(user_id);

        let removed = manager.cleanup_expired();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_active_session_count() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        assert_eq!(manager.active_session_count(), 0);

        manager.create_session(user_id);
        assert_eq!(manager.active_session_count(), 1);

        manager.create_session(user_id);
        assert_eq!(manager.active_session_count(), 2);
    }

    #[test]
    fn test_is_session_valid_returns_true_for_active() {
        let manager = SessionManager::new(30);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        assert!(manager.is_session_valid(&session.token));
    }

    #[test]
    fn test_is_session_valid_returns_false_for_unknown() {
        let manager = SessionManager::new(30);
        assert!(!manager.is_session_valid("nonexistent"));
    }

    #[test]
    fn test_is_session_valid_returns_false_for_expired() {
        let manager = SessionManager::new(0);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(!manager.is_session_valid(&session.token));
    }

    #[test]
    fn test_session_token_is_hex_encoded() {
        let token = generate_session_token();
        // Should be valid hex
        assert!(hex::decode(&token).is_ok());
        assert_eq!(token.len(), SESSION_TOKEN_BYTES * 2);
    }

    #[test]
    fn test_uniform_error_for_expired_and_nonexistent() {
        let manager = SessionManager::new(0);
        let user_id = UserId::new();

        let session = manager.create_session(user_id);
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Both expired and nonexistent tokens return SessionExpired.
        let expired_result = manager.validate_session(&session.token);
        let nonexistent_result = manager.validate_session("does-not-exist");

        assert!(matches!(expired_result, Err(SecurityError::SessionExpired)));
        assert!(matches!(
            nonexistent_result,
            Err(SecurityError::SessionExpired)
        ));
    }

    #[test]
    fn test_timeout_minutes_getter() {
        let manager = SessionManager::new(45);
        assert_eq!(manager.timeout_minutes(), 45);
    }
}
