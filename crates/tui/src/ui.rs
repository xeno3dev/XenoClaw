use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{AgentStatus, App, ChatRole, ConnectionState, InteractionMode};
use crate::theme;

/// Draw the complete UI frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Status bar
            Constraint::Min(10),   // Message history
            Constraint::Length(3), // Input area
        ])
        .split(frame.area());

    draw_status_bar(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);

    // Draw disconnection banner if needed
    if let ConnectionState::Disconnected {
        reconnect_attempts, ..
    }
    | ConnectionState::Reconnecting {
        attempt: reconnect_attempts,
    } = &app.connection_state
    {
        draw_disconnection_banner(frame, *reconnect_attempts, app.max_reconnect_attempts);
    }

    // Draw help overlay if active
    if app.show_help {
        draw_help_overlay(frame);
    }
}

/// Draw the status bar at the top.
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let status_text = match &app.agent_status {
        AgentStatus::Idle => Span::styled("● IDLE", Style::default().fg(theme::GREEN)),
        AgentStatus::Working { task, progress } => {
            let progress_str = progress
                .as_ref()
                .map(|p| format!(" ({})", p))
                .unwrap_or_default();
            Span::styled(
                format!("● WORKING: {}{}", task, progress_str),
                Style::default().fg(theme::AMBER),
            )
        }
        AgentStatus::Error { message } => Span::styled(
            format!("✗ ERROR: {}", message),
            Style::default().fg(theme::RED_BRIGHT),
        ),
    };

    let connection_indicator = match &app.connection_state {
        ConnectionState::Connected => {
            Span::styled("● Connected", Style::default().fg(theme::GREEN))
        }
        ConnectionState::Disconnected {
            reconnect_attempts, ..
        } => Span::styled(
            format!(
                "⚠ Disconnected (retry {}/{})",
                reconnect_attempts, app.max_reconnect_attempts
            ),
            Style::default().fg(theme::RED_BRIGHT),
        ),
        ConnectionState::Reconnecting { attempt } => Span::styled(
            format!(
                "↻ Reconnecting ({}/{})",
                attempt, app.max_reconnect_attempts
            ),
            Style::default().fg(theme::AMBER),
        ),
    };

    let mode_indicator = match app.mode {
        InteractionMode::General => Span::styled("[General]", Style::default().fg(theme::TEXT)),
        InteractionMode::Coding => Span::styled("[Coding]", Style::default().fg(theme::RED)),
    };

    let resource_info = Span::raw(format!(
        "CPU: {:.1}% | Mem: {:.0}MB",
        app.resource_usage.cpu_percent, app.resource_usage.memory_mb
    ));

    let status_line = Line::from(vec![
        status_text,
        Span::raw(" │ "),
        resource_info,
        Span::raw(" │ "),
        connection_indicator,
        Span::raw(" │ "),
        mode_indicator,
    ]);

    let border_style = Style::default().fg(theme::RED);
    let title_style = Style::default().fg(theme::RED).add_modifier(Modifier::BOLD);

    let bg_style = if theme::bg_enabled() {
        Style::default().bg(theme::BG)
    } else {
        Style::default()
    };

    let status_bar = Paragraph::new(status_line).style(bg_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(Span::styled(" XenoClaw Agent ", title_style))),
    );

    frame.render_widget(status_bar, area);
}

/// Draw the scrollable message history.
fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let messages: Vec<ListItem> = app
        .messages
        .iter()
        .rev()
        .skip(app.scroll_offset)
        .take(area.height as usize - 2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|msg| {
            let (role_style, role_str) = match msg.role {
                ChatRole::User => (
                    Style::default().fg(theme::RED).add_modifier(Modifier::BOLD),
                    "You >",
                ),
                ChatRole::Assistant => (
                    Style::default()
                        .fg(theme::WHITE)
                        .add_modifier(Modifier::BOLD),
                    "xenoclaw >",
                ),
                ChatRole::System => (
                    Style::default()
                        .fg(theme::AMBER)
                        .add_modifier(Modifier::ITALIC),
                    "System",
                ),
            };

            let timestamp = msg.timestamp.format("%H:%M:%S");
            let header = Line::from(vec![
                Span::styled(role_str, role_style),
                Span::styled(format!(" [{}]", timestamp), Style::default().fg(theme::DIM)),
            ]);

            let content = Line::from(Span::raw(&msg.content));

            ListItem::new(vec![header, content, Line::from("")])
        })
        .collect();

    let scroll_indicator = if app.scroll_offset > 0 {
        format!(" Messages ({} more) ", app.scroll_offset)
    } else {
        " Messages ".to_string()
    };

    let border_style = Style::default().fg(theme::RULE_FG);
    let title_style = Style::default().fg(theme::DIM);

    let bg_style = if theme::bg_enabled() {
        Style::default().bg(theme::BG)
    } else {
        Style::default()
    };

    let messages_widget = List::new(messages).style(bg_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Line::from(Span::styled(scroll_indicator, title_style))),
    );

    frame.render_widget(messages_widget, area);
}

