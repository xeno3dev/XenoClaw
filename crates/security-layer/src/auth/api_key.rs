//! API key authentication.
//!
//! API keys must be at least 32 characters long and are stored as SHA-256 hashes.
//! Validation always returns the same error type regardless of whether the key
//! exists or is invalid, to prevent enumeration attacks.

use sha2::{Digest, Sha256};

use common::errors::SecurityError;
use common::models::ApiKey;
use common::types::ApiKeyId;

/// Minimum required length for API keys.
pub const MIN_API_KEY_LENGTH: usize = 32;

/// Handles API key generation, hashing, and validation.
#[derive(Debug, Clone)]
pub struct ApiKeyAuthenticator;

impl ApiKeyAuthenticator {
    /// Create a new API key authenticator.
    pub fn new() -> Self {
        Self
    }

    /// Validate that a raw API key meets the minimum length requirement.
    ///
    /// Returns `Err(SecurityError::InvalidCredentials)` if the key is too short.
    /// The error intentionally does not reveal *why* validation failed.
    pub fn validate_key_format(&self, raw_key: &str) -> Result<(), SecurityError> {
        if raw_key.len() < MIN_API_KEY_LENGTH {
            return Err(SecurityError::InvalidCredentials);
        }
        Ok(())
    }

    /// Hash a raw API key using SHA-256 for storage.
    ///
    /// API keys are hashed (not encrypted) because they are high-entropy
    /// random strings that don't need bcrypt's slow hashing.
    pub fn hash_key(&self, raw_key: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Generate a new random API key of the specified length.
    ///
    /// The returned key is a hex-encoded random string. The minimum length
    /// is enforced to be at least `MIN_API_KEY_LENGTH`.
    pub fn generate_key(&self, length: usize) -> String {
        use rand::Rng;

        let effective_length = length.max(MIN_API_KEY_LENGTH);
        // Generate enough random bytes to produce the desired hex string length.
        // Each byte produces 2 hex characters.
        let byte_count = (effective_length + 1) / 2;
        let mut rng = rand::thread_rng();
        let bytes: Vec<u8> = (0..byte_count).map(|_| rng.gen()).collect();
        let hex_str = hex::encode(bytes);
        hex_str[..effective_length].to_string()
    }

    /// Verify a raw API key against a stored hash.
    ///
    /// Returns `Ok(())` if the key matches, or `Err(SecurityError::InvalidCredentials)`
    /// if it doesn't. The error is uniform to prevent timing-based enumeration.
    pub fn verify_key(&self, raw_key: &str, stored_hash: &str) -> Result<(), SecurityError> {
        let computed_hash = self.hash_key(raw_key);
        // Use constant-time comparison to prevent timing attacks.
        if constant_time_eq(computed_hash.as_bytes(), stored_hash.as_bytes()) {
            Ok(())
        } else {
            Err(SecurityError::InvalidCredentials)
        }
    }

    /// Authenticate a raw API key against a list of stored keys.
    ///
    /// Returns the matching `ApiKeyId` on success, or a uniform
    /// `SecurityError::InvalidCredentials` on failure — never revealing
    /// whether the key exists or is simply invalid.
    pub fn authenticate(
        &self,
        raw_key: &str,
        stored_keys: &[ApiKey],
    ) -> Result<ApiKeyId, SecurityError> {
        // First validate format — but still return the same error type.
        if raw_key.len() < MIN_API_KEY_LENGTH {
            return Err(SecurityError::InvalidCredentials);
        }

        let computed_hash = self.hash_key(raw_key);

        // Check all keys to avoid timing leaks about which position matched.
        let mut matched_id: Option<ApiKeyId> = None;
        for key in stored_keys {
            if constant_time_eq(computed_hash.as_bytes(), key.key_hash.as_bytes()) {
                matched_id = Some(key.id);
            }
        }

        matched_id.ok_or(SecurityError::InvalidCredentials)
    }
}

impl Default for ApiKeyAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_validate_key_format_accepts_valid_key() {
        let auth = ApiKeyAuthenticator::new();
        let key = "a".repeat(32);
        assert!(auth.validate_key_format(&key).is_ok());
    }

