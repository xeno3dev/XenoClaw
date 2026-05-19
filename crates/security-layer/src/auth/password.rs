//! Password authentication using bcrypt.
//!
//! Passwords must be at least 12 characters long and are hashed using bcrypt
//! with a configurable cost factor. Authentication failures always return
//! the same error type to prevent user enumeration.

use common::errors::SecurityError;
use common::models::User;
use common::types::UserId;

/// Minimum required password length.
pub const MIN_PASSWORD_LENGTH: usize = 12;

/// Default bcrypt cost factor. 12 is a good balance between security and speed.
const DEFAULT_BCRYPT_COST: u32 = 12;

/// Handles password hashing and verification using bcrypt.
#[derive(Debug, Clone)]
pub struct PasswordAuthenticator {
    /// The bcrypt cost factor (higher = slower but more secure).
    cost: u32,
}

impl PasswordAuthenticator {
    /// Create a new password authenticator with the default cost factor.
    pub fn new() -> Self {
        Self {
            cost: DEFAULT_BCRYPT_COST,
        }
    }

    /// Create a new password authenticator with a custom cost factor.
    ///
    /// Cost must be between 4 and 31 (bcrypt limits).
    pub fn with_cost(cost: u32) -> Self {
        let cost = cost.clamp(4, 31);
        Self { cost }
    }

    /// Validate that a password meets the minimum length requirement.
    ///
    /// Returns `Err(SecurityError::InvalidCredentials)` if the password is too short.
    /// The error intentionally does not reveal the specific validation failure.
    pub fn validate_password_format(&self, password: &str) -> Result<(), SecurityError> {
        if password.len() < MIN_PASSWORD_LENGTH {
            return Err(SecurityError::InvalidCredentials);
        }
        Ok(())
    }

    /// Hash a password using bcrypt.
    ///
    /// Returns the bcrypt hash string suitable for storage.
    /// Returns an error if hashing fails (should not happen with valid input).
    pub fn hash_password(&self, password: &str) -> Result<String, SecurityError> {
        bcrypt::hash(password, self.cost).map_err(|_| SecurityError::InvalidCredentials)
    }

    /// Verify a password against a stored bcrypt hash.
    ///
    /// Returns `Ok(())` if the password matches, or `Err(SecurityError::InvalidCredentials)`
    /// if it doesn't. The error is uniform to prevent enumeration.
    pub fn verify_password(&self, password: &str, hash: &str) -> Result<(), SecurityError> {
        match bcrypt::verify(password, hash) {
            Ok(true) => Ok(()),
            Ok(false) => Err(SecurityError::InvalidCredentials),
            Err(_) => Err(SecurityError::InvalidCredentials),
        }
    }

    /// Authenticate a username/password pair against a list of users.
    ///
    /// Returns the matching `UserId` on success, or a uniform
    /// `SecurityError::InvalidCredentials` on failure — never revealing
    /// whether the username exists or the password is wrong.
    pub fn authenticate(
        &self,
        username: &str,
        password: &str,
        users: &[User],
    ) -> Result<UserId, SecurityError> {
        // Validate password format first, but return the same error.
        if password.len() < MIN_PASSWORD_LENGTH {
            // Still do a dummy bcrypt verify to prevent timing attacks
            // that could reveal whether the password was too short vs wrong.
            let _ = bcrypt::verify(
                password,
                "$2b$12$000000000000000000000uGTWvMPqHJB2LTnGOEHOV0bMFfGPkkS",
            );
            return Err(SecurityError::InvalidCredentials);
        }

        // Find the user by username.
        let user = users.iter().find(|u| u.username == username);

        match user {
            Some(u) => {
                // Verify the password against the stored hash.
                self.verify_password(password, &u.password_hash)?;
                Ok(u.id)
            }
            None => {
                // User not found — do a dummy bcrypt verify to prevent timing
                // attacks that could reveal whether the username exists.
                let _ = bcrypt::verify(
                    password,
                    "$2b$12$000000000000000000000uGTWvMPqHJB2LTnGOEHOV0bMFfGPkkS",
                );
                Err(SecurityError::InvalidCredentials)
            }
        }
    }
}

