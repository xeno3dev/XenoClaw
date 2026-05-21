use std::io::{self, Write};
use std::time::Duration;

use crossterm::{
    cursor, event, execute,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};
use tokio::sync::mpsc;

use crate::app::{AgentStatus, App};
use crate::client::http::ApiClient;
use crate::client::ws;
use crate::client::{ClientCommand, ServerEvent};
use crate::inline_colors::{DIM, GREEN, RED, RED_BRIGHT, WHITE};
use crate::slash;
use crate::TuiConfig;

/// Result returned by the key handler — drives rendering decisions in the loop.
enum InlineKeyResult {
    /// Normal edit — redraw the input line.
    Continue,
    /// User submitted a message; "xenoclaw > " was already printed.
    Submitted,
    /// Application should exit.
    Exit,
}

/// Run the TUI in inline mode (no alternate screen).
pub async fn run(config: TuiConfig) -> Result<(), crate::TuiError> {
    let http_client = ApiClient::new(config.api_base_url.clone(), config.api_key.clone());

    let ws_url = if config.ws_url.is_empty() {
        let base = config.api_base_url.trim_start_matches("http://");
        format!("ws://{}/api/v1/ws/chat?token={}", base, config.api_key)
    } else {
        config.ws_url.clone()
    };

    let (http_cmd_tx, ws_cmd_tx, mut event_rx) = ws::spawn_client_tasks(
        http_client,
        ws_url,
        config.api_key.clone(),
        config.session_id,
    );
    let cmd_tx = ws_cmd_tx.clone();

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Show)?;

    let mut app = App::new(config);

    // Show the initial prompt
    redraw_input_line(&mut stdout, "", 0)?;
    stdout.flush()?;

    let result = run_inline_loop(&mut app, &mut event_rx, &cmd_tx, &mut stdout).await;

    terminal::disable_raw_mode()?;
    execute!(stdout, ResetColor, cursor::Show)?;

    let _ = http_cmd_tx.try_send(ClientCommand::Shutdown);
    let _ = ws_cmd_tx.try_send(ClientCommand::Shutdown);

    result
}

/// Main inline event/input loop.
async fn run_inline_loop(
    app: &mut App,
    event_rx: &mut mpsc::Receiver<ServerEvent>,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    stdout: &mut io::Stdout,
) -> Result<(), crate::TuiError> {
    let mut input = String::new();
    let mut cursor_pos: usize = 0;

    loop {
        // Check for backend events with a short timeout
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(ServerEvent::AssistantToken { content }) => {
                        print!("{content}");
                        stdout.flush()?;
                    }
                    Some(ServerEvent::AssistantDone) => {
                        // End streaming line, then restore the input prompt
                        execute!(stdout, Print("\r\n"))?;
                        redraw_input_line(stdout, &input, cursor_pos)?;
                        stdout.flush()?;
                    }
                    Some(ServerEvent::ToolCall { name, args_summary }) => {
                        erase_input_line(stdout)?;
                        print_tool_call(stdout, &name, &args_summary)?;
                        redraw_input_line(stdout, &input, cursor_pos)?;
                        stdout.flush()?;
                    }
                    Some(ServerEvent::ToolResult { name, success, summary }) => {
                        erase_input_line(stdout)?;
                        print_tool_result(stdout, &name, success, &summary)?;
                        redraw_input_line(stdout, &input, cursor_pos)?;
                        stdout.flush()?;
                    }
                    Some(ServerEvent::Connection(state)) => {
                        app.connection_state = state;
                    }
                    Some(ServerEvent::Status { status, cpu, memory_mb, .. }) => {
                        app.agent_status = status;
                        app.resource_usage.cpu_percent = cpu;
                        app.resource_usage.memory_mb = memory_mb;
                    }
                    Some(ServerEvent::Error { message }) => {
                        erase_input_line(stdout)?;
                        print_system_message(stdout, &message)?;
                        redraw_input_line(stdout, &input, cursor_pos)?;
                        stdout.flush()?;
                    }
                    None => break,
                }
                continue;
            }
            _ = tokio::time::sleep(Duration::from_millis(30)) => {
                // Timeout — check for key events
            }
        }

        // Poll for a key event (non-blocking)
        if event::poll(Duration::from_millis(0)).unwrap_or(false) {
            if let Ok(event::Event::Key(key)) = event::read() {
                match handle_key(app, key, &mut input, &mut cursor_pos, cmd_tx) {
                    InlineKeyResult::Exit => break,
                    InlineKeyResult::Submitted => {
                        // "xenoclaw > " was already printed inside handle_key; no redraw needed
                    }
                    InlineKeyResult::Continue => {
                        redraw_input_line(stdout, &input, cursor_pos)?;
                        stdout.flush()?;
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Handle a key event in inline mode.
fn handle_key(
    app: &mut App,
    key: event::KeyEvent,
    input: &mut String,
    cursor_pos: &mut usize,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) -> InlineKeyResult {
    use event::{KeyCode, KeyModifiers};

    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
            if matches!(app.agent_status, AgentStatus::Working { .. }) {
                let _ = cmd_tx.try_send(ClientCommand::CancelGeneration);
                app.cancel_task();
                InlineKeyResult::Continue
            } else {
                app.should_quit = true;
                InlineKeyResult::Exit
            }
        }
        (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
            if input.is_empty() {
                app.should_quit = true;
                InlineKeyResult::Exit
            } else {
                InlineKeyResult::Continue
            }
        }
        (KeyModifiers::NONE, KeyCode::Enter) => {
            let trimmed = input.trim().to_string();
            if trimmed.is_empty() {
                return InlineKeyResult::Continue;
            }

            if let Some(cmd) = slash::parse(&trimmed) {
                slash::execute(cmd, app, cmd_tx);
                input.clear();
                *cursor_pos = 0;
                if app.should_quit {
                    return InlineKeyResult::Exit;
                }
                return InlineKeyResult::Continue;
            }

            let _ = cmd_tx.try_send(ClientCommand::SendMessage {
                content: trimmed.clone(),
            });
            app.submit_input_inline(&trimmed);
            input.clear();
            *cursor_pos = 0;

            // End the current input line, then show the assistant prefix on a new line.
            print_newline_and_assistant_prefix().ok();
            InlineKeyResult::Submitted
        }
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if *cursor_pos > 0 {
                input.remove(*cursor_pos - 1);
                *cursor_pos -= 1;
            }
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Delete) => {
            if *cursor_pos < input.len() {
                input.remove(*cursor_pos);
            }
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Left) => {
            *cursor_pos = cursor_pos.saturating_sub(1);
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            if *cursor_pos < input.len() {
                *cursor_pos += 1;
            }
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Home) => {
            *cursor_pos = 0;
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::End) => {
            *cursor_pos = input.len();
            InlineKeyResult::Continue
        }
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
            input.clear();
            *cursor_pos = 0;
            InlineKeyResult::Continue
        }
        (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
            let new_pos = find_prev_word_boundary(input, *cursor_pos);
            input.drain(new_pos..*cursor_pos);
            *cursor_pos = new_pos;
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Up) => {
            if input.is_empty() || app.history_index.is_some() {
                app.history_up_inline(input);
                *cursor_pos = input.len();
            }
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            if app.history_index.is_some() {
                app.history_down_inline(input);
                *cursor_pos = input.len();
            }
            InlineKeyResult::Continue
        }
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            input.insert(*cursor_pos, c);
            *cursor_pos += 1;
            InlineKeyResult::Continue
        }
        _ => InlineKeyResult::Continue,
    }
}

