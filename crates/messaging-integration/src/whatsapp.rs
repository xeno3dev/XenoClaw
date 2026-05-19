//! WhatsApp bot integration using the WhatsApp Business Cloud API.
//!
//! This module implements the `MessagingBot` trait for WhatsApp using
//! HTTP-based communication with Meta's WhatsApp Business Cloud API.
//! It supports:
//! - Webhook verification (GET challenge-response)
//! - Incoming message processing via webhook (POST)
//! - Sending text and media messages
//! - Management commands (!status, !tasks, !mode, !schedule, !config)
//! - Diff image delivery as media attachments
//! - 30-second response timeout enforcement

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::{
    IncomingMessage, ManagementCommand, MessageContent, MessageTarget,
    MessagingBot, MessagingError, Platform, PlatformStatus,
};

/// Maximum time allowed to process and respond to a message.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retries for message delivery when platform is unreachable.
const MAX_DELIVERY_RETRIES: u32 = 3;

/// Interval between delivery retries.
const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// WhatsApp Business Cloud API base URL.
const WHATSAPP_API_BASE: &str = "https://graph.facebook.com/v18.0";

// ─── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the WhatsApp bot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    /// The Phone Number ID from WhatsApp Business API.
    pub phone_number_id: String,
    /// Permanent access token for the WhatsApp Business API.
    pub access_token: String,
    /// Verify token used for webhook verification handshake.
    pub verify_token: String,
    /// The webhook URL where WhatsApp sends events (for reference).
    pub webhook_url: String,
    /// App secret for payload signature verification (optional).
    pub app_secret: Option<String>,
}

// ─── WhatsApp API Request/Response Types ─────────────────────────────────────

/// Webhook verification query parameters (GET request from WhatsApp).
#[derive(Debug, Deserialize)]
pub struct WebhookVerifyQuery {
    #[serde(rename = "hub.mode")]
    pub mode: String,
    #[serde(rename = "hub.verify_token")]
    pub verify_token: String,
    #[serde(rename = "hub.challenge")]
    pub challenge: String,
}

/// Top-level webhook payload from WhatsApp (POST).
#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    pub object: String,
    pub entry: Vec<WebhookEntry>,
}

/// A single entry in the webhook payload.
#[derive(Debug, Deserialize)]
pub struct WebhookEntry {
    pub id: String,
    pub changes: Vec<WebhookChange>,
}

/// A change within a webhook entry.
#[derive(Debug, Deserialize)]
pub struct WebhookChange {
    pub field: String,
    pub value: WebhookValue,
}

/// The value object within a webhook change.
#[derive(Debug, Deserialize)]
pub struct WebhookValue {
    pub messaging_product: Option<String>,
    pub metadata: Option<WebhookMetadata>,
    pub contacts: Option<Vec<WebhookContact>>,
    pub messages: Option<Vec<WebhookMessage>>,
    pub statuses: Option<Vec<WebhookStatus>>,
}

/// Metadata about the receiving phone number.
#[derive(Debug, Deserialize)]
pub struct WebhookMetadata {
    pub display_phone_number: Option<String>,
    pub phone_number_id: Option<String>,
}

/// Contact information from a webhook message.
#[derive(Debug, Deserialize)]
pub struct WebhookContact {
    pub profile: Option<WebhookProfile>,
    pub wa_id: String,
}

/// Profile information for a contact.
#[derive(Debug, Deserialize)]
pub struct WebhookProfile {
    pub name: Option<String>,
}

/// An incoming message from the webhook.
#[derive(Debug, Deserialize)]
pub struct WebhookMessage {
    pub from: String,
    pub id: String,
    pub timestamp: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub text: Option<WebhookTextBody>,
}

/// Text body of a webhook message.
#[derive(Debug, Deserialize)]
pub struct WebhookTextBody {
    pub body: String,
}

/// Message delivery status from webhook.
#[derive(Debug, Deserialize)]
pub struct WebhookStatus {
    pub id: String,
    pub status: String,
    pub timestamp: String,
    pub recipient_id: String,
}

// ─── Outgoing message types ──────────────────────────────────────────────────

/// Request body for sending a text message via the Cloud API.
#[derive(Debug, Serialize)]
struct SendTextRequest {
    messaging_product: &'static str,
    to: String,
    #[serde(rename = "type")]
    message_type: &'static str,
    text: TextBody,
}

/// Text body for outgoing messages.
#[derive(Debug, Serialize)]
struct TextBody {
    body: String,
}

