use ratatui::style::Color;

/// Primary brand red — headers, cursors, active selections.
pub const RED: Color = Color::Rgb(255, 56, 56);
/// Bright crimson for emphasis.
pub const RED_BRIGHT: Color = Color::Rgb(255, 96, 96);
/// Deep blood-red for backgrounds of error/warning callouts.
pub const RED_DEEP: Color = Color::Rgb(120, 20, 20);
/// Pure white — body text, values, key contents.
pub const WHITE: Color = Color::Rgb(240, 240, 240);
/// Soft white-grey — secondary labels, hints.
pub const TEXT: Color = Color::Rgb(200, 200, 200);
/// Medium grey — inactive menu items, dim text.
pub const DIM: Color = Color::Rgb(130, 130, 135);
/// Dark grey — horizontal rules, borders.
pub const RULE_FG: Color = Color::Rgb(75, 75, 80);
/// Success green — checkmarks ONLY (sparingly).
pub const GREEN: Color = Color::Rgb(80, 220, 100);
/// Amber — warnings, callouts.
pub const AMBER: Color = Color::Rgb(255, 176, 0);
/// Charcoal background.
pub const BG: Color = Color::Rgb(18, 18, 22);

/// Returns true if backgrounds should be painted.
///
/// Disabled when:
/// - `NO_COLOR` env var is set
/// - `TERM=dumb` — terminal has no color support
/// - Running over SSH (`SSH_CONNECTION` or `SSH_TTY` set)
///
/// FG colors are still emitted. Users can override with `XENOCLAW_FORCE_BG=1`.
pub fn bg_enabled() -> bool {
    if std::env::var_os("XENOCLAW_FORCE_BG").is_some() {
        return true;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Ok(term) = std::env::var("TERM") {
        if term == "dumb" || term.is_empty() {
            return false;
        }
    }
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return false;
    }
    true
}
