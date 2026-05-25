//! Messaging identity mapping and authorization.
//!
//! Maps platform user IDs (Telegram, Discord, WhatsApp) to operator accounts
//! and enforces the same RBAC rules as the Web Interface.
//!
//! Also provides credential validation (test connection on config save) and
//! delivery retry logic (up to 3 retries with 5s interval).

use std::collections::HashMap;
use std::time::Duration;

use common::models::Role;
use common::types::UserId;
use security_layer::rbac::{authorize, Permission};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::{
    IncomingMessage, MessageContent, MessageTarget, MessagingBot, MessagingError, Platform,
};

// =============================================================================
// Identity Mapper
// =============================================================================

/// A composite key for looking up platform user identities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlatformIdentity {
    pub platform: Platform,
    pub platform_user_id: String,
}

/// Mapping entry that associates a platform identity with an operator account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityMapping {
    pub user_id: UserId,
    pub role: Role,
}

/// Maps messaging platform user identities to platform operator accounts.
///
/// Stores (Platform, platform_user_id) -> (UserId, Role) mappings in memory
/// with serialization support for persistence.
///
/// Serialization uses a Vec of entries rather than a HashMap to avoid
/// JSON's requirement that map keys be strings.
#[derive(Debug, Clone)]
pub struct IdentityMapper {
    mappings: HashMap<PlatformIdentity, IdentityMapping>,
}

/// Serialization helper: represents a single identity mapping entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdentityEntry {
    platform: Platform,
    platform_user_id: String,
    user_id: UserId,
    role: Role,
}

impl Serialize for IdentityMapper {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let entries: Vec<IdentityEntry> = self
            .mappings
            .iter()
            .map(|(key, mapping)| IdentityEntry {
                platform: key.platform,
                platform_user_id: key.platform_user_id.clone(),
                user_id: mapping.user_id,
                role: mapping.role,
            })
            .collect();
        entries.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IdentityMapper {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let entries: Vec<IdentityEntry> = Vec::deserialize(deserializer)?;
        let mut mappings = HashMap::new();
        for entry in entries {
            let key = PlatformIdentity {
                platform: entry.platform,
                platform_user_id: entry.platform_user_id,
            };
            let mapping = IdentityMapping {
                user_id: entry.user_id,
                role: entry.role,
            };
            mappings.insert(key, mapping);
        }
        Ok(IdentityMapper { mappings })
    }
}

impl IdentityMapper {
    /// Create a new empty identity mapper.
    pub fn new() -> Self {
        Self {
            mappings: HashMap::new(),
        }
    }

    /// Add a mapping from a platform identity to an operator account.
    ///
    /// If a mapping already exists for this platform identity, it is replaced.
    pub fn add_mapping(
        &mut self,
        platform: Platform,
        platform_user_id: String,
        user_id: UserId,
        role: Role,
    ) {
        let key = PlatformIdentity {
            platform,
            platform_user_id,
        };
        let mapping = IdentityMapping { user_id, role };
        info!(
            "Added identity mapping: {:?} -> {:?}",
            key.platform, user_id
        );
        self.mappings.insert(key, mapping);
    }

    /// Remove a mapping for a platform identity.
    ///
    /// Returns the removed mapping if it existed.
    pub fn remove_mapping(
        &mut self,
        platform: Platform,
        platform_user_id: &str,
    ) -> Option<IdentityMapping> {
        let key = PlatformIdentity {
            platform,
            platform_user_id: platform_user_id.to_string(),
        };
        let removed = self.mappings.remove(&key);
        if removed.is_some() {
            info!(
                "Removed identity mapping for {:?}:{}",
                platform, platform_user_id
            );
        }
        removed
    }

    /// Look up the operator account for a platform identity.
    ///
    /// Returns None if no mapping exists.
    pub fn get_user_id(
        &self,
        platform: Platform,
        platform_user_id: &str,
    ) -> Option<&IdentityMapping> {
        let key = PlatformIdentity {
            platform,
            platform_user_id: platform_user_id.to_string(),
        };
        self.mappings.get(&key)
    }