impl Default for PasswordAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::models::Role;

    fn make_test_user(username: &str, password: &str) -> User {
        let auth = PasswordAuthenticator::with_cost(4); // Low cost for tests
        User {
            id: UserId::new(),
            username: username.to_string(),
            password_hash: auth.hash_password(password).unwrap(),
            role: Role::Operator,
            api_keys: vec![],
            messaging_identities: vec![],
            created_at: Utc::now(),
            last_login: None,
        }
    }

    #[test]
    fn test_validate_password_format_accepts_valid() {
        let auth = PasswordAuthenticator::new();
        let password = "a".repeat(12);
        assert!(auth.validate_password_format(&password).is_ok());
    }

    #[test]
    fn test_validate_password_format_accepts_long() {
        let auth = PasswordAuthenticator::new();
        let password = "x".repeat(100);
        assert!(auth.validate_password_format(&password).is_ok());
    }

    #[test]
    fn test_validate_password_format_rejects_short() {
        let auth = PasswordAuthenticator::new();
        let password = "a".repeat(11);
        let result = auth.validate_password_format(&password);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_validate_password_format_rejects_empty() {
        let auth = PasswordAuthenticator::new();
        let result = auth.validate_password_format("");
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_hash_password_produces_valid_bcrypt() {
        let auth = PasswordAuthenticator::with_cost(4);
        let hash = auth.hash_password("test-password!").unwrap();
        // bcrypt hashes start with $2b$ (or $2a$)
        assert!(hash.starts_with("$2b$") || hash.starts_with("$2a$"));
    }

    #[test]
    fn test_hash_password_produces_different_hashes() {
        let auth = PasswordAuthenticator::with_cost(4);
        let hash1 = auth.hash_password("same-password!").unwrap();
        let hash2 = auth.hash_password("same-password!").unwrap();
        // bcrypt uses random salt, so same password produces different hashes.
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_verify_password_succeeds_with_correct_password() {
        let auth = PasswordAuthenticator::with_cost(4);
        let password = "my-secure-password-123";
        let hash = auth.hash_password(password).unwrap();
        assert!(auth.verify_password(password, &hash).is_ok());
    }

    #[test]
    fn test_verify_password_fails_with_wrong_password() {
        let auth = PasswordAuthenticator::with_cost(4);
        let hash = auth.hash_password("correct-password").unwrap();
        let result = auth.verify_password("wrong-password!!", &hash);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_verify_password_fails_with_invalid_hash() {
        let auth = PasswordAuthenticator::with_cost(4);
        let result = auth.verify_password("any-password-here", "not-a-valid-hash");
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_authenticate_succeeds_with_valid_credentials() {
        let auth = PasswordAuthenticator::with_cost(4);
        let password = "valid-password-123";
        let user = make_test_user("alice", password);

        let result = auth.authenticate("alice", password, &[user.clone()]);
        assert_eq!(result.unwrap(), user.id);
    }

    #[test]
    fn test_authenticate_fails_with_wrong_password() {
        let auth = PasswordAuthenticator::with_cost(4);
        let user = make_test_user("alice", "correct-password!");

        let result = auth.authenticate("alice", "wrong-password!!", &[user]);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_authenticate_fails_with_nonexistent_user() {
        let auth = PasswordAuthenticator::with_cost(4);
        let user = make_test_user("alice", "some-password-123");

        let result = auth.authenticate("bob", "some-password-123", &[user]);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_authenticate_fails_with_short_password() {
        let auth = PasswordAuthenticator::with_cost(4);
        let user = make_test_user("alice", "valid-password-123");

        let result = auth.authenticate("alice", "short", &[user]);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_uniform_error_for_all_failure_modes() {
        let auth = PasswordAuthenticator::with_cost(4);
        let user = make_test_user("alice", "valid-password-123");
        let users = vec![user];

        // Wrong user, wrong password, short password — all return InvalidCredentials.
        let r1 = auth.authenticate("bob", "valid-password-123", &users);
        let r2 = auth.authenticate("alice", "wrong-password!!", &users);
        let r3 = auth.authenticate("alice", "short", &users);

        assert!(matches!(r1, Err(SecurityError::InvalidCredentials)));
        assert!(matches!(r2, Err(SecurityError::InvalidCredentials)));
        assert!(matches!(r3, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_with_cost_clamps_to_valid_range() {
        // Cost below 4 should be clamped to 4
        let auth = PasswordAuthenticator::with_cost(1);
        assert_eq!(auth.cost, 4);

        // Cost above 31 should be clamped to 31
        let auth = PasswordAuthenticator::with_cost(50);
        assert_eq!(auth.cost, 31);
    }
}
