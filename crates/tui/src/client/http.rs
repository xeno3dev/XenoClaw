use reqwest::header::{HeaderValue, AUTHORIZATION};
use std::time::Duration;

use super::{ClientError, TaskSummary};

/// HTTP client for the XenoClaw API server.
///
/// Wraps `reqwest::Client` with auth headers and typed methods for
/// each REST endpoint the TUI needs.
pub struct ApiClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl ApiClient {
    /// Create a new API client.
    pub fn new(base_url: String, api_key: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest::Client::builder() should not fail");

        Self {
            client,
            base_url,
            api_key,
        }
    }

    fn auth_header(&self) -> Result<HeaderValue, ClientError> {
        HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|e| {
            ClientError::Protocol {
                message: format!("Invalid auth header: {e}"),
            }
        })
    }

    /// Check server health (unauthenticated).
    pub async fn health(&self) -> Result<(), ClientError> {
        let url = format!("{}/api/v1/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Network {
                message: e.to_string(),
            })?;

        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }

    /// Get agent status.
    pub async fn status(&self) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/api/v1/status", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, self.auth_header()?)
            .send()
            .await
            .map_err(|e| ClientError::Network {
                message: e.to_string(),
            })?;

        if resp.status().is_success() {
            resp.json::<serde_json::Value>()
                .await
                .map_err(|e| ClientError::Protocol {
                    message: e.to_string(),
                })
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }

    /// List active tasks.
    pub async fn tasks(&self) -> Result<Vec<TaskSummary>, ClientError> {
        let url = format!("{}/api/v1/tasks", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, self.auth_header()?)
            .send()
            .await
            .map_err(|e| ClientError::Network {
                message: e.to_string(),
            })?;

        if resp.status().is_success() {
            #[derive(serde::Deserialize)]
            struct TaskList {
                tasks: Vec<TaskSummary>,
            }
            let list = resp
                .json::<TaskList>()
                .await
                .map_err(|e| ClientError::Protocol {
                    message: e.to_string(),
                })?;
            Ok(list.tasks)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }

    /// Send a message via HTTP POST (alternative to WebSocket).
    pub async fn send_message(&self, session_id: &str, content: &str) -> Result<(), ClientError> {
        let url = format!("{}/api/v1/messages", self.base_url);
        let body = serde_json::json!({
            "session_id": session_id,
            "content": content,
        });
        let resp = self
            .client
            .post(&url)
            .header(AUTHORIZATION, self.auth_header()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Network {
                message: e.to_string(),
            })?;

        if resp.status().is_success() || resp.status().as_u16() == 202 {
            Ok(())
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Http { status, body })
        }
    }
}
