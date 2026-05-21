use tokio::sync::mpsc;

use crate::app::App;
use crate::client::ClientCommand;

/// Parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Exit,
    Clear,
    Status,
    Help,
    Reconnect,
    Mode,
    Unknown(String),
}

/// Parse a slash command from user input.
///
/// Returns `None` if the input does not start with `/`.
pub fn parse(input: &str) -> Option<SlashCommand> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let cmd = trimmed[1..].trim().to_lowercase();
    let cmd = cmd.split_whitespace().next().unwrap_or("");

    match cmd {
        "exit" | "quit" | "q" => Some(SlashCommand::Exit),
        "clear" | "cls" => Some(SlashCommand::Clear),
        "status" | "st" => Some(SlashCommand::Status),
        "help" | "h" | "?" => Some(SlashCommand::Help),
        "reconnect" | "recon" => Some(SlashCommand::Reconnect),
        "mode" | "m" => Some(SlashCommand::Mode),
        other => Some(SlashCommand::Unknown(other.to_string())),
    }
}

/// Execute a slash command against the app state.
///
/// Returns `true` if the app should continue running, `false` if it should exit.
pub fn execute(cmd: SlashCommand, app: &mut App, tx: &mpsc::Sender<ClientCommand>) -> bool {
    match cmd {
        SlashCommand::Exit => {
            app.should_quit = true;
            false
        }
        SlashCommand::Clear => {
            app.clear_screen();
            true
        }
        SlashCommand::Status => {
            app.show_status();
            true
        }
        SlashCommand::Help => {
            app.show_help = true;
            true
        }
        SlashCommand::Reconnect => {
            let _ = tx.try_send(ClientCommand::Reconnect);
            app.add_system_message("Reconnecting...".to_string());
            true
        }
        SlashCommand::Mode => {
            app.toggle_mode();
            true
        }
        SlashCommand::Unknown(name) => {
            app.add_system_message(format!(
                "Unknown command: /{name}. Type /help for available commands."
            ));
            true
        }
    }
}

/// Get the help text for slash commands.
pub fn help_text() -> &'static [(&'static str, &'static str)] {
    &[
        ("/exit, /quit, /q", "Exit the TUI"),
        ("/clear, /cls", "Clear the screen"),
        ("/status, /st", "Show current agent status"),
        ("/help, /h, /?", "Show this help"),
        ("/reconnect, /recon", "Force reconnection"),
        ("/mode, /m", "Toggle General/Coding mode"),
    ]
}
