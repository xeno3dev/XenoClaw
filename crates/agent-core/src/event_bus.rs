//! Event Bus — async channel-based event bus for component communication.
//!
//! The Event Bus enables loose coupling between platform components by providing
//! a publish/subscribe mechanism. Components can publish events and subscribe to
//! specific event types without direct dependencies on each other.
//!
//! Uses `tokio::sync::broadcast` channels for multi-subscriber support.
//!
//! # Event Types
//!
//! - `ToolRegistered` / `ToolUnregistered` — Tool lifecycle events
//! - `TaskCompleted` / `TaskFailed` — Task execution outcomes
//! - `MessageReceived` — New message arrived for processing
//! - `AgentModeChanged` — Agent switched between General and Coding modes
//! - `PluginLoaded` / `PluginUnloaded` — Plugin lifecycle events
//!
//! # Example
//!
//! ```rust,no_run
//! use agent_core::event_bus::{EventBus, Event, EventType};
//! use serde_json::json;
//!
//! # async fn example() {
//! let bus = EventBus::new(128);
//!
//! // Subscribe to tool events
//! let mut rx = bus.subscribe(EventType::ToolRegistered);
//!
//! // Publish an event
//! bus.publish(Event::new(
//!     EventType::ToolRegistered,
//!     json!({ "tool_name": "echo", "plugin": null }),
//! ));
//!
//! // Receive the event
//! if let Ok(event) = rx.recv().await {
//!     println!("Tool registered: {:?}", event.payload);
//! }
//! # }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};
use uuid::Uuid;

/// The types of events that can be published on the event bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// A tool was registered in the Tool Registry.
    ToolRegistered,
    /// A tool was unregistered from the Tool Registry.
    ToolUnregistered,
    /// A scheduled task completed successfully.
    TaskCompleted,
    /// A scheduled task failed.
    TaskFailed,
    /// A new message was received for processing.
    MessageReceived,
    /// The agent mode was changed (General ↔ Coding).
    AgentModeChanged,
    /// A plugin was loaded.
    PluginLoaded,
    /// A plugin was unloaded.
    PluginUnloaded,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::ToolRegistered => write!(f, "tool_registered"),
            EventType::ToolUnregistered => write!(f, "tool_unregistered"),
            EventType::TaskCompleted => write!(f, "task_completed"),
            EventType::TaskFailed => write!(f, "task_failed"),
            EventType::MessageReceived => write!(f, "message_received"),
            EventType::AgentModeChanged => write!(f, "agent_mode_changed"),
            EventType::PluginLoaded => write!(f, "plugin_loaded"),
            EventType::PluginUnloaded => write!(f, "plugin_unloaded"),
        }
    }
}

/// An event published on the event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique identifier for this event instance.
    pub id: Uuid,
    /// The type of event.
    pub event_type: EventType,
    /// The event payload (typed data as JSON).
    pub payload: Value,
    /// When the event was created.
    pub timestamp: DateTime<Utc>,
}

impl Event {
    /// Create a new event with the given type and payload.
    pub fn new(event_type: EventType, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type,
            payload,
            timestamp: Utc::now(),
        }
    }

    /// Create a new event with an empty payload.
    pub fn empty(event_type: EventType) -> Self {
        Self::new(event_type, Value::Null)
    }
}

/// A receiver for events of a specific type.
///
/// Wraps a `tokio::sync::broadcast::Receiver` and filters events by type.
pub struct EventReceiver {
    inner: broadcast::Receiver<Event>,
    event_type: EventType,
}