    /// Returns the number of stored mappings.
    pub fn len(&self) -> usize {
        self.mappings.len()
    }

    /// Returns true if no mappings are stored.
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }
}

impl Default for IdentityMapper {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Authorization
// =============================================================================

/// Authorize an incoming message by mapping the platform user to an operator
/// account and checking RBAC permissions.
///
/// Enforces the same role-based authorization rules as the Web Interface:
/// - The platform user must be mapped to an operator account
/// - The mapped account must have the `SendMessages` permission
///
/// Returns the mapped UserId on success, or `MessagingError::Unauthorized`
/// if the user is not mapped or lacks permissions.
pub fn authorize_message(
    mapper: &IdentityMapper,
    incoming: &IncomingMessage,
) -> Result<UserId, MessagingError> {
    // Look up the platform user in the identity map
    let mapping = mapper
        .get_user_id(incoming.platform, &incoming.platform_user_id)
        .ok_or_else(|| {
            warn!(
                platform = ?incoming.platform,
                platform_user_id = %incoming.platform_user_id,
                "Message from unmapped user rejected"
            );
            MessagingError::Unauthorized(format!(
                "User '{}' on {:?} is not mapped to an authorized operator account",
                incoming.platform_user_id, incoming.platform
            ))
        })?;

    // Check RBAC permissions — messaging users need SendMessages permission
    authorize(&mapping.role, &Permission::SendMessages).map_err(|e| {
        warn!(
            user_id = %mapping.user_id,
            role = ?mapping.role,
            "Message from user with insufficient permissions rejected: {}",
            e
        );
        MessagingError::Unauthorized(format!(
            "User does not have permission to send messages: {}",
            e
        ))
    })?;

    Ok(mapping.user_id)
}

// =============================================================================
// Credential Validation
// =============================================================================

/// Validate messaging platform credentials by attempting a test connection.
///
/// This is called when configuration is saved to verify that the provided
/// credentials are valid before persisting them.
///
/// The `config` value should contain platform-specific credential fields:
/// - Telegram: `{ "bot_token": "..." }`
/// - Discord: `{ "bot_token": "..." }`
/// - WhatsApp: `{ "phone_number_id": "...", "access_token": "..." }`
///
/// Returns Ok(()) if the connection test succeeds, or an error describing
/// the failure.
pub async fn validate_credentials(
    platform: Platform,
    config: &serde_json::Value,
) -> Result<(), MessagingError> {
    match platform {
        Platform::Telegram => validate_telegram_credentials(config).await,
        Platform::Discord => validate_discord_credentials(config).await,
        Platform::WhatsApp => validate_whatsapp_credentials(config).await,
    }
}

async fn validate_telegram_credentials(config: &serde_json::Value) -> Result<(), MessagingError> {
    let bot_token = config
        .get("bot_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MessagingError::Config("Missing 'bot_token' in Telegram config".into()))?;

    if bot_token.is_empty() {
        return Err(MessagingError::Config(
            "Telegram bot_token cannot be empty".into(),
        ));
    }

    // Test connection by calling the Telegram Bot API getMe endpoint
    let url = format!("https://api.telegram.org/bot{}/getMe", bot_token);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| MessagingError::ConnectionFailed(format!("HTTP client error: {}", e)))?;

    let response = client.get(&url).send().await.map_err(|e| {
        MessagingError::ConnectionFailed(format!("Telegram connection failed: {}", e))
    })?;

    if !response.status().is_success() {
        return Err(MessagingError::AuthFailed(format!(
            "Telegram credential validation failed with status {}",
            response.status()
        )));
    }

    info!("Telegram credentials validated successfully");
    Ok(())
}

async fn validate_discord_credentials(config: &serde_json::Value) -> Result<(), MessagingError> {
    let bot_token = config
        .get("bot_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MessagingError::Config("Missing 'bot_token' in Discord config".into()))?;

    if bot_token.is_empty() {
        return Err(MessagingError::Config(
            "Discord bot_token cannot be empty".into(),
        ));
    }

    // Test connection by calling the Discord API /users/@me endpoint
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| MessagingError::ConnectionFailed(format!("HTTP client error: {}", e)))?;

    let response = client
        .get("https://discord.com/api/v10/users/@me")
        .header("Authorization", format!("Bot {}", bot_token))
        .send()
        .await
        .map_err(|e| {
            MessagingError::ConnectionFailed(format!("Discord connection failed: {}", e))
        })?;

    if !response.status().is_success() {
        return Err(MessagingError::AuthFailed(format!(
            "Discord credential validation failed with status {}",
            response.status()
        )));
    }

    info!("Discord credentials validated successfully");
    Ok(())
}

async fn validate_whatsapp_credentials(config: &serde_json::Value) -> Result<(), MessagingError> {
    let phone_number_id = config
        .get("phone_number_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            MessagingError::Config("Missing 'phone_number_id' in WhatsApp config".into())
        })?;

