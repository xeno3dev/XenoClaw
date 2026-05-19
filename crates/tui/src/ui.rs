//! UI rendering for the TUI.
//!
//! Renders the terminal interface using Ratatui with the following layout:
//! - Top: Status bar (agent status, CPU%, memory MB, connection indicator, mode)
//! - Middle: Scrollable message history
//! - Bottom: Text input area with cursor
//!
//! Supports minimum 80x24 terminal dimensions and adapts to resize events.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{AgentStatus, App, ChatRole, ConnectionState, InteractionMode};

/// Draw the complete UI frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Status bar
            Constraint::Min(10),   // Message history
            Constraint::Length(3), // Input area
        ])
        .split(frame.area());

    draw_status_bar(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);

    // Draw disconnection banner if needed
    if let ConnectionState::Disconnected { reconnect_attempts, .. }
    | ConnectionState::Reconnecting { attempt: reconnect_attempts } = &app.connection_state
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
        AgentStatus::Idle => Span::styled("● IDLE", Style::default().fg(Color::Green)),
        AgentStatus::Working { task, progress } => {
            let progress_str = progress
                .as_ref()
                .map(|p| format!(" ({})", p))
                .unwrap_or_default();
            Span::styled(
                format!("◉ WORKING: {}{}", task, progress_str),
                Style::default().fg(Color::Yellow),
            )
        }
        AgentStatus::Error { message } => {
            Span::styled(
                format!("✗ ERROR: {}", message),
                Style::default().fg(Color::Red),
            )
        }
    };

    let connection_indicator = match &app.connection_state {
        ConnectionState::Connected => Span::styled("⚡ Connected", Style::default().fg(Color::Green)),
        ConnectionState::Disconnected { reconnect_attempts, .. } => Span::styled(
            format!("⚠ Disconnected (retry {}/{})", reconnect_attempts, app.max_reconnect_attempts),
            Style::default().fg(Color::Red),
        ),
        ConnectionState::Reconnecting { attempt } => Span::styled(
            format!("↻ Reconnecting ({}/{})", attempt, app.max_reconnect_attempts),
            Style::default().fg(Color::Yellow),
        ),
    };

    let mode_indicator = match app.mode {
        InteractionMode::General => Span::styled("[General]", Style::default().fg(Color::Cyan)),
        InteractionMode::Coding => Span::styled("[Coding]", Style::default().fg(Color::Magenta)),
    };

    let resource_info = Span::raw(format!(
        "CPU: {:.1}% | Mem: {:.0}MB",
        app.resource_usage.cpu_percent, app.resource_usage.memory_mb
    ));

    // Build status line with spacing
    let status_line = Line::from(vec![
        status_text,
        Span::raw(" │ "),
        resource_info,
        Span::raw(" │ "),
        connection_indicator,
        Span::raw(" │ "),
        mode_indicator,
    ]);

    let status_bar = Paragraph::new(status_line).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" XenoClaw Agent "),
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
        .take(area.height as usize - 2) // Account for borders
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|msg| {
            let (role_style, role_str) = match msg.role {
                ChatRole::User => (Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD), "You"),
                ChatRole::Assistant => (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD), "Agent"),
                ChatRole::System => (Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC), "System"),
            };

            let timestamp = msg.timestamp.format("%H:%M:%S");
            let header = Line::from(vec![
                Span::styled(role_str, role_style),
                Span::styled(
                    format!(" [{}]", timestamp),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let content = Line::from(Span::raw(&msg.content));

            ListItem::new(vec![header, content, Line::from("")])
        })
        .collect();

    let scroll_indicator = if app.scroll_offset > 0 {
        format!(" Messages (↑{} more) ", app.scroll_offset)
    } else {
        " Messages ".to_string()
    };

    let messages_widget = List::new(messages).block(
        Block::default()
            .borders(Borders::ALL)
            .title(scroll_indicator),
    );

    frame.render_widget(messages_widget, area);
}

/// Draw the text input area.
fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_text = if app.input.is_empty() {
        Text::styled(
            "Type a message... (F1 for help)",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Text::raw(&app.input)
    };

    let input_widget = Paragraph::new(input_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Input [{}] ", app.mode)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(input_widget, area);

    // Position cursor in input area
    if !app.input.is_empty() || true {
        // +1 for border
        let cursor_x = area.x + 1 + app.input_cursor as u16;
        let cursor_y = area.y + 1;
        if cursor_x < area.x + area.width - 1 {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

/// Draw a disconnection banner overlay.
fn draw_disconnection_banner(frame: &mut Frame, attempts: u8, max_attempts: u8) {
    let area = frame.area();
    // Center the banner
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
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Red)),
    )
    .wrap(Wrap { trim: false });

    frame.render_widget(Clear, banner_area);
    frame.render_widget(banner, banner_area);
}

/// Draw the help overlay.
fn draw_help_overlay(frame: &mut Frame) {
    let area = frame.area();
    let overlay_width = 60u16.min(area.width.saturating_sub(4));
    let overlay_height = 18u16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(overlay_width)) / 2;
    let y = (area.height.saturating_sub(overlay_height)) / 2;

    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    let help_text = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Enter       ", Style::default().fg(Color::Yellow)),
            Span::raw("Send message"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+C/Q    ", Style::default().fg(Color::Yellow)),
            Span::raw("Quit"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+K      ", Style::default().fg(Color::Yellow)),
            Span::raw("Cancel current task"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+M      ", Style::default().fg(Color::Yellow)),
            Span::raw("Switch mode (General/Coding)"),
        ]),
        Line::from(vec![
            Span::styled("  Up/Down     ", Style::default().fg(Color::Yellow)),
            Span::raw("Navigate input history / scroll"),
        ]),
        Line::from(vec![
            Span::styled("  PgUp/PgDn   ", Style::default().fg(Color::Yellow)),
            Span::raw("Scroll message history"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+U      ", Style::default().fg(Color::Yellow)),
            Span::raw("Clear input line"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+W      ", Style::default().fg(Color::Yellow)),
            Span::raw("Delete previous word"),
        ]),
        Line::from(vec![
            Span::styled("  F1 / ?      ", Style::default().fg(Color::Yellow)),
            Span::raw("Show this help"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help_widget = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help ")
            .style(Style::default().bg(Color::DarkGray)),
    );

    frame.render_widget(Clear, overlay_area);
    frame.render_widget(help_widget, overlay_area);
}
