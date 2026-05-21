//! Application state for the TUI.
//!
//! The `App` struct holds all mutable state including message history,
//! input buffer, agent status, connection state, and resource usage.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::TuiConfig;

/// Maximum number of messages retained in history.
const DEFAULT_MAX_HISTORY: usize = 200;

/// Agent operational status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentStatus {
    #[default]
    Idle,
    Working {
        task: String,
        progress: Option<TaskProgress>,
    },
    Error {
        message: String,
    },
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Idle => write!(f, "Idle"),
            AgentStatus::Working { task, progress } => {
                write!(f, "Working: {task}")?;
                if let Some(p) = progress {
                    write!(f, " ({p})")?;
                }
                Ok(())
            }
            AgentStatus::Error { message } => write!(f, "Error: {message}"),
        }
    }
}

/// Task progress information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgress {
    /// Progress as a percentage (0-100).
    Percentage(u8),
    /// Progress as step count with label.
    Steps {
        current: u32,
        total: u32,
        label: String,
    },
}

impl std::fmt::Display for TaskProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskProgress::Percentage(p) => write!(f, "{}%", p),
            TaskProgress::Steps {
                current,
                total,
                label,
            } => {
                write!(f, "{}/{} {}", current, total, label)
            }
        }
    }
}

/// Resource usage metrics.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// CPU usage as a percentage (0.0 - 100.0).
    pub cpu_percent: f32,
    /// Memory usage in megabytes.
    pub memory_mb: f32,
}

/// WebSocket connection state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Connected,
    Disconnected {
        since: Instant,
        reconnect_attempts: u8,
    },
    Reconnecting {
        attempt: u8,
    },
}

/// A single chat message displayed in the TUI.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// Role of a chat message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

impl std::fmt::Display for ChatRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatRole::User => write!(f, "You"),
            ChatRole::Assistant => write!(f, "Agent"),
            ChatRole::System => write!(f, "System"),
        }
    }
}

/// The active interaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    General,
    Coding,
}

impl std::fmt::Display for InteractionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InteractionMode::General => write!(f, "General"),
            InteractionMode::Coding => write!(f, "Coding"),
        }
    }
}

/// Main application state.
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,

    /// Chat message history (bounded VecDeque).
    pub messages: VecDeque<ChatMessage>,

    /// Maximum messages to retain.
    pub max_history_size: usize,

    /// Current text input buffer.
    pub input: String,

    /// Cursor position within the input buffer.
    pub input_cursor: usize,

    /// Agent status.
    pub agent_status: AgentStatus,

    /// Resource usage metrics.
    pub resource_usage: ResourceUsage,

    /// WebSocket connection state.
    pub connection_state: ConnectionState,

    /// Current interaction mode.
    pub mode: InteractionMode,

    /// Whether the help overlay is visible.
    pub show_help: bool,

    /// Scroll offset for message history (0 = bottom/most recent).
    pub scroll_offset: usize,

    /// Input history for up/down navigation.
    pub input_history: Vec<String>,

    /// Current position in input history (-1 = current input).
    pub history_index: Option<usize>,

    /// Saved current input when navigating history.
    pub saved_input: String,

    /// Terminal dimensions.
    pub terminal_width: u16,
    pub terminal_height: u16,

    /// Last status refresh time.
    pub last_status_refresh: Instant,

    /// Status refresh interval.
    pub status_refresh_interval: Duration,

    /// Reconnect configuration.
    pub reconnect_interval: Duration,
    pub max_reconnect_attempts: u8,

    /// Last reconnect attempt time.
    pub last_reconnect_attempt: Option<Instant>,

    /// Tick counter for status refresh timing.
    tick_count: u64,
}

impl App {
    /// Create a new App with the given configuration.
    pub fn new(config: TuiConfig) -> Self {
        Self {
            should_quit: false,
            messages: VecDeque::with_capacity(config.max_history_size + 10),
            max_history_size: config.max_history_size.max(DEFAULT_MAX_HISTORY),
            input: String::new(),
            input_cursor: 0,
            agent_status: AgentStatus::default(),
            resource_usage: ResourceUsage::default(),
            connection_state: ConnectionState::default(),
            mode: InteractionMode::General,
            show_help: false,
            scroll_offset: 0,
            input_history: Vec::new(),
            history_index: None,
            saved_input: String::new(),
            terminal_width: 80,
            terminal_height: 24,
            last_status_refresh: Instant::now(),
            status_refresh_interval: config.status_refresh_interval,
            reconnect_interval: config.reconnect_interval,
            max_reconnect_attempts: config.max_reconnect_attempts,
            last_reconnect_attempt: None,
            tick_count: 0,
        }
    }

    /// Handle a tick event (called every ~100ms for responsive UI).
    pub fn on_tick(&mut self) {
        self.tick_count += 1;

        // Check if we need to refresh status (every 2 seconds)
        if self.last_status_refresh.elapsed() >= self.status_refresh_interval {
            self.refresh_status();
            self.last_status_refresh = Instant::now();
        }

        // Handle reconnection logic
        self.handle_reconnection();
    }