/// Request body for sending a media (image) message.
#[derive(Debug, Serialize)]
struct SendImageRequest {
    messaging_product: &'static str,
    to: String,
    #[serde(rename = "type")]
    message_type: &'static str,
    image: ImageBody,
}

/// Image body for outgoing media messages.
#[derive(Debug, Serialize)]
struct ImageBody {
    /// Media ID (after uploading to WhatsApp).
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    caption: Option<String>,
}

/// Response from the media upload endpoint.
#[derive(Debug, Deserialize)]
struct MediaUploadResponse {
    id: String,
}

// ─── WhatsApp Bot Implementation ─────────────────────────────────────────────

/// Internal state for the WhatsApp bot.
#[derive(Debug)]
struct WhatsAppState {
    connected: bool,
    last_message_at: Option<chrono::DateTime<Utc>>,
    last_error: Option<String>,
}

/// WhatsApp bot that communicates via the WhatsApp Business Cloud API.
///
/// This bot uses HTTP requests to send messages and expects webhook
/// events to be forwarded to it for processing incoming messages.
pub struct WhatsAppBot {
    config: WhatsAppConfig,
    client: Client,
    state: Arc<RwLock<WhatsAppState>>,
}

impl WhatsAppBot {
    /// Create a new WhatsApp bot with the given configuration.
    pub fn new(config: WhatsAppConfig) -> Result<Self, MessagingError> {
        if config.phone_number_id.is_empty() {
            return Err(MessagingError::Config(
                "phone_number_id is required".to_string(),
            ));
        }
        if config.access_token.is_empty() {
            return Err(MessagingError::Config(
                "access_token is required".to_string(),
            ));
        }
        if config.verify_token.is_empty() {
            return Err(MessagingError::Config(
                "verify_token is required".to_string(),
            ));
        }

        let client = Client::builder()
            .timeout(RESPONSE_TIMEOUT)
            .build()
            .map_err(|e| {
                MessagingError::Config(format!("Failed to create HTTP client: {e}"))
            })?;

        Ok(Self {
            config,
            client,
            state: Arc::new(RwLock::new(WhatsAppState {
                connected: false,
                last_message_at: None,
                last_error: None,
            })),
        })
    }

    /// Verify a webhook subscription request from WhatsApp.
    ///
    /// WhatsApp sends a GET request with hub.mode, hub.verify_token,
    /// and hub.challenge. If the verify_token matches, return the challenge.
    pub fn verify_webhook(
        &self,
        query: &WebhookVerifyQuery,
    ) -> Result<String, MessagingError> {
        if query.mode != "subscribe" {
            return Err(MessagingError::Platform(format!(
                "Unexpected hub.mode: {}",
                query.mode
            )));
        }

        if query.verify_token != self.config.verify_token {
            warn!("Webhook verification failed: token mismatch");
            return Err(MessagingError::AuthFailed(
                "Verify token mismatch".to_string(),
            ));
        }

        info!("Webhook verification successful");
        Ok(query.challenge.clone())
    }

    /// Process an incoming webhook payload and extract messages.
    pub async fn process_webhook(
        &self,
        payload: WebhookPayload,
    ) -> Result<Vec<IncomingMessage>, MessagingError> {
        if payload.object != "whatsapp_business_account" {
            return Err(MessagingError::Platform(format!(
                "Unexpected object type: {}",
                payload.object
            )));
        }

        let mut messages = Vec::new();

        for entry in &payload.entry {
            for change in &entry.changes {
                if change.field != "messages" {
                    continue;
                }
                if let Some(ref webhook_messages) = change.value.messages {
                    for msg in webhook_messages {
                        if msg.message_type == "text" {
                            if let Some(ref text_body) = msg.text {
                                let incoming = IncomingMessage {
                                    platform: Platform::WhatsApp,
                                    platform_user_id: msg.from.clone(),
                                    channel_id: None,
                                    content: text_body.body.clone(),
                                    timestamp: Utc::now(),
                                };
                                messages.push(incoming);
                            }
                        }
                    }
                }
            }
        }

        // Update state with last message time
        if !messages.is_empty() {
            let mut state = self.state.write().await;
            state.last_message_at = Some(Utc::now());
            state.connected = true;
            state.last_error = None;
        }

        debug!("Processed {} incoming WhatsApp messages", messages.len());
        Ok(messages)
    }

    /// Parse a management command from message text.
    pub fn parse_command(text: &str) -> Option<ManagementCommand> {
        ManagementCommand::parse(text)
    }