    #[test]
    fn test_validate_key_format_accepts_long_key() {
        let auth = ApiKeyAuthenticator::new();
        let key = "x".repeat(64);
        assert!(auth.validate_key_format(&key).is_ok());
    }

    #[test]
    fn test_validate_key_format_rejects_short_key() {
        let auth = ApiKeyAuthenticator::new();
        let key = "a".repeat(31);
        let result = auth.validate_key_format(&key);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_validate_key_format_rejects_empty_key() {
        let auth = ApiKeyAuthenticator::new();
        let result = auth.validate_key_format("");
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_hash_key_produces_consistent_output() {
        let auth = ApiKeyAuthenticator::new();
        let key = "test-api-key-that-is-long-enough-for-validation";
        let hash1 = auth.hash_key(key);
        let hash2 = auth.hash_key(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_key_produces_different_output_for_different_keys() {
        let auth = ApiKeyAuthenticator::new();
        let hash1 = auth.hash_key("key-one-that-is-long-enough-12345");
        let hash2 = auth.hash_key("key-two-that-is-long-enough-12345");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_generate_key_meets_minimum_length() {
        let auth = ApiKeyAuthenticator::new();
        let key = auth.generate_key(32);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_generate_key_enforces_minimum_when_requested_shorter() {
        let auth = ApiKeyAuthenticator::new();
        let key = auth.generate_key(10);
        assert_eq!(key.len(), MIN_API_KEY_LENGTH);
    }

    #[test]
    fn test_generate_key_produces_unique_keys() {
        let auth = ApiKeyAuthenticator::new();
        let key1 = auth.generate_key(32);
        let key2 = auth.generate_key(32);
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_verify_key_succeeds_with_correct_key() {
        let auth = ApiKeyAuthenticator::new();
        let raw_key = "my-secret-api-key-that-is-long-enough";
        let hash = auth.hash_key(raw_key);
        assert!(auth.verify_key(raw_key, &hash).is_ok());
    }

    #[test]
    fn test_verify_key_fails_with_wrong_key() {
        let auth = ApiKeyAuthenticator::new();
        let raw_key = "my-secret-api-key-that-is-long-enough";
        let hash = auth.hash_key(raw_key);
        let result = auth.verify_key("wrong-key-that-is-also-long-enough", &hash);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_authenticate_succeeds_with_matching_key() {
        let auth = ApiKeyAuthenticator::new();
        let raw_key = auth.generate_key(48);
        let key_hash = auth.hash_key(&raw_key);
        let api_key = ApiKey {
            id: ApiKeyId::new(),
            key_hash,
            name: "test-key".to_string(),
            rate_limit: 100,
            created_at: Utc::now(),
            last_used: None,
        };

        let result = auth.authenticate(&raw_key, &[api_key.clone()]);
        assert_eq!(result.unwrap(), api_key.id);
    }

    #[test]
    fn test_authenticate_fails_with_no_matching_key() {
        let auth = ApiKeyAuthenticator::new();
        let raw_key = auth.generate_key(48);
        let api_key = ApiKey {
            id: ApiKeyId::new(),
            key_hash: auth.hash_key("different-key-that-is-long-enough-too"),
            name: "test-key".to_string(),
            rate_limit: 100,
            created_at: Utc::now(),
            last_used: None,
        };

        let result = auth.authenticate(&raw_key, &[api_key]);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_authenticate_fails_with_short_key() {
        let auth = ApiKeyAuthenticator::new();
        let short_key = "too-short";
        let api_key = ApiKey {
            id: ApiKeyId::new(),
            key_hash: auth.hash_key(short_key),
            name: "test-key".to_string(),
            rate_limit: 100,
            created_at: Utc::now(),
            last_used: None,
        };

        let result = auth.authenticate(short_key, &[api_key]);
        assert!(matches!(result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_uniform_error_for_nonexistent_key() {
        let auth = ApiKeyAuthenticator::new();
        // Both a short key and a valid-length but wrong key should return
        // the same error variant — InvalidCredentials.
        let short_result = auth.authenticate("short", &[]);
        let wrong_result = auth.authenticate(&"x".repeat(48), &[]);

        assert!(matches!(short_result, Err(SecurityError::InvalidCredentials)));
        assert!(matches!(wrong_result, Err(SecurityError::InvalidCredentials)));
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
    }
}