fn find_prev_word_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut i = pos - 1;
    while i > 0 && bytes[i] == b' ' {
        i -= 1;
    }
    while i > 0 && bytes[i - 1] != b' ' {
        i -= 1;
    }
    i
}

// ─── Rendering helpers ────────────────────────────────────────────────

const PROMPT: &str = "You > ";

/// Erase the current line (for inserting server output above the prompt).
fn erase_input_line(stdout: &mut io::Stdout) -> io::Result<()> {
    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
    )
}

/// Redraw the input prompt + current input with the cursor in the right column.
///
/// Call this after every key event that modifies input so the user can see
/// what they're typing (raw mode suppresses OS-level echo).
fn redraw_input_line(stdout: &mut io::Stdout, input: &str, cursor_pos: usize) -> io::Result<()> {
    let col = (PROMPT.len() + cursor_pos) as u16;
    execute!(
        stdout,
        cursor::MoveToColumn(0),
        terminal::Clear(ClearType::CurrentLine),
        SetForegroundColor(RED),
        SetAttribute(Attribute::Bold),
        Print(PROMPT),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::Reset),
        Print(input),
        cursor::MoveToColumn(col),
    )
}

/// Print a newline to end the current input line, then show "xenoclaw > ".
fn print_newline_and_assistant_prefix() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        Print("\r\n"),
        SetForegroundColor(WHITE),
        SetAttribute(Attribute::Bold),
        Print("xenoclaw > "),
        SetAttribute(Attribute::Reset),
    )
}

fn print_tool_call(stdout: &mut io::Stdout, name: &str, args_summary: &str) -> io::Result<()> {
    execute!(
        stdout,
        SetForegroundColor(DIM),
        Print(format!("  \u{2699} {name}({args_summary})\n")),
        SetAttribute(Attribute::Reset),
    )
}

fn print_tool_result(
    stdout: &mut io::Stdout,
    name: &str,
    success: bool,
    summary: &str,
) -> io::Result<()> {
    let color = if success { GREEN } else { RED_BRIGHT };
    let prefix = if success { "\u{2713}" } else { "\u{2717}" };
    execute!(
        stdout,
        SetForegroundColor(color),
        SetAttribute(Attribute::Bold),
        Print(format!("  {prefix} {name}: {summary}\n")),
        SetAttribute(Attribute::Reset),
    )
}

fn print_system_message(stdout: &mut io::Stdout, message: &str) -> io::Result<()> {
    execute!(
        stdout,
        SetForegroundColor(RED_BRIGHT),
        SetAttribute(Attribute::Bold),
        Print(format!("\u{26A0} {message}\n")),
        SetAttribute(Attribute::Reset),
    )
}