    /// Send a text message to a WhatsApp user.
    async fn send_text(
        &self,
        to: &str,
        text: &str,
    ) -> Result<(), MessagingError> {
        let url = format!(
            "{}/{}/messages",
            WHATSAPP_API_BASE, self.config.phone_number_id
        );

        let request_body = SendTextRequest {
            messaging_product: "whatsapp",
            to: to.to_string(),
            message_type: "text",
            text: TextBody {
                body: text.to_string(),
            },
        };

        self.send_with_retry(&url, &request_body).await
    }

    /// Upload media (image) to WhatsApp and return the media ID.
    async fn upload_media(&self, data: &[u8]) -> Result<String, MessagingError> {
        let url = format!(
            "{}/{}/media",
            WHATSAPP_API_BASE, self.config.phone_number_id
        );

        let form = reqwest::multipart::Form::new()
            .text("messaging_product", "whatsapp")
            .text("type", "image/png")
            .part(
                "file",
                reqwest::multipart::Part::bytes(data.to_vec())
                    .file_name("diff.png")
                    .mime_str("image/png")
                    .map_err(|e| {
                        MessagingError::SendFailed(e.to_string())
                    })?,
            );

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                MessagingError::SendFailed(format!("Media upload failed: {e}"))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MessagingError::SendFailed(format!(
                "Upload failed with status {status}: {body}"
            )));
        }

        let upload_response: MediaUploadResponse = response
            .json()
            .await
            .map_err(|e| {
                MessagingError::SendFailed(format!(
                    "Failed to parse upload response: {e}"
                ))
            })?;

        Ok(upload_response.id)
    }

    /// Send an image message using a previously uploaded media ID.
    async fn send_image(
        &self,
        to: &str,
        media_id: &str,
        caption: Option<&str>,
    ) -> Result<(), MessagingError> {
        let url = format!(
            "{}/{}/messages",
            WHATSAPP_API_BASE, self.config.phone_number_id
        );

        let request_body = SendImageRequest {
            messaging_product: "whatsapp",
            to: to.to_string(),
            message_type: "image",
            image: ImageBody {
                id: media_id.to_string(),
                caption: caption.map(|s| s.to_string()),
            },
        };

        self.send_with_retry(&url, &request_body).await
    }

    /// Send an HTTP request with retry logic.
    ///
    /// Retries up to MAX_DELIVERY_RETRIES times with RETRY_INTERVAL
    /// between attempts.
    async fn send_with_retry<T: Serialize>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<(), MessagingError> {
        let mut last_error = None;

        for attempt in 1..=MAX_DELIVERY_RETRIES {
            match self
                .client
                .post(url)
                .bearer_auth(&self.config.access_token)
                .json(body)
                .send()
                .await
            {
                Ok(response) => {
                    if response.status().is_success() {
                        debug!("Message sent on attempt {attempt}");
                        return Ok(());
                    }
                    let status = response.status();
                    let err_body = response.text().await.unwrap_or_default();
                    let err_msg = format!("API returned {status}: {err_body}");
                    warn!("Send attempt {attempt} failed: {err_msg}");
                    last_error = Some(err_msg);
                }

                Err(e) => {
                    let err_msg = format!("HTTP request failed: {e}");
                    warn!("Send attempt {attempt} failed: {err_msg}");
                    last_error = Some(err_msg);
                }
            }

            if attempt < MAX_DELIVERY_RETRIES {
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
        }

        let error_msg =
            last_error.unwrap_or_else(|| "Unknown error".to_string());
        error!(
            "Message delivery failed after {MAX_DELIVERY_RETRIES} attempts: \
             {error_msg}"
        );

        let mut state = self.state.write().await;
        state.last_error = Some(error_msg.clone());

        Err(MessagingError::SendFailed(error_msg))
    }

    /// Send a response within the 30-second timeout.
    ///
    /// Wraps the send operation in a tokio timeout to ensure we respond
    /// within the required 30-second window.
    pub async fn send_response_with_timeout(
        &self,
        target: &MessageTarget,
        content: MessageContent,
    ) -> Result<(), MessagingError> {
        let result = tokio::time::timeout(
            RESPONSE_TIMEOUT,
            self.send_message(target, content),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => {
                error!(
                    "Response timed out after {}s",
                    RESPONSE_TIMEOUT.as_secs()
                );
                Err(MessagingError::Timeout)
            }
        }
    }
}

// ─── MessagingBot Trait Implementation ───────────────────────────────────────

