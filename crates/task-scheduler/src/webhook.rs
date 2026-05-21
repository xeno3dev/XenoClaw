//! Webhook receipt triggers for task scheduling.
//!
//! Registers HTTP endpoint paths that, when called, trigger associated tasks.
//! The actual HTTP server is managed by the API server; this module provides
//! the registry and matching logic.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info};

use common::models::{HttpMethod, TaskTrigger};
use common::types::TaskId;

/// A triggered webhook event with associated task information.
#[derive(Debug, Clone)]
pub struct WebhookEvent {
    /// The task that should be triggered.
    pub task_id: TaskId,
    /// The path that was called.
    pub path: String,
    /// The HTTP method used.
    pub method: HttpMethod,
    /// Optional request body.
    pub body: Option<String>,
    /// Optional request headers.
    pub headers: HashMap<String, String>,
}

/// Configuration for a registered webhook endpoint.
#[derive(Debug, Clone)]
struct WebhookConfig {
    task_id: TaskId,
    path: String,
    method: HttpMethod,
}

/// Registry for webhook-triggered tasks.
///
/// Manages the mapping between HTTP endpoints and tasks. When a matching
/// HTTP request is received (via the API server), the registry dispatches
/// the appropriate task trigger event.
pub struct WebhookRegistry {
    /// Registered webhook endpoints.
    webhooks: Arc<RwLock<Vec<WebhookConfig>>>,
    /// Channel for sending triggered events to the scheduler.
    event_tx: mpsc::UnboundedSender<WebhookEvent>,
}

impl WebhookRegistry {
    /// Create a new WebhookRegistry with the given event channel.
    pub fn new(event_tx: mpsc::UnboundedSender<WebhookEvent>) -> Self {
        Self {
            webhooks: Arc::new(RwLock::new(Vec::new())),
            event_tx,
        }
    }

    /// Register a task's webhook trigger.
    pub async fn register(
        &self,
        task_id: TaskId,
        trigger: &TaskTrigger,
    ) -> Result<(), WebhookError> {
        if let TaskTrigger::Webhook { path, method } = trigger {
            // Check for duplicate registrations
            let webhooks = self.webhooks.read().await;
            let duplicate = webhooks
                .iter()
                .any(|w| w.path == *path && w.method == *method);
            if duplicate {
                return Err(WebhookError::PathAlreadyRegistered {
                    path: path.clone(),
                    method: *method,
                });
            }
            drop(webhooks);

            let config = WebhookConfig {
                task_id,
                path: path.clone(),
                method: *method,
            };

            self.webhooks.write().await.push(config);
            info!(
                "Registered webhook: {:?} {} for task {}",
                method, path, task_id
            );
            Ok(())
        } else {
            Err(WebhookError::InvalidTrigger)
        }
    }

    /// Unregister a task's webhook trigger.
    pub async fn unregister(&self, task_id: &TaskId) {
        self.webhooks
            .write()
            .await
            .retain(|w| w.task_id != *task_id);
        debug!("Unregistered webhooks for task {}", task_id);
    }

    /// Handle an incoming HTTP request and trigger matching tasks.
    ///
    /// Returns true if a matching webhook was found and triggered.
    pub async fn handle_request(
        &self,
        path: &str,
        method: HttpMethod,
        body: Option<String>,
        headers: HashMap<String, String>,
    ) -> bool {
        let webhooks = self.webhooks.read().await;
        let matching: Vec<_> = webhooks
            .iter()
            .filter(|w| w.path == path && w.method == method)
            .collect();

        if matching.is_empty() {
            return false;
        }

        for webhook in matching {
            let event = WebhookEvent {
                task_id: webhook.task_id,
                path: path.to_string(),
                method,
                body: body.clone(),
                headers: headers.clone(),
            };
            if let Err(e) = self.event_tx.send(event) {
                error!("Failed to send webhook event: {}", e);
            }
        }

        true
    }

    /// List all registered webhook endpoints.
    pub async fn list_endpoints(&self) -> Vec<(TaskId, String, HttpMethod)> {
        self.webhooks
            .read()
            .await
            .iter()
            .map(|w| (w.task_id, w.path.clone(), w.method))
            .collect()
    }