impl EventReceiver {
    /// Receive the next event matching this receiver's event type.
    ///
    /// Blocks until an event is available or the channel is closed.
    pub async fn recv(&mut self) -> Result<Event, EventRecvError> {
        loop {
            match self.inner.recv().await {
                Ok(event) if event.event_type == self.event_type => return Ok(event),
                Ok(_) => continue, // Skip events of other types
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        event_type = %self.event_type,
                        skipped = n,
                        "Event receiver lagged, skipped events"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(EventRecvError::Closed);
                }
            }
        }
    }

    /// Try to receive the next event without blocking.
    ///
    /// Returns `None` if no event is immediately available.
    pub fn try_recv(&mut self) -> Option<Event> {
        loop {
            match self.inner.try_recv() {
                Ok(event) if event.event_type == self.event_type => return Some(event),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    /// The event type this receiver is subscribed to.
    pub fn event_type(&self) -> EventType {
        self.event_type
    }
}

/// Errors that can occur when receiving events.
#[derive(Debug, thiserror::Error)]
pub enum EventRecvError {
    /// The event bus has been dropped and no more events will be sent.
    #[error("Event bus closed")]
    Closed,
}

/// The Event Bus provides async publish/subscribe communication between components.
///
/// Multiple subscribers can listen for the same event type. Events are broadcast
/// to all active subscribers. If a subscriber falls behind, it will skip missed
/// events (with a warning logged).
pub struct EventBus {
    /// The broadcast sender — all events go through this single channel.
    sender: broadcast::Sender<Event>,
    /// Track subscriber counts per event type for diagnostics.
    subscriber_counts: Arc<RwLock<HashMap<EventType, usize>>>,
}

impl EventBus {
    /// Create a new EventBus with the given channel capacity.
    ///
    /// The capacity determines how many events can be buffered before
    /// slow subscribers start lagging (missing events).
    ///
    /// A capacity of 128–256 is suitable for most workloads.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            subscriber_counts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Publish an event to all subscribers.
    ///
    /// Returns the number of active receivers that will receive this event.
    /// Returns 0 if there are no active subscribers.
    pub fn publish(&self, event: Event) -> usize {
        trace!(
            event_type = %event.event_type,
            event_id = %event.id,
            "Publishing event"
        );

        match self.sender.send(event) {
            Ok(receiver_count) => {
                debug!(
                    receiver_count = receiver_count,
                    "Event published successfully"
                );
                receiver_count
            }
            Err(_) => {
                // No active receivers — this is not an error, just means nobody is listening
                trace!("Event published but no active receivers");
                0
            }
        }
    }

    /// Subscribe to events of a specific type.
    ///
    /// Returns an `EventReceiver` that will yield only events matching
    /// the specified type. Multiple subscribers can listen to the same type.
    pub fn subscribe(&self, event_type: EventType) -> EventReceiver {
        let inner = self.sender.subscribe();

        // Track subscriber count (best-effort, non-blocking)
        let counts = Arc::clone(&self.subscriber_counts);
        tokio::spawn(async move {
            let mut counts = counts.write().await;
            *counts.entry(event_type).or_insert(0) += 1;
        });

        debug!(
            event_type = %event_type,
            "New subscriber registered"
        );

        EventReceiver { inner, event_type }
    }

    /// Get the number of active receivers on the broadcast channel.
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Get subscriber counts per event type (diagnostic info).
    pub async fn subscriber_counts(&self) -> HashMap<EventType, usize> {
        self.subscriber_counts.read().await.clone()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}

impl Clone for EventBus {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            subscriber_counts: Arc::clone(&self.subscriber_counts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolRegistered);

        let event = Event::new(
            EventType::ToolRegistered,
            json!({ "tool_name": "echo" }),
        );
        let count = bus.publish(event.clone());
        assert_eq!(count, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, EventType::ToolRegistered);
        assert_eq!(received.payload["tool_name"], "echo");
    }

    #[tokio::test]
    async fn test_multiple_subscribers_same_type() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe(EventType::TaskCompleted);
        let mut rx2 = bus.subscribe(EventType::TaskCompleted);

        let event = Event::new(
            EventType::TaskCompleted,
            json!({ "task_id": "abc-123" }),
        );
        let count = bus.publish(event);
        assert_eq!(count, 2);

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        assert_eq!(e1.payload["task_id"], "abc-123");
        assert_eq!(e2.payload["task_id"], "abc-123");
    }

    #[tokio::test]
    async fn test_subscriber_filters_by_type() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::PluginLoaded);

        // Publish events of different types
        bus.publish(Event::new(EventType::ToolRegistered, json!({})));
        bus.publish(Event::new(EventType::TaskCompleted, json!({})));
        bus.publish(Event::new(
            EventType::PluginLoaded,
            json!({ "plugin": "my-plugin" }),
        ));

        // Should only receive the PluginLoaded event
        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, EventType::PluginLoaded);
        assert_eq!(received.payload["plugin"], "my-plugin");
    }

    #[tokio::test]
    async fn test_publish_with_no_subscribers() {
        let bus = EventBus::new(16);

        // Publishing with no subscribers should return 0
        let count = bus.publish(Event::empty(EventType::MessageReceived));
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_event_creation() {
        let event = Event::new(
            EventType::AgentModeChanged,
            json!({ "from": "general", "to": "coding" }),
        );

        assert_eq!(event.event_type, EventType::AgentModeChanged);
        assert_eq!(event.payload["from"], "general");
        assert_eq!(event.payload["to"], "coding");
        assert!(!event.id.is_nil());
    }

    #[tokio::test]
    async fn test_empty_event() {
        let event = Event::empty(EventType::TaskFailed);
        assert_eq!(event.event_type, EventType::TaskFailed);
        assert_eq!(event.payload, Value::Null);
    }

    #[tokio::test]
    async fn test_try_recv_no_event() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolUnregistered);

        // No events published yet
        assert!(rx.try_recv().is_none());
    }

    #[tokio::test]
    async fn test_try_recv_with_event() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::PluginUnloaded);

        bus.publish(Event::new(
            EventType::PluginUnloaded,
            json!({ "plugin": "old-plugin" }),
        ));

        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::PluginUnloaded);
        assert_eq!(event.payload["plugin"], "old-plugin");
    }

    #[tokio::test]
    async fn test_receiver_count() {
        let bus = EventBus::new(16);
        assert_eq!(bus.receiver_count(), 0);

        let _rx1 = bus.subscribe(EventType::ToolRegistered);
        assert_eq!(bus.receiver_count(), 1);

        let _rx2 = bus.subscribe(EventType::TaskCompleted);
        assert_eq!(bus.receiver_count(), 2);

        drop(_rx1);
        // Note: broadcast receiver count may not update immediately
    }

    #[tokio::test]
    async fn test_clone_shares_channel() {
        let bus1 = EventBus::new(16);
        let bus2 = bus1.clone();

        let mut rx = bus1.subscribe(EventType::MessageReceived);

        // Publish on the clone
        bus2.publish(Event::new(
            EventType::MessageReceived,
            json!({ "session": "s1" }),
        ));

        // Receive on the original's subscriber
        let event = rx.recv().await.unwrap();
        assert_eq!(event.payload["session"], "s1");
    }

    #[tokio::test]
    async fn test_event_type_display() {
        assert_eq!(EventType::ToolRegistered.to_string(), "tool_registered");
        assert_eq!(EventType::ToolUnregistered.to_string(), "tool_unregistered");
        assert_eq!(EventType::TaskCompleted.to_string(), "task_completed");
        assert_eq!(EventType::TaskFailed.to_string(), "task_failed");
        assert_eq!(EventType::MessageReceived.to_string(), "message_received");
        assert_eq!(EventType::AgentModeChanged.to_string(), "agent_mode_changed");
        assert_eq!(EventType::PluginLoaded.to_string(), "plugin_loaded");
        assert_eq!(EventType::PluginUnloaded.to_string(), "plugin_unloaded");
    }
}