#[async_trait]
impl MessagingBot for WhatsAppBot {
    async fn start(&self) -> Result<(), MessagingError> {
        info!(
            "WhatsApp bot starting with phone_number_id={}",
            self.config.phone_number_id
        );

        // Verify connectivity by checking the phone number endpoint
        let url = format!(
            "{}/{}",
            WHATSAPP_API_BASE, self.config.phone_number_id
        );

        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
            .map_err(|e| {
                MessagingError::ConnectionFailed(format!(
                    "Cannot reach WhatsApp API: {e}"
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(MessagingError::AuthFailed(format!(
                "WhatsApp API returned {status}: {body}"
            )));
        }

        let mut state = self.state.write().await;
        state.connected = true;
        state.last_error = None;

        info!("WhatsApp bot started successfully");
        Ok(())
    }

    async fn stop(&self) -> Result<(), MessagingError> {
        info!("WhatsApp bot stopping");
        let mut state = self.state.write().await;
        state.connected = false;
        Ok(())
    }

    async fn send_message(
        &self,
        target: &MessageTarget,
        content: MessageContent,
    ) -> Result<(), MessagingError> {
        let to = &target.user_id;

        match content {
            MessageContent::Text(text) => {
                self.send_text(to, &text).await
            }
            MessageContent::Image { data, caption } => {
                let media_id = self.upload_media(&data).await?;
                self.send_image(to, &media_id, caption.as_deref()).await
            }
            MessageContent::TextWithImage { text, image } => {
                // WhatsApp doesn't support inline images with text in
                // a single message. Send image as attachment with text
                // as caption.
                let media_id = self.upload_media(&image).await?;
                self.send_image(to, &media_id, Some(&text)).await
            }
        }
    }

    fn status(&self) -> PlatformStatus {
        // Use try_read to avoid blocking
        match self.state.try_read() {
            Ok(s) => PlatformStatus {
                platform: Platform::WhatsApp,
                connected: s.connected,
                last_message_at: s.last_message_at,
                error: s.last_error.clone(),
            },
            Err(_) => PlatformStatus {
                platform: Platform::WhatsApp,
                connected: false,
                last_message_at: None,
                error: Some("State lock unavailable".to_string()),
            },
        }
    }

    fn platform(&self) -> Platform {
        Platform::WhatsApp
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WhatsAppConfig {
        WhatsAppConfig {
            phone_number_id: "123456789".to_string(),
            access_token: "test_access_token_abc123".to_string(),
            verify_token: "my_verify_token".to_string(),
            webhook_url: "https://example.com/webhook".to_string(),
            app_secret: None,
        }
    }

    #[test]
    fn test_new_bot_valid_config() {
        let bot = WhatsAppBot::new(test_config());
        assert!(bot.is_ok());
    }

    #[test]
    fn test_new_bot_empty_phone_number_id() {
        let mut config = test_config();
        config.phone_number_id = String::new();
        let bot = WhatsAppBot::new(config);
        assert!(bot.is_err());
    }

    #[test]
    fn test_new_bot_empty_access_token() {
        let mut config = test_config();
        config.access_token = String::new();
        let bot = WhatsAppBot::new(config);
        assert!(bot.is_err());
    }

    #[test]
    fn test_new_bot_empty_verify_token() {
        let mut config = test_config();
        config.verify_token = String::new();
        let bot = WhatsAppBot::new(config);
        assert!(bot.is_err());
    }

    #[test]
    fn test_webhook_verification_success() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let query = WebhookVerifyQuery {
            mode: "subscribe".to_string(),
            verify_token: "my_verify_token".to_string(),
            challenge: "challenge_string_123".to_string(),
        };
        let result = bot.verify_webhook(&query);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "challenge_string_123");
    }

    #[test]
    fn test_webhook_verification_wrong_token() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let query = WebhookVerifyQuery {
            mode: "subscribe".to_string(),
            verify_token: "wrong_token".to_string(),
            challenge: "challenge_string_123".to_string(),
        };
        let result = bot.verify_webhook(&query);
        assert!(result.is_err());
    }

    #[test]
    fn test_webhook_verification_wrong_mode() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let query = WebhookVerifyQuery {
            mode: "unsubscribe".to_string(),
            verify_token: "my_verify_token".to_string(),
            challenge: "challenge_string_123".to_string(),
        };
        let result = bot.verify_webhook(&query);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_status_command() {
        let cmd = WhatsAppBot::parse_command("!status");
        assert_eq!(cmd, Some(ManagementCommand::Status));
    }

    #[test]
    fn test_parse_tasks_command() {
        let cmd = WhatsAppBot::parse_command("!tasks");
        assert_eq!(cmd, Some(ManagementCommand::Tasks));
    }

    #[test]
    fn test_parse_mode_command() {
        let cmd = WhatsAppBot::parse_command("!mode coding");
        assert_eq!(
            cmd,
            Some(ManagementCommand::Mode(Some("coding".to_string())))
        );
    }

    #[test]
    fn test_parse_schedule_command() {
        let cmd = WhatsAppBot::parse_command("!schedule 0 * * * * backup");
        assert!(cmd.is_some());
        if let Some(ManagementCommand::Schedule(rest)) = cmd {
            assert!(rest.contains("0 * * * *"));
        }
    }

    #[test]
    fn test_parse_config_command() {
        let cmd = WhatsAppBot::parse_command("!config llm.model gpt-4");
        assert_eq!(
            cmd,
            Some(ManagementCommand::Config(
                Some("llm.model".to_string()),
                Some("gpt-4".to_string()),
            ))
        );
    }

    #[test]
    fn test_parse_non_command() {
        let cmd = WhatsAppBot::parse_command("hello world");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_platform_returns_whatsapp() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        assert_eq!(bot.platform(), Platform::WhatsApp);
    }

    #[test]
    fn test_status_when_not_started() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let status = bot.status();
        assert_eq!(status.platform, Platform::WhatsApp);
        assert!(!status.connected);
        assert!(status.last_message_at.is_none());
    }