    let access_token = config
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            MessagingError::Config("Missing 'access_token' in WhatsApp config".into())
        })?;

    if phone_number_id.is_empty() || access_token.is_empty() {
        return Err(MessagingError::Config(
            "WhatsApp phone_number_id and access_token cannot be empty".into(),
        ));
    }

    // Test connection by calling the WhatsApp Business API
    let url = format!("https://graph.facebook.com/v18.0/{}", phone_number_id);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| MessagingError::ConnectionFailed(format!("HTTP client error: {}", e)))?;

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await
        .map_err(|e| {
            MessagingError::ConnectionFailed(format!("WhatsApp connection failed: {}", e))
        })?;

    if !response.status().is_success() {
        return Err(MessagingError::AuthFailed(format!(
            "WhatsApp credential validation failed with status {}",
            response.status()
        )));
    }

    info!("WhatsApp credentials validated successfully");
    Ok(())
}

// =============================================================================
// Delivery Retry
// =============================================================================

/// Send a message with retry logic.
///
/// Retries delivery up to `max_retries` times with the given `interval` between
/// attempts when the platform is unreachable. Logs each failure and discards
/// (returns error) after all retries are exhausted.
///
/// Per requirement 17.10: retry up to 3 times with 5s interval, log and discard
/// on failure.
pub async fn send_with_retry(
    bot: &dyn MessagingBot,
    target: &MessageTarget,
    content: MessageContent,
    max_retries: u32,
    interval: Duration,
) -> Result<(), MessagingError> {
    let mut last_error = None;

    for attempt in 0..=max_retries {
        match bot.send_message(target, content.clone()).await {
            Ok(()) => {
                if attempt > 0 {
                    info!(
                        platform = ?target.platform,
                        user_id = %target.user_id,
                        attempt = attempt + 1,
                        "Message delivered successfully after retry"
                    );
                }
                return Ok(());
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < max_retries {
                    warn!(
                        platform = ?target.platform,
                        user_id = %target.user_id,
                        attempt = attempt + 1,
                        max_retries = max_retries,
                        error = %last_error.as_ref().unwrap(),
                        "Message delivery failed, retrying in {:?}",
                        interval
                    );
                    tokio::time::sleep(interval).await;
                }
            }
        }
    }

    // All retries exhausted — log and discard
    let err = last_error.unwrap_or_else(|| MessagingError::SendFailed("Unknown error".into()));
    error!(
        platform = ?target.platform,
        user_id = %target.user_id,
        max_retries = max_retries,
        error = %err,
        "Message delivery failed after all retries, discarding"
    );

    Err(MessagingError::SendFailed(format!(
        "Delivery failed after {} retries: {}",
        max_retries, err
    )))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PlatformStatus;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// A mock bot for testing retry logic.
    struct MockBot {
        fail_count: Arc<AtomicU32>,
        platform: Platform,
    }

    impl MockBot {
        fn new(fail_count: u32) -> Self {
            Self {
                fail_count: Arc::new(AtomicU32::new(fail_count)),
                platform: Platform::Telegram,
            }
        }
    }

    #[async_trait]
    impl MessagingBot for MockBot {
        async fn start(&self) -> Result<(), MessagingError> {
            Ok(())
        }

        async fn stop(&self) -> Result<(), MessagingError> {
            Ok(())
        }

        async fn send_message(
            &self,
            _target: &MessageTarget,
            _content: MessageContent,
        ) -> Result<(), MessagingError> {
            let remaining = self.fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_count.fetch_sub(1, Ordering::SeqCst);
                Err(MessagingError::ConnectionFailed(
                    "Platform unreachable".into(),
                ))
            } else {
                Ok(())
            }
        }

        fn status(&self) -> PlatformStatus {
            PlatformStatus {
                platform: self.platform,
                connected: true,
                last_message_at: None,
                error: None,
            }
        }

        fn platform(&self) -> Platform {
            self.platform
        }
    }

    #[test]
    fn test_identity_mapper_add_and_get() {
        let mut mapper = IdentityMapper::new();
        let user_id = UserId::new();

        mapper.add_mapping(
            Platform::Telegram,
            "12345".to_string(),
            user_id,
            Role::Operator,
        );

        let result = mapper.get_user_id(Platform::Telegram, "12345");
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.user_id, user_id);
        assert_eq!(mapping.role, Role::Operator);
    }

    #[test]
    fn test_identity_mapper_get_nonexistent() {
        let mapper = IdentityMapper::new();
        let result = mapper.get_user_id(Platform::Discord, "99999");
        assert!(result.is_none());
    }

    #[test]
    fn test_identity_mapper_remove() {
        let mut mapper = IdentityMapper::new();
        let user_id = UserId::new();

        mapper.add_mapping(Platform::Discord, "abc".to_string(), user_id, Role::Admin);
        assert_eq!(mapper.len(), 1);

        let removed = mapper.remove_mapping(Platform::Discord, "abc");
        assert!(removed.is_some());
        assert_eq!(mapper.len(), 0);
        assert!(mapper.get_user_id(Platform::Discord, "abc").is_none());
    }

    #[test]
    fn test_identity_mapper_replace_existing() {
        let mut mapper = IdentityMapper::new();
        let user_id_1 = UserId::new();
        let user_id_2 = UserId::new();

        mapper.add_mapping(
            Platform::Telegram,
            "123".to_string(),
            user_id_1,
            Role::Operator,
        );
        mapper.add_mapping(
            Platform::Telegram,
            "123".to_string(),
            user_id_2,
            Role::Admin,
        );

        let result = mapper.get_user_id(Platform::Telegram, "123").unwrap();
        assert_eq!(result.user_id, user_id_2);
        assert_eq!(result.role, Role::Admin);
        assert_eq!(mapper.len(), 1);
    }

    #[test]
    fn test_identity_mapper_different_platforms_same_id() {
        let mut mapper = IdentityMapper::new();
        let user_id_1 = UserId::new();
        let user_id_2 = UserId::new();

        mapper.add_mapping(
            Platform::Telegram,
            "123".to_string(),
            user_id_1,
            Role::Operator,
        );
        mapper.add_mapping(Platform::Discord, "123".to_string(), user_id_2, Role::Admin);

        assert_eq!(mapper.len(), 2);
        assert_eq!(
            mapper
                .get_user_id(Platform::Telegram, "123")
                .unwrap()
                .user_id,
            user_id_1
        );
        assert_eq!(
            mapper
                .get_user_id(Platform::Discord, "123")
                .unwrap()
                .user_id,
            user_id_2
        );
    }

    #[test]
    fn test_authorize_message_unmapped_user() {
        let mapper = IdentityMapper::new();
        let incoming = IncomingMessage {
            platform: Platform::Telegram,
            platform_user_id: "unknown_user".to_string(),
            channel_id: None,
            content: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            attachments: Vec::new(),
        };

        let result = authorize_message(&mapper, &incoming);
        assert!(result.is_err());
        match result.unwrap_err() {
            MessagingError::Unauthorized(msg) => {
                assert!(msg.contains("not mapped"));
            }
            other => panic!("Expected Unauthorized, got: {:?}", other),
        }
    }

    #[test]
    fn test_authorize_message_mapped_operator() {
        let mut mapper = IdentityMapper::new();
        let user_id = UserId::new();
        mapper.add_mapping(
            Platform::Discord,
            "user123".to_string(),
            user_id,
            Role::Operator,
        );

        let incoming = IncomingMessage {
            platform: Platform::Discord,
            platform_user_id: "user123".to_string(),
            channel_id: None,
            content: "hello".to_string(),
            timestamp: chrono::Utc::now(),
            attachments: Vec::new(),
        };

        let result = authorize_message(&mapper, &incoming);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), user_id);
    }

    #[test]
    fn test_authorize_message_mapped_admin() {
        let mut mapper = IdentityMapper::new();
        let user_id = UserId::new();
        mapper.add_mapping(
            Platform::WhatsApp,
            "+1234567890".to_string(),
            user_id,
            Role::Admin,
        );

        let incoming = IncomingMessage {
            platform: Platform::WhatsApp,
            platform_user_id: "+1234567890".to_string(),
            channel_id: None,
            content: "!status".to_string(),
            timestamp: chrono::Utc::now(),
            attachments: Vec::new(),
        };

        let result = authorize_message(&mapper, &incoming);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), user_id);
    }

    #[tokio::test]
    async fn test_send_with_retry_immediate_success() {
        let bot = MockBot::new(0); // No failures
        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "123".to_string(),
            channel_id: None,
        };
        let content = MessageContent::Text("hello".to_string());

        let result = send_with_retry(&bot, &target, content, 3, Duration::from_millis(10)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_with_retry_succeeds_after_retries() {
        let bot = MockBot::new(2); // Fail twice, then succeed
        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "123".to_string(),
            channel_id: None,
        };
        let content = MessageContent::Text("hello".to_string());

        let result = send_with_retry(&bot, &target, content, 3, Duration::from_millis(10)).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_send_with_retry_exhausts_retries() {
        let bot = MockBot::new(5); // Fail more times than retries allow
        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "123".to_string(),
            channel_id: None,
        };
        let content = MessageContent::Text("hello".to_string());

        let result = send_with_retry(&bot, &target, content, 3, Duration::from_millis(10)).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            MessagingError::SendFailed(msg) => {
                assert!(msg.contains("failed after 3 retries"));
            }
            other => panic!("Expected SendFailed, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_send_with_retry_zero_retries() {
        let bot = MockBot::new(1); // Fail once
        let target = MessageTarget {
            platform: Platform::Telegram,
            user_id: "123".to_string(),
            channel_id: None,
        };
        let content = MessageContent::Text("hello".to_string());

        // With 0 retries, only the initial attempt is made
        let result = send_with_retry(&bot, &target, content, 0, Duration::from_millis(10)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_identity_mapper_serialization() {
        let mut mapper = IdentityMapper::new();
        let user_id = UserId::new();
        mapper.add_mapping(
            Platform::Telegram,
            "12345".to_string(),
            user_id,
            Role::Operator,
        );

        // Serialize
        let json = serde_json::to_string(&mapper).unwrap();
        // Deserialize
        let restored: IdentityMapper = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.len(), 1);
        let mapping = restored.get_user_id(Platform::Telegram, "12345").unwrap();
        assert_eq!(mapping.user_id, user_id);
        assert_eq!(mapping.role, Role::Operator);
    }
}