    /// Check if a specific path/method combination is registered.
    pub async fn is_registered(&self, path: &str, method: HttpMethod) -> bool {
        self.webhooks
            .read()
            .await
            .iter()
            .any(|w| w.path == path && w.method == method)
    }
}

/// Errors specific to the webhook registry.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Webhook path already registered: {method:?} {path}")]
    PathAlreadyRegistered { path: String, method: HttpMethod },

    #[error("Invalid trigger type for webhook registry")]
    InvalidTrigger,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_webhook() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let task_id = TaskId::new();
        let trigger = TaskTrigger::Webhook {
            path: "/hooks/deploy".to_string(),
            method: HttpMethod::Post,
        };

        let result = registry.register(task_id, &trigger).await;
        assert!(result.is_ok());
        assert!(
            registry
                .is_registered("/hooks/deploy", HttpMethod::Post)
                .await
        );
    }

    #[tokio::test]
    async fn test_register_duplicate_webhook() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let trigger = TaskTrigger::Webhook {
            path: "/hooks/deploy".to_string(),
            method: HttpMethod::Post,
        };

        registry.register(TaskId::new(), &trigger).await.unwrap();
        let result = registry.register(TaskId::new(), &trigger).await;
        assert!(matches!(
            result,
            Err(WebhookError::PathAlreadyRegistered { .. })
        ));
    }

    #[tokio::test]
    async fn test_register_invalid_trigger() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let trigger = TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        };

        let result = registry.register(TaskId::new(), &trigger).await;
        assert!(matches!(result, Err(WebhookError::InvalidTrigger)));
    }

    #[tokio::test]
    async fn test_handle_matching_request() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let task_id = TaskId::new();
        let trigger = TaskTrigger::Webhook {
            path: "/hooks/build".to_string(),
            method: HttpMethod::Post,
        };
        registry.register(task_id, &trigger).await.unwrap();

        let matched = registry
            .handle_request(
                "/hooks/build",
                HttpMethod::Post,
                Some("{\"ref\": \"main\"}".to_string()),
                HashMap::new(),
            )
            .await;

        assert!(matched);
        let event = rx.recv().await.unwrap();
        assert_eq!(event.task_id, task_id);
        assert_eq!(event.path, "/hooks/build");
    }

    #[tokio::test]
    async fn test_handle_non_matching_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let trigger = TaskTrigger::Webhook {
            path: "/hooks/build".to_string(),
            method: HttpMethod::Post,
        };
        registry.register(TaskId::new(), &trigger).await.unwrap();

        // Wrong path
        let matched = registry
            .handle_request("/hooks/deploy", HttpMethod::Post, None, HashMap::new())
            .await;
        assert!(!matched);

        // Wrong method
        let matched = registry
            .handle_request("/hooks/build", HttpMethod::Get, None, HashMap::new())
            .await;
        assert!(!matched);
    }

    #[tokio::test]
    async fn test_unregister_webhook() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let task_id = TaskId::new();
        let trigger = TaskTrigger::Webhook {
            path: "/hooks/test".to_string(),
            method: HttpMethod::Post,
        };
        registry.register(task_id, &trigger).await.unwrap();
        assert!(
            registry
                .is_registered("/hooks/test", HttpMethod::Post)
                .await
        );

        registry.unregister(&task_id).await;
        assert!(
            !registry
                .is_registered("/hooks/test", HttpMethod::Post)
                .await
        );
    }

    #[tokio::test]
    async fn test_list_endpoints() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let registry = WebhookRegistry::new(tx);

        let t1 = TaskId::new();
        let t2 = TaskId::new();

        registry
            .register(
                t1,
                &TaskTrigger::Webhook {
                    path: "/hooks/a".to_string(),
                    method: HttpMethod::Post,
                },
            )
            .await
            .unwrap();
        registry
            .register(
                t2,
                &TaskTrigger::Webhook {
                    path: "/hooks/b".to_string(),
                    method: HttpMethod::Get,
                },
            )
            .await
            .unwrap();

        let endpoints = registry.list_endpoints().await;
        assert_eq!(endpoints.len(), 2);
    }
}
