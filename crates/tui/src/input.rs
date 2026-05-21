//! Keyboard input handling for the TUI.
//!
//! Maps key events to application actions. Supports:
//! - Enter: send message
//! - Ctrl+C / Ctrl+Q: quit
//! - Ctrl+K: cancel current task
//! - Up/Down: scroll history (when input empty) or navigate input history
//! - Ctrl+M: mode switching (General/Coding)
//! - F1 or ?: show help overlay
//! - Standard text editing (backspace, delete, left/right cursor movement)

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::app::App;
use crate::client::ClientCommand;
use crate::slash;

/// Process a key event and update application state accordingly.
///
/// `cmd_tx` is optional — pass `Some` in the live UI to forward submitted
/// messages and slash-command network actions to the WS task; pass `None`
/// in unit tests.
pub fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
    cmd_tx: Option<&mpsc::Sender<ClientCommand>>,
) {
    // If help overlay is showing, any key dismisses it
    if app.show_help {
        app.show_help = false;
        return;
    }

    match (key.modifiers, key.code) {
        // Quit: Ctrl+C or Ctrl+Q
        (KeyModifiers::CONTROL, KeyCode::Char('c'))
        | (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
            app.should_quit = true;
        }

        // Cancel task: Ctrl+K
        (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
            app.cancel_task();
        }

        // Mode switching: Ctrl+M
        (KeyModifiers::CONTROL, KeyCode::Char('m')) => {
            app.toggle_mode();
        }

        // Help overlay: F1
        (_, KeyCode::F(1)) => {
            app.show_help = true;
        }

        // Submit message: Enter (with slash command interception)
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Enter) => {
            let content = app.input.trim().to_string();
            if content.is_empty() {
                return;
            }

            if let Some(cmd) = slash::parse(&content) {
                app.input.clear();
                app.input_cursor = 0;
                if let Some(tx) = cmd_tx {
                    // Full slash::execute handles all commands including Reconnect.
                    slash::execute(cmd, app, tx);
                } else {
                    // No channel (tests / offline): handle local-only commands.
                    match cmd {
                        slash::SlashCommand::Exit => app.should_quit = true,
                        slash::SlashCommand::Clear => app.clear_screen(),
                        slash::SlashCommand::Status => app.show_status(),
                        slash::SlashCommand::Help => app.show_help = true,
                        slash::SlashCommand::Mode => app.toggle_mode(),
                        slash::SlashCommand::Reconnect | slash::SlashCommand::Unknown(_) => {
                            app.add_system_message(
                                "This command requires a live connection.".to_string(),
                            );
                        }
                    }
                }
            } else {
                // Save content before submit_input() clears it.
                app.submit_input();
                if let Some(tx) = cmd_tx {
                    let _ = tx.try_send(ClientCommand::SendMessage { content });
                }
            }
        }

        // History navigation: Up arrow
        (KeyModifiers::NONE, KeyCode::Up) => {
            if app.input.is_empty() || app.history_index.is_some() {
                app.history_up();
            } else {
                app.scroll_up();
            }
        }

        // History navigation: Down arrow
        (KeyModifiers::NONE, KeyCode::Down) => {
            if app.history_index.is_some() {
                app.history_down();
            } else {
                app.scroll_down();
            }
        }

        // Page Up: scroll message history
        (_, KeyCode::PageUp) => {
            for _ in 0..5 {
                app.scroll_up();
            }
        }

        // Page Down: scroll message history
        (_, KeyCode::PageDown) => {
            for _ in 0..5 {
                app.scroll_down();
            }
        }

        // Home: scroll to top
        (KeyModifiers::CONTROL, KeyCode::Home) => {
            app.scroll_offset = app.messages.len().saturating_sub(1);
        }

        // End: scroll to bottom
        (KeyModifiers::CONTROL, KeyCode::End) => {
            app.scroll_offset = 0;
        }

        // Help overlay: '?' character (only when input is empty)
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char('?')) if app.input.is_empty() => {
            app.show_help = true;
        }

        // Text input: regular characters
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.input.insert(app.input_cursor, c);
            app.input_cursor += 1;
        }

        // Backspace: delete character before cursor
        (_, KeyCode::Backspace) => {
            if app.input_cursor > 0 {
                app.input_cursor -= 1;
                app.input.remove(app.input_cursor);
            }
        }

        // Delete: delete character at cursor
        (_, KeyCode::Delete) => {
            if app.input_cursor < app.input.len() {
                app.input.remove(app.input_cursor);
            }
        }

        // Left arrow: move cursor left
        (KeyModifiers::NONE, KeyCode::Left) => {
            app.input_cursor = app.input_cursor.saturating_sub(1);
        }

        // Right arrow: move cursor right
        (KeyModifiers::NONE, KeyCode::Right) => {
            if app.input_cursor < app.input.len() {
                app.input_cursor += 1;
            }
        }

        // Ctrl+Left: move cursor to previous word boundary
        (KeyModifiers::CONTROL, KeyCode::Left) => {
            app.input_cursor = find_prev_word_boundary(&app.input, app.input_cursor);
        }

        // Ctrl+Right: move cursor to next word boundary
        (KeyModifiers::CONTROL, KeyCode::Right) => {
            app.input_cursor = find_next_word_boundary(&app.input, app.input_cursor);
        }

        // Home: move cursor to start of input
        (KeyModifiers::NONE, KeyCode::Home) => {
            app.input_cursor = 0;
        }

        // End: move cursor to end of input
        (KeyModifiers::NONE, KeyCode::End) => {
            app.input_cursor = app.input.len();
        }

        // Ctrl+U: clear input line
        (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
            app.input.clear();
            app.input_cursor = 0;
        }

        // Ctrl+W: delete previous word
        (KeyModifiers::CONTROL, KeyCode::Char('w')) => {
            let new_pos = find_prev_word_boundary(&app.input, app.input_cursor);
            app.input.drain(new_pos..app.input_cursor);
            app.input_cursor = new_pos;
        }

        _ => {}
    }
}

