pub mod app;
pub mod client;
pub mod connection;
pub mod event;
pub mod inline;
pub mod input;
pub mod slash;
pub mod ui;

mod inline_colors;
mod theme;

use std::io;
use std::time::Duration;

use crate::app::App;
use crate::client::{ClientCommand, ServerEvent};
use crate::event::EventHandler;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use thiserror::Error;
use tokio::sync::mpsc;

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
    /// API base URL (e.g. "http://127.0.0.1:9090").
    pub api_base_url: String,
    /// WebSocket URL for chat streaming.
    pub ws_url: String,
    /// API key for authentication.
    pub api_key: String,
    /// Session UUID.
    pub session_id: uuid::Uuid,
    /// Inline mode (no alternate screen).
    pub inline: bool,
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
            api_base_url: String::new(),
            ws_url: String::new(),
            api_key: String::new(),
            session_id: uuid::Uuid::default(),
            inline: false,
            status_refresh_interval: Duration::from_secs(2),
            reconnect_interval: Duration::from_secs(5),
            max_reconnect_attempts: 6,
            max_history_size: 200,
        }
    }
}

/// Run the TUI application.
///
/// Dispatches to either inline or fullscreen mode based on `config.inline`.
pub async fn run(config: TuiConfig) -> Result<(), TuiError> {
    if config.inline {
        inline::run(config).await
    } else {
        run_fullscreen(config).await
    }
}

/// Run the fullscreen (Ratatui-based) TUI.
async fn run_fullscreen(config: TuiConfig) -> Result<(), TuiError> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Check minimum terminal size
    let size = terminal.size()?;
    if size.width < 80 || size.height < 24 {
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

    // Spawn HTTP + WS client tasks
    let http_client =
        client::http::ApiClient::new(config.api_base_url.clone(), config.api_key.clone());
    let (http_cmd_tx, ws_cmd_tx, server_rx) = client::ws::spawn_client_tasks(
        http_client,
        config.ws_url.clone(),
        config.api_key.clone(),
        config.session_id,
    );
    let cmd_tx = ws_cmd_tx;

    // Create app and event handler
    let mut app = App::new(config.clone());
    let event_handler = EventHandler::new(config.status_refresh_interval);

    // Run main loop
    let result = run_app(
        &mut terminal,
        &mut app,
        event_handler,
        server_rx,
        cmd_tx.clone(),
    )
    .await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Graceful shutdown of client tasks
    let _ = cmd_tx.try_send(ClientCommand::Shutdown);
    let _ = http_cmd_tx.try_send(ClientCommand::Shutdown);

    result
}

/// Main fullscreen application loop.
///
/// Selects between terminal input events and server events on every iteration
/// so that streaming tokens appear in real time without blocking on key input.
async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mut event_handler: EventHandler,
    mut server_rx: mpsc::Receiver<ServerEvent>,
    cmd_tx: mpsc::Sender<ClientCommand>,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        tokio::select! {
            app_event = event_handler.next() => {
                match app_event {
                    event::AppEvent::Tick => {
                        app.on_tick();
                    }
                    event::AppEvent::Key(key_event) => {
                        input::handle_key_event(app, key_event, Some(&cmd_tx));
                    }
                    event::AppEvent::Resize(width, height) => {
                        app.on_resize(width, height);
                    }
                }
            }
            server_event = server_rx.recv() => {
                match server_event {
                    Some(ev) => handle_server_event(app, ev),
                    None => break, // client tasks shut down
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
    Ok(())
}

/// Apply a server event to the app state.
fn handle_server_event(app: &mut App, event: ServerEvent) {
    use app::ChatRole;
    match event {
        ServerEvent::AssistantToken { content } => {
            app.append_assistant_token(&content);
        }
        ServerEvent::AssistantDone => {}
        ServerEvent::ToolCall { name, args_summary } => {
            app.add_message(ChatRole::System, format!("> {name}({args_summary})"));
        }
        ServerEvent::ToolResult {
            name,
            success,
            summary,
        } => {
            let sym = if success { "v" } else { "x" };
            app.add_message(ChatRole::System, format!("  {sym} {name}: {summary}"));
        }
        ServerEvent::Status {
            status,
            cpu,
            memory_mb,
            ..
        } => {
            app.update_status(status);
            app.update_resources(cpu, memory_mb);
        }
        ServerEvent::Connection(state) => {
            app.connection_state = state;
        }
        ServerEvent::Error { message } => {
            app.add_message(ChatRole::System, format!("! {message}"));
        }
    }
}
