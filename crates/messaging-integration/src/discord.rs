//! Discord bot integration using serenity.
//!
//! Implements the `MessagingBot` trait for Discord, handling:
//! - Gateway connection and message events
//! - Management commands (!status, !tasks, !mode, !schedule, !config)
//! - Response delivery within 30 seconds (tokio timeout)
//! - Diff image delivery as attachments

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serenity::all::{
    ChannelId, Context, CreateAttachment, CreateMessage, EventHandler, GatewayIntents, Message,
    Ready,
};
use serenity::Client;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info};

use crate::{
    IncomingMessage, ManagementCommand, MessageContent, MessageHandler, MessageTarget,
    MessagingBot, MessagingError, Platform, PlatformStatus,
};

/// Configuration for the Discord bot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    /// Bot token for authentication with Discord.
    pub bot_token: String,
    /// Guild (server) IDs the bot should operate in. Empty means all guilds.
    pub guild_ids: Vec<u64>,
    /// Channel IDs the bot should listen to. Empty means all channels.
    pub channel_ids: Vec<u64>,
    /// Response timeout in seconds (default: 30).
    #[serde(default = "default_response_timeout")]
    pub response_timeout_secs: u64,
}

fn default_response_timeout() -> u64 {
    30
}

/// Discord bot implementation using serenity.
pub struct DiscordBot {
    config: DiscordConfig,
    handler: Arc<dyn MessageHandler>,
    status: Arc<RwLock<PlatformStatus>>,
    client: Arc<Mutex<Option<Client>>>,
}

impl DiscordBot {
    /// Create a new Discord bot with the given configuration and message handler.
    pub fn new(config: DiscordConfig, handler: Arc<dyn MessageHandler>) -> Self {
        Self {
            config,
            handler,
            status: Arc::new(RwLock::new(PlatformStatus {
                platform: Platform::Discord,
                connected: false,
                last_message_at: None,
                error: None,
            })),
            client: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl MessagingBot for DiscordBot {
    async fn start(&self) -> Result<(), MessagingError> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;

        let event_handler = DiscordEventHandler {
            config: self.config.clone(),
            handler: Arc::clone(&self.handler),
            status: Arc::clone(&self.status),
            response_timeout: std::time::Duration::from_secs(self.config.response_timeout_secs),
        };

        let client = Client::builder(&self.config.bot_token, intents)
            .event_handler(event_handler)
            .await
            .map_err(|e| {
                MessagingError::ConnectionFailed(format!("Failed to build Discord client: {e}"))
            })?;

        // Store the client for later shutdown
        {
            let mut client_lock = self.client.lock().await;
            *client_lock = Some(client);
        }

        // Start the client in a background task
        let client_arc = Arc::clone(&self.client);
        let status_arc = Arc::clone(&self.status);

        tokio::spawn(async move {
            let mut client_guard = client_arc.lock().await;
            if let Some(ref mut client) = *client_guard {
                if let Err(e) = client.start().await {
                    error!("Discord client error: {e}");
                    let mut status = status_arc.write().await;
                    status.connected = false;
                    status.error = Some(format!("Client error: {e}"));
                }
            }
        });

        info!("Discord bot started");
        Ok(())
    }

    async fn stop(&self) -> Result<(), MessagingError> {
        let mut client_lock = self.client.lock().await;
        if let Some(ref client) = *client_lock {
            client.shard_manager.shutdown_all().await;
        }
        *client_lock = None;

        let mut status = self.status.write().await;
        status.connected = false;
        status.error = None;

        info!("Discord bot stopped");
        Ok(())
    }

    async fn send_message(
        &self,
        target: &MessageTarget,
        content: MessageContent,
    ) -> Result<(), MessagingError> {
        let client_lock = self.client.lock().await;
        let client = client_lock
            .as_ref()
            .ok_or_else(|| MessagingError::ConnectionFailed("Discord client not started".into()))?;

        let channel_id = target.channel_id.as_ref().ok_or_else(|| {
            MessagingError::SendFailed("No channel_id specified for Discord message".into())
        })?;

        let channel = ChannelId::new(
            channel_id
                .parse::<u64>()
                .map_err(|e| MessagingError::SendFailed(format!("Invalid channel ID: {e}")))?,
        );

        let http = &client.http;

        match content {
            MessageContent::Text(text) => {
                let msg = CreateMessage::new().content(text);
                channel
                    .send_message(http, msg)
                    .await
                    .map_err(|e| MessagingError::SendFailed(format!("Discord send error: {e}")))?;
            }
            MessageContent::Image { data, caption } => {
                let attachment = CreateAttachment::bytes(data, "diff.png");
                let mut msg = CreateMessage::new().add_file(attachment);
                if let Some(cap) = caption {
                    msg = msg.content(cap);
                }
                channel
                    .send_message(http, msg)
                    .await
                    .map_err(|e| MessagingError::SendFailed(format!("Discord send error: {e}")))?;
            }
            MessageContent::TextWithImage { text, image } => {
                let attachment = CreateAttachment::bytes(image, "diff.png");
                let msg = CreateMessage::new().content(text).add_file(attachment);
                channel
                    .send_message(http, msg)
                    .await
                    .map_err(|e| MessagingError::SendFailed(format!("Discord send error: {e}")))?;
            }
        }

        Ok(())
    }

    fn status(&self) -> PlatformStatus {
        // Use try_read to avoid blocking; fall back to a default if locked
        match self.status.try_read() {
            Ok(s) => s.clone(),
            Err(_) => PlatformStatus {
                platform: Platform::Discord,
                connected: false,
                last_message_at: None,
                error: Some("Status unavailable".into()),
            },
        }
    }

    fn platform(&self) -> Platform {
        Platform::Discord
    }
}

/// Serenity event handler that processes Discord gateway events.
struct DiscordEventHandler {
    config: DiscordConfig,
    handler: Arc<dyn MessageHandler>,
    status: Arc<RwLock<PlatformStatus>>,
    response_timeout: std::time::Duration,
}

#[async_trait]
impl EventHandler for DiscordEventHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
        let mut status = self.status.write().await;
        status.connected = true;
        status.error = None;
    }

    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore messages from bots (including ourselves)
        if msg.author.bot {
            return;
        }