/// Find the previous word boundary from the given position.
fn find_prev_word_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut i = pos - 1;

    // Skip whitespace
    while i > 0 && bytes[i] == b' ' {
        i -= 1;
    }

    // Skip word characters
    while i > 0 && bytes[i - 1] != b' ' {
        i -= 1;
    }

    i
}

/// Find the next word boundary from the given position.
fn find_next_word_boundary(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    let bytes = text.as_bytes();
    let mut i = pos;

    // Skip current word characters
    while i < len && bytes[i] != b' ' {
        i += 1;
    }

    // Skip whitespace
    while i < len && bytes[i] == b' ' {
        i += 1;
    }

    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::TuiConfig;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn make_key(modifiers: KeyModifiers, code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn test_app() -> App {
        App::new(TuiConfig::default())
    }

    #[test]
    fn test_quit_ctrl_c() {
        let mut app = test_app();
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::CONTROL, KeyCode::Char('c')),
            None,
        );
        assert!(app.should_quit);
    }

    #[test]
    fn test_quit_ctrl_q() {
        let mut app = test_app();
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::CONTROL, KeyCode::Char('q')),
            None,
        );
        assert!(app.should_quit);
    }

    #[test]
    fn test_char_input() {
        let mut app = test_app();
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::NONE, KeyCode::Char('h')),
            None,
        );
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::NONE, KeyCode::Char('i')),
            None,
        );
        assert_eq!(app.input, "hi");
        assert_eq!(app.input_cursor, 2);
    }

    #[test]
    fn test_backspace() {
        let mut app = test_app();
        app.input = "hello".to_string();
        app.input_cursor = 5;
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::NONE, KeyCode::Backspace),
            None,
        );
        assert_eq!(app.input, "hell");
        assert_eq!(app.input_cursor, 4);
    }

    #[test]
    fn test_enter_submits() {
        let mut app = test_app();
        app.input = "test message".to_string();
        app.input_cursor = 12;
        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::Enter), None);
        assert!(app.input.is_empty());
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn test_ctrl_k_cancels_task() {
        let mut app = test_app();
        app.agent_status = crate::app::AgentStatus::Working {
            task: "test".to_string(),
            progress: None,
        };
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::CONTROL, KeyCode::Char('k')),
            None,
        );
        assert_eq!(app.agent_status, crate::app::AgentStatus::Idle);
    }

    #[test]
    fn test_ctrl_m_toggles_mode() {
        let mut app = test_app();
        assert_eq!(app.mode, crate::app::InteractionMode::General);
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::CONTROL, KeyCode::Char('m')),
            None,
        );
        assert_eq!(app.mode, crate::app::InteractionMode::Coding);
    }

    #[test]
    fn test_f1_shows_help() {
        let mut app = test_app();
        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::F(1)), None);
        assert!(app.show_help);
    }

    #[test]
    fn test_help_dismissed_by_any_key() {
        let mut app = test_app();
        app.show_help = true;
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::NONE, KeyCode::Char('a')),
            None,
        );
        assert!(!app.show_help);
    }

    #[test]
    fn test_cursor_movement() {
        let mut app = test_app();
        app.input = "hello".to_string();
        app.input_cursor = 3;

        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::Left), None);
        assert_eq!(app.input_cursor, 2);

        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::Right), None);
        assert_eq!(app.input_cursor, 3);

        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::Home), None);
        assert_eq!(app.input_cursor, 0);

        handle_key_event(&mut app, make_key(KeyModifiers::NONE, KeyCode::End), None);
        assert_eq!(app.input_cursor, 5);
    }

    #[test]
    fn test_ctrl_u_clears_input() {
        let mut app = test_app();
        app.input = "some text".to_string();
        app.input_cursor = 5;
        handle_key_event(
            &mut app,
            make_key(KeyModifiers::CONTROL, KeyCode::Char('u')),
            None,
        );
        assert!(app.input.is_empty());
        assert_eq!(app.input_cursor, 0);
    }

    #[test]
    fn test_word_boundary_navigation() {
        assert_eq!(find_prev_word_boundary("hello world", 11), 6);
        assert_eq!(find_prev_word_boundary("hello world", 6), 0);
        assert_eq!(find_next_word_boundary("hello world", 0), 6);
        assert_eq!(find_next_word_boundary("hello world", 6), 11);
    }
}
