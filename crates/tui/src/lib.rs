//! TUI — terminal user interface for local/SSH-based interaction.
//!
//! Provides a full-featured terminal interface for the VPS AI Agent Platform,
//! built with Ratatui and Crossterm. Supports:
//!
//! - Chat interface with scrollable message history (200+ messages)
//! - Agent status display (idle/working/error, progress, resources)
//! - Keyboard shortcuts for common operations
//! - Responsive to terminal resize and SSH latency (up to 500ms)
//! - Auto-reconnect on WebSocket disconnection
//! - Minimum 80x24 terminal dimensions

pub mod app;
pub mod connection;
pub mod event;
pub mod input;
pub mod ui;

use std::io;
use std::time::Duration;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use thiserror::Error;

use crate::app::App;
use crate::event::EventHandler;

/// Errors that can occur in the TUI.
#[derive(Debug, Error)]
pub enum TuiError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Terminal too small: requires at least 80x24, got {width}x{height}")]
    TerminalTooSmall { width: u16, height: u16 },

    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Render error: {0}")]
    Render(String),
}

/// Configuration for the TUI application.
#[derive(Debug, Clone)]
pub struct TuiConfig {
    /// WebSocket URL to connect to the API server.
    pub ws_url: String,
    /// API key for authentication.
    pub api_key: String,
    /// Status refresh interval (default: 2 seconds).
    pub status_refresh_interval: Duration,
    /// Reconnect interval on disconnection (default: 5 seconds).
    pub reconnect_interval: Duration,
    /// Maximum reconnect attempts (default: 6).
    pub max_reconnect_attempts: u8,
    /// Maximum messages to keep in history (default: 200).
    pub max_history_size: usize,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            ws_url: "ws://127.0.0.1:3000/api/v1/ws/chat/default".to_string(),
            api_key: String::new(),
            status_refresh_interval: Duration::from_secs(2),
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_attempts: 6,
            max_history_size: 200,
        }
    }
}

/// Run the TUI application.
///
/// This is the main entry point for the terminal interface. It sets up the
/// terminal, runs the event loop, and restores the terminal on exit.
pub async fn run(config: TuiConfig) -> Result<(), TuiError> {
    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Check minimum terminal size
    let size = terminal.size()?;
    if size.width < 80 || size.height < 24 {
        // Restore terminal before returning error
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        return Err(TuiError::TerminalTooSmall {
            width: size.width,
            height: size.height,
        });
    }

    // Create app state and event handler
    let mut app = App::new(config.clone());
    let event_handler = EventHandler::new(config.status_refresh_interval);

    // Run the main loop
    let result = run_app(&mut terminal, &mut app, event_handler).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

/// Main application loop.
async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut event_handler: EventHandler,
) -> Result<(), TuiError> {
    loop {
        // Draw the UI
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Handle events
        match event_handler.next().await {
            event::AppEvent::Tick => {
                app.on_tick();
            }
            event::AppEvent::Key(key_event) => {
                input::handle_key_event(app, key_event);
            }
            event::AppEvent::Resize(width, height) => {
                app.on_resize(width, height);
            }
        }

        // Check if we should quit
        if app.should_quit {
            return Ok(());
        }
    }
}