        // Filter by guild if configured
        if !self.config.guild_ids.is_empty() {
            if let Some(guild_id) = msg.guild_id {
                if !self.config.guild_ids.contains(&guild_id.get()) {
                    return;
                }
            }
        }

        // Filter by channel if configured
        if !self.config.channel_ids.is_empty()
            && !self.config.channel_ids.contains(&msg.channel_id.get())
        {
            return;
        }

        // Update last message timestamp
        {
            let mut status = self.status.write().await;
            status.last_message_at = Some(Utc::now());
        }

        let incoming = IncomingMessage {
            platform: Platform::Discord,
            platform_user_id: msg.author.id.get().to_string(),
            channel_id: Some(msg.channel_id.get().to_string()),
            content: msg.content.clone(),
            timestamp: Utc::now(),
            // Discord attachment download is a follow-up (Telegram handled first).
            attachments: Vec::new(),
        };

        // Check if this is a management command
        let response = if let Some(command) = ManagementCommand::parse(&msg.content) {
            // Process command with timeout
            let handler = Arc::clone(&self.handler);
            let incoming_clone = incoming.clone();
            let result = tokio::time::timeout(self.response_timeout, async move {
                handler.handle_command(command, &incoming_clone).await
            })
            .await;

            match result {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => format!("❌ Command error: {e}"),
                Err(_) => "⏱️ Command timed out (30s limit exceeded)".to_string(),
            }
        } else {
            // Process as conversation message with timeout
            let handler = Arc::clone(&self.handler);
            let result = tokio::time::timeout(self.response_timeout, async move {
                handler.handle_message(incoming).await
            })
            .await;

            match result {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => format!("❌ Error: {e}"),
                Err(_) => "⏱️ Response timed out (30s limit exceeded)".to_string(),
            }
        };

        // Send the response back to the same channel
        if let Err(e) = msg.channel_id.say(&ctx.http, &response).await {
            error!("Failed to send Discord response: {e}");
        }
    }
}

/// Send a diff image to a Discord channel.
///
/// This is a convenience function for delivering diff images from the coding module.
pub async fn send_diff_image(
    bot: &DiscordBot,
    channel_id: &str,
    image_data: Vec<u8>,
    caption: Option<String>,
) -> Result<(), MessagingError> {
    let target = MessageTarget {
        platform: Platform::Discord,
        user_id: String::new(),
        channel_id: Some(channel_id.to_string()),
    };

    let content = match caption {
        Some(cap) => MessageContent::TextWithImage {
            text: cap,
            image: image_data,
        },
        None => MessageContent::Image {
            data: image_data,
            caption: None,
        },
    };

    bot.send_message(&target, content).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_management_command_parse_status() {
        assert_eq!(
            ManagementCommand::parse("!status"),
            Some(ManagementCommand::Status)
        );
    }

    #[test]
    fn test_management_command_parse_tasks() {
        assert_eq!(
            ManagementCommand::parse("!tasks"),
            Some(ManagementCommand::Tasks)
        );
    }

    #[test]
    fn test_management_command_parse_mode_with_arg() {
        assert_eq!(
            ManagementCommand::parse("!mode coding"),
            Some(ManagementCommand::Mode(Some("coding".to_string())))
        );
    }

    #[test]
    fn test_management_command_parse_mode_without_arg() {
        assert_eq!(
            ManagementCommand::parse("!mode"),
            Some(ManagementCommand::Mode(None))
        );
    }

    #[test]
    fn test_management_command_parse_config() {
        assert_eq!(
            ManagementCommand::parse("!config log_level debug"),
            Some(ManagementCommand::Config(
                Some("log_level".to_string()),
                Some("debug".to_string())
            ))
        );
    }

    #[test]
    fn test_management_command_parse_schedule() {
        assert_eq!(
            ManagementCommand::parse("!schedule 0 * * * * run_backup"),
            Some(ManagementCommand::Schedule(
                "0 * * * * run_backup".to_string()
            ))
        );
    }

    #[test]
    fn test_management_command_parse_not_a_command() {
        assert_eq!(ManagementCommand::parse("hello world"), None);
    }

    #[test]
    fn test_management_command_parse_unknown_command() {
        assert_eq!(ManagementCommand::parse("!unknown"), None);
    }

    #[test]
    fn test_discord_config_defaults() {
        let json = r#"{"bot_token": "test-token", "guild_ids": [], "channel_ids": []}"#;
        let config: DiscordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.response_timeout_secs, 30);
    }
}