/// Draw the text input area.
fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_text = if app.input.is_empty() {
        Text::styled(
            "Type a message...  (/help for commands)",
            Style::default().fg(theme::DIM),
        )
    } else {
        Text::raw(&app.input)
    };

    let border_style = Style::default().fg(theme::RED);
    let title_style = Style::default().fg(theme::RED).add_modifier(Modifier::BOLD);

    let bg_style = if theme::bg_enabled() {
        Style::default().bg(theme::BG)
    } else {
        Style::default()
    };

    let input_widget = Paragraph::new(input_text)
        .style(bg_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(Line::from(Span::styled(
                    format!(" Input [{}] ", app.mode),
                    title_style,
                ))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(input_widget, area);

    // Position cursor in input area
    let cursor_x = area.x + 1 + app.input_cursor as u16;
    let cursor_y = area.y + 1;
    if cursor_x < area.x + area.width - 1 {
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

/// Draw a disconnection banner overlay.
fn draw_disconnection_banner(frame: &mut Frame, attempts: u8, max_attempts: u8) {
    let area = frame.area();
    let banner_width = 50u16.min(area.width.saturating_sub(4));
    let banner_height = 3u16;
    let x = (area.width.saturating_sub(banner_width)) / 2;
    let y = area.height / 3;

    let banner_area = Rect::new(x, y, banner_width, banner_height);

    let message = if attempts >= max_attempts {
        "Connection lost. Max reconnect attempts reached."
    } else {
        "Connection lost. Reconnecting..."
    };

    let banner = Paragraph::new(Line::from(Span::styled(
        message,
        Style::default()
            .fg(theme::WHITE)
            .add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::RED))
            .style(Style::default().bg(theme::RED_DEEP)),
    )
    .wrap(Wrap { trim: false });

    frame.render_widget(Clear, banner_area);
    frame.render_widget(banner, banner_area);
}

/// Draw the help overlay.
fn draw_help_overlay(frame: &mut Frame) {
    let area = frame.area();
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    let overlay_height = 22u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    let help_text = vec![
        Line::from(Span::styled(
            " Keyboard Shortcuts ",
            Style::default().add_modifier(Modifier::BOLD).fg(theme::RED),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter       ", Style::default().fg(theme::AMBER)),
            Span::raw("Send message"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+C/Q    ", Style::default().fg(theme::AMBER)),
            Span::raw("Quit"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+K      ", Style::default().fg(theme::AMBER)),
            Span::raw("Cancel current task"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+M      ", Style::default().fg(theme::AMBER)),
            Span::raw("Switch mode (General/Coding)"),
        ]),
        Line::from(vec![
            Span::styled("  Up/Down     ", Style::default().fg(theme::AMBER)),
            Span::raw("Navigate input history / scroll"),
        ]),
        Line::from(vec![
            Span::styled("  PgUp/PgDn   ", Style::default().fg(theme::AMBER)),
            Span::raw("Scroll message history"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+U      ", Style::default().fg(theme::AMBER)),
            Span::raw("Clear input line"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+W      ", Style::default().fg(theme::AMBER)),
            Span::raw("Delete previous word"),
        ]),
        Line::from(vec![
            Span::styled("  F1 / ?      ", Style::default().fg(theme::AMBER)),
            Span::raw("Show this help"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            " Slash Commands ",
            Style::default().add_modifier(Modifier::BOLD).fg(theme::RED),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  /exit, /quit", Style::default().fg(theme::AMBER)),
            Span::raw("  Exit the TUI"),
        ]),
        Line::from(vec![
            Span::styled("  /clear      ", Style::default().fg(theme::AMBER)),
            Span::raw("  Clear screen"),
        ]),
        Line::from(vec![
            Span::styled("  /status     ", Style::default().fg(theme::AMBER)),
            Span::raw("  Show agent status"),
        ]),
        Line::from(vec![
            Span::styled("  /help       ", Style::default().fg(theme::AMBER)),
            Span::raw("  Show this help"),
        ]),
        Line::from(vec![
            Span::styled("  /reconnect  ", Style::default().fg(theme::AMBER)),
            Span::raw("  Force reconnection"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(theme::DIM),
        )),
    ];

    let bg_style = if theme::bg_enabled() {
        Style::default().bg(theme::BG)
    } else {
        Style::default()
    };

    let help_widget = Paragraph::new(help_text).style(bg_style).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::RED))
            .title(" Help ")
            .title_style(Style::default().fg(theme::RED).add_modifier(Modifier::BOLD)),
    );

    frame.render_widget(Clear, overlay_area);
    frame.render_widget(help_widget, overlay_area);
}