    #[tokio::test]
    async fn test_process_webhook_valid_text_message() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let payload = WebhookPayload {
            object: "whatsapp_business_account".to_string(),
            entry: vec![WebhookEntry {
                id: "entry1".to_string(),
                changes: vec![WebhookChange {
                    field: "messages".to_string(),
                    value: WebhookValue {
                        messaging_product: Some("whatsapp".to_string()),
                        metadata: None,
                        contacts: None,
                        messages: Some(vec![WebhookMessage {
                            from: "15551234567".to_string(),
                            id: "msg_001".to_string(),
                            timestamp: "1234567890".to_string(),
                            message_type: "text".to_string(),
                            text: Some(WebhookTextBody {
                                body: "Hello agent!".to_string(),
                            }),
                        }]),
                        statuses: None,
                    },
                }],
            }],
        };

        let messages = bot.process_webhook(payload).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].platform_user_id, "15551234567");
        assert_eq!(messages[0].content, "Hello agent!");
        assert_eq!(messages[0].platform, Platform::WhatsApp);
    }

    #[tokio::test]
    async fn test_process_webhook_wrong_object() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let payload = WebhookPayload {
            object: "instagram".to_string(),
            entry: vec![],
        };
        let result = bot.process_webhook(payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_process_webhook_status_update_ignored() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let payload = WebhookPayload {
            object: "whatsapp_business_account".to_string(),
            entry: vec![WebhookEntry {
                id: "entry1".to_string(),
                changes: vec![WebhookChange {
                    field: "messages".to_string(),
                    value: WebhookValue {
                        messaging_product: Some("whatsapp".to_string()),
                        metadata: None,
                        contacts: None,
                        messages: None,
                        statuses: Some(vec![WebhookStatus {
                            id: "msg_001".to_string(),
                            status: "delivered".to_string(),
                            timestamp: "1234567890".to_string(),
                            recipient_id: "15551234567".to_string(),
                        }]),
                    },
                }],
            }],
        };

        let messages = bot.process_webhook(payload).await.unwrap();
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_process_webhook_non_text_message_ignored() {
        let bot = WhatsAppBot::new(test_config()).unwrap();
        let payload = WebhookPayload {
            object: "whatsapp_business_account".to_string(),
            entry: vec![WebhookEntry {
                id: "entry1".to_string(),
                changes: vec![WebhookChange {
                    field: "messages".to_string(),
                    value: WebhookValue {
                        messaging_product: Some("whatsapp".to_string()),
                        metadata: None,
                        contacts: None,
                        messages: Some(vec![WebhookMessage {
                            from: "15551234567".to_string(),
                            id: "msg_002".to_string(),
                            timestamp: "1234567890".to_string(),
                            message_type: "image".to_string(),
                            text: None,
                        }]),
                        statuses: None,
                    },
                }],
            }],
        };

        let messages = bot.process_webhook(payload).await.unwrap();
        assert!(messages.is_empty());
    }
}