    /// Handle terminal resize.
    pub fn on_resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
    }

    /// Add a message to the history.
    pub fn add_message(&mut self, role: ChatRole, content: String) {
        let message = ChatMessage {
            role,
            content,
            timestamp: Utc::now(),
        };
        self.messages.push_back(message);

        // Trim to max history size
        while self.messages.len() > self.max_history_size {
            self.messages.pop_front();
        }

        // Auto-scroll to bottom on new message
        self.scroll_offset = 0;
    }

    /// Submit the current input as a message.
    pub fn submit_input(&mut self) {
        let input = self.input.trim().to_string();
        if input.is_empty() {
            return;
        }

        // Add to input history
        self.input_history.push(input.clone());
        self.history_index = None;
        self.saved_input.clear();

        // Add as user message
        self.add_message(ChatRole::User, input);

        // Clear input
        self.input.clear();
        self.input_cursor = 0;
    }

    /// Cancel the current task.
    pub fn cancel_task(&mut self) {
        if matches!(self.agent_status, AgentStatus::Working { .. }) {
            self.add_message(
                ChatRole::System,
                "Task cancellation requested...".to_string(),
            );
            self.agent_status = AgentStatus::Idle;
        }
    }

    /// Add a system message (used by slash commands).
    pub fn add_system_message(&mut self, content: String) {
        self.add_message(ChatRole::System, content);
    }

    /// Clear all messages from the chat history.
    pub fn clear_screen(&mut self) {
        self.messages.clear();
        self.add_message(ChatRole::System, "Screen cleared.".to_string());
    }

    /// Show current status as a system message.
    pub fn show_status(&mut self) {
        let status_line = format!(
            "Agent: {} | CPU: {:.1}% | Mem: {:.0}MB | Mode: {}",
            self.agent_status,
            self.resource_usage.cpu_percent,
            self.resource_usage.memory_mb,
            self.mode,
        );
        self.add_message(ChatRole::System, status_line);
    }

    /// Submit input inline (for inline mode — just adds to history).
    pub fn submit_input_inline(&mut self, input: &str) {
        self.input_history.push(input.to_string());
        self.history_index = None;
        self.saved_input.clear();
    }

    /// Navigate input history up (inline mode).
    pub fn history_up_inline(&mut self, input: &mut String) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.saved_input = input.clone();
                let idx = self.input_history.len() - 1;
                self.history_index = Some(idx);
                *input = self.input_history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                *input = self.input_history[new_idx].clone();
            }
            _ => {}
        }
    }

    /// Navigate input history down (inline mode).
    pub fn history_down_inline(&mut self, input: &mut String) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.input_history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                *input = self.input_history[new_idx].clone();
            } else {
                self.history_index = None;
                *input = self.saved_input.clone();
            }
        }
    }

    /// Toggle the interaction mode.
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            InteractionMode::General => InteractionMode::Coding,
            InteractionMode::Coding => InteractionMode::General,
        };
        self.add_message(ChatRole::System, format!("Switched to {} mode", self.mode));
    }

    /// Navigate input history (up).
    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }

        match self.history_index {
            None => {
                // Save current input and go to most recent history
                self.saved_input = self.input.clone();
                let idx = self.input_history.len() - 1;
                self.history_index = Some(idx);
                self.input = self.input_history[idx].clone();
                self.input_cursor = self.input.len();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                self.input = self.input_history[new_idx].clone();
                self.input_cursor = self.input.len();
            }
            _ => {}
        }
    }

    /// Navigate input history (down).
    pub fn history_down(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.input_history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.input = self.input_history[new_idx].clone();
                self.input_cursor = self.input.len();
            } else {
                // Restore saved input
                self.history_index = None;
                self.input = self.saved_input.clone();
                self.input_cursor = self.input.len();
            }
        }
    }

    /// Scroll message history up.
    pub fn scroll_up(&mut self) {
        let max_scroll = self.messages.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + 1).min(max_scroll);
    }

    /// Scroll message history down.
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Refresh resource usage status.
    fn refresh_status(&mut self) {
        // In a real implementation, this would query the API server.
        // For now, we simulate with placeholder values that would be
        // updated via the WebSocket connection.
    }

    /// Handle auto-reconnection logic.
    fn handle_reconnection(&mut self) {
        // Reconnection is now handled by the client/ws task.
        // This method remains for tick-based status refresh only.
    }

    /// Mark connection as disconnected.
    pub fn set_disconnected(&mut self) {
        self.connection_state = ConnectionState::Disconnected {
            since: Instant::now(),
            reconnect_attempts: 0,
        };
        self.last_reconnect_attempt = None;
    }

    /// Mark connection as connected.
    pub fn set_connected(&mut self) {
        self.connection_state = ConnectionState::Connected;
        self.last_reconnect_attempt = None;
    }

    /// Update agent status from server data.
    pub fn update_status(&mut self, status: AgentStatus) {
        self.agent_status = status;
    }

    /// Update resource usage from server data.
    pub fn update_resources(&mut self, cpu: f32, memory: f32) {
        self.resource_usage.cpu_percent = cpu;
        self.resource_usage.memory_mb = memory;
    }

    /// Append a streaming token to the last assistant message, or start a new one.
    pub fn append_assistant_token(&mut self, content: &str) {
        if let Some(msg) = self.messages.back_mut() {
            if msg.role == ChatRole::Assistant {
                msg.content.push_str(content);
                self.scroll_offset = 0;
                return;
            }
        }
        self.messages.push_back(ChatMessage {
            role: ChatRole::Assistant,
            content: content.to_string(),
            timestamp: Utc::now(),
        });
        while self.messages.len() > self.max_history_size {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> TuiConfig {
        TuiConfig::default()
    }

    #[test]
    fn test_app_creation() {
        let app = App::new(test_config());
        assert!(!app.should_quit);
        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert_eq!(app.agent_status, AgentStatus::Idle);
        assert_eq!(app.mode, InteractionMode::General);
        assert!(!app.show_help);
    }

    #[test]
    fn test_add_message_within_limit() {
        let mut app = App::new(test_config());
        for i in 0..200 {
            app.add_message(ChatRole::User, format!("Message {}", i));
        }
        assert_eq!(app.messages.len(), 200);
    }

    #[test]
    fn test_add_message_exceeds_limit_trims() {
        let mut config = test_config();
        config.max_history_size = 200;
        let mut app = App::new(config);
        for i in 0..250 {
            app.add_message(ChatRole::User, format!("Message {}", i));
        }
        assert_eq!(app.messages.len(), 200);
        // Oldest messages should be trimmed
        assert_eq!(app.messages.front().unwrap().content, "Message 50");
    }

    #[test]
    fn test_submit_input() {
        let mut app = App::new(test_config());
        app.input = "Hello agent".to_string();
        app.input_cursor = 11;
        app.submit_input();
        assert!(app.input.is_empty());
        assert_eq!(app.input_cursor, 0);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].content, "Hello agent");
        assert_eq!(app.messages[0].role, ChatRole::User);
    }

    #[test]
    fn test_submit_empty_input_does_nothing() {
        let mut app = App::new(test_config());
        app.input = "   ".to_string();
        app.submit_input();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn test_toggle_mode() {
        let mut app = App::new(test_config());
        assert_eq!(app.mode, InteractionMode::General);
        app.toggle_mode();
        assert_eq!(app.mode, InteractionMode::Coding);
        app.toggle_mode();
        assert_eq!(app.mode, InteractionMode::General);
    }

    #[test]
    fn test_history_navigation() {
        let mut app = App::new(test_config());
        app.input = "first".to_string();
        app.submit_input();
        app.input = "second".to_string();
        app.submit_input();

        // Navigate up
        app.input = "current".to_string();
        app.history_up();
        assert_eq!(app.input, "second");
        app.history_up();
        assert_eq!(app.input, "first");

        // Navigate down
        app.history_down();
        assert_eq!(app.input, "second");
        app.history_down();
        assert_eq!(app.input, "current");
    }

    #[test]
    fn test_scroll() {
        let mut app = App::new(test_config());
        for i in 0..10 {
            app.add_message(ChatRole::User, format!("msg {}", i));
        }
        assert_eq!(app.scroll_offset, 0);
        app.scroll_up();
        assert_eq!(app.scroll_offset, 1);
        app.scroll_up();
        assert_eq!(app.scroll_offset, 2);
        app.scroll_down();
        assert_eq!(app.scroll_offset, 1);
        app.scroll_down();
        assert_eq!(app.scroll_offset, 0);
        // Can't scroll below 0
        app.scroll_down();
        assert_eq!(app.scroll_offset, 0);
    }

    #[test]
    fn test_cancel_task_when_working() {
        let mut app = App::new(test_config());
        app.agent_status = AgentStatus::Working {
            task: "Building project".to_string(),
            progress: Some(TaskProgress::Percentage(50)),
        };
        app.cancel_task();
        assert_eq!(app.agent_status, AgentStatus::Idle);
        assert_eq!(app.messages.len(), 1); // System message about cancellation
    }

    #[test]
    fn test_cancel_task_when_idle_does_nothing() {
        let mut app = App::new(test_config());
        app.cancel_task();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn test_connection_state_transitions() {
        let mut app = App::new(test_config());
        assert_eq!(app.connection_state, ConnectionState::Connected);

        app.set_disconnected();
        assert!(matches!(
            app.connection_state,
            ConnectionState::Disconnected { .. }
        ));

        app.set_connected();
        assert_eq!(app.connection_state, ConnectionState::Connected);
    }

    #[test]
    fn test_resize() {
        let mut app = App::new(test_config());
        app.on_resize(120, 40);
        assert_eq!(app.terminal_width, 120);
        assert_eq!(app.terminal_height, 40);
    }
}
