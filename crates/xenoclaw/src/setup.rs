//! First-run setup wizard for XenoClaw.
//!
//! An inline terminal wizard using crossterm's alternate screen.
//!
//! ## Rendering strategy
//!
//! The wizard adapts to terminal capabilities to look correct everywhere:
//!
//! - **True-color terminals** (`COLORTERM=truecolor|24bit`): 24-bit RGB palette
//!   for exact brand colors (red #FF3838, charcoal #121212, etc.).
//! - **256-color terminals** (SSH, web consoles, tmux, PuTTY): all colors fall
//!   back to the nearest ANSI 256-color indexed equivalents using `\x1b[38;5;Nm`
//!   sequences, which every xterm-compatible terminal handles reliably.
//! - **Dumb terminals** (`TERM=dumb`, `NO_COLOR`): BG fills are skipped; FG
//!   colors still use 256-color; text remains legible on any background.
//! - **Narrow terminals (< 80 cols)**: the big ASCII banner is replaced with
//!   a compact `[ XENOCLAW ]` block so nothing wraps.
//!
//! All text is laid out using `unicode-width` for emoji-correct padding.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    QueueableCommand,
};
use unicode_width::UnicodeWidthStr;

use security_layer::auth::ApiKeyAuthenticator;

use crate::cli_health::{CliHealth, CliType};

// ─── Color Palette — Xeno Brand (X3NO: black, red, charcoal accents) ─────────
//
// Three-tier color selection, detected once at startup:
//
//   Tier 1 — TrueColor  (COLORTERM=truecolor|24bit)
//     Exact 24-bit RGB.  Kitty, Alacritty, WezTerm, modern xterm.
//
//   Tier 2 — Color256  (TERM contains "256color")
//     Nearest ANSI 256-color index (\x1b[38;5;Nm).
//     Every xterm-256color terminal including SSH forwarding and web consoles
//     that set TERM=xterm-256color.
//
//   Tier 3 — Color16  (everything else: TERM=xterm, linux, vt100, etc.)
//     Basic 8/16-color named constants (\x1b[31m etc.).
//     Works on every terminal without exception — Proxmox pct enter, mosh,
//     old SSH, serial consoles, and anywhere 256-color isn't advertised.
//     BG fill is also skipped at this tier (AnsiValue won't render reliably).

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorDepth {
    TrueColor,
    Color256,
    Color16,
}

static COLOR_DEPTH: OnceLock<ColorDepth> = OnceLock::new();

fn color_depth() -> ColorDepth {
    *COLOR_DEPTH.get_or_init(|| {
        if matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor") | Ok("24bit")
        ) {
            return ColorDepth::TrueColor;
        }
        if let Ok(term) = std::env::var("TERM") {
            if term.contains("256color") {
                return ColorDepth::Color256;
            }
        }
        ColorDepth::Color16
    })
}

fn red() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 255, g: 56, b: 56 },
        ColorDepth::Color256 => Color::AnsiValue(196),
        ColorDepth::Color16 => Color::Red,
    }
}

fn red_bright() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 255, g: 96, b: 96 },
        ColorDepth::Color256 => Color::AnsiValue(203),
        ColorDepth::Color16 => Color::Red,
    }
}

fn red_deep() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 120, g: 20, b: 20 },
        ColorDepth::Color256 => Color::AnsiValue(88),
        ColorDepth::Color16 => Color::DarkRed,
    }
}

fn white() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 240, g: 240, b: 240 },
        ColorDepth::Color256 => Color::AnsiValue(255),
        ColorDepth::Color16 => Color::White,
    }
}

fn text() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 200, g: 200, b: 200 },
        ColorDepth::Color256 => Color::AnsiValue(251),
        ColorDepth::Color16 => Color::Grey,
    }
}

fn dim() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 130, g: 130, b: 135 },
        ColorDepth::Color256 => Color::AnsiValue(244),
        ColorDepth::Color16 => Color::DarkGrey,
    }
}

fn rule_fg() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 75, g: 75, b: 80 },
        ColorDepth::Color256 => Color::AnsiValue(239),
        ColorDepth::Color16 => Color::DarkGrey,
    }
}

fn green() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 80, g: 220, b: 100 },
        ColorDepth::Color256 => Color::AnsiValue(83),
        ColorDepth::Color16 => Color::Green,
    }
}

fn amber() -> Color {
    match color_depth() {
        ColorDepth::TrueColor => Color::Rgb { r: 255, g: 176, b: 0 },
        ColorDepth::Color256 => Color::AnsiValue(214),
        ColorDepth::Color16 => Color::Yellow,
    }
}

/// Charcoal background — ANSI 256-color #233 (#121212).
/// Only used when color256 or truecolor is available; bg_enabled() returns
/// false on Color16 terminals where AnsiValue sequences don't render.
const BG: Color = Color::AnsiValue(233);

const TOTAL_STEPS: u8 = 9;
const RULE: &str = "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━";

// ─── Provider Definitions ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ProviderDef {
    name: &'static str,
    provider_type: &'static str,
    default_model: &'static str,
    default_base_url: &'static str,
    needs_api_key: bool,
}

const PROVIDERS: &[ProviderDef] = &[
    ProviderDef {
        name: "Anthropic",
        provider_type: "anthropic",
        default_model: "claude-sonnet-4-20250514",
        default_base_url: "https://api.anthropic.com",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Google Gemini",
        provider_type: "open_ai_compatible",
        default_model: "gemini-2.5-pro",
        default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        needs_api_key: true,
    },
    ProviderDef {
        name: "OpenAI",
        provider_type: "open_ai_compatible",
        default_model: "gpt-4o",
        default_base_url: "https://api.openai.com/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "AWS Bedrock",
        provider_type: "open_ai_compatible",
        default_model: "anthropic.claude-sonnet-4-20250514-v1:0",
        default_base_url: "https://bedrock-runtime.us-east-1.amazonaws.com",
        needs_api_key: true,
    },
    ProviderDef {
        name: "OpenRouter",
        provider_type: "open_ai_compatible",
        default_model: "anthropic/claude-sonnet-4-20250514",
        default_base_url: "https://openrouter.ai/api/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Together AI",
        provider_type: "open_ai_compatible",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        default_base_url: "https://api.together.xyz/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Mistral AI",
        provider_type: "open_ai_compatible",
        default_model: "mistral-large-latest",
        default_base_url: "https://api.mistral.ai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Fireworks AI",
        provider_type: "open_ai_compatible",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        default_base_url: "https://api.fireworks.ai/inference/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "DeepSeek",
        provider_type: "open_ai_compatible",
        default_model: "deepseek-chat",
        default_base_url: "https://api.deepseek.com/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Groq",
        provider_type: "open_ai_compatible",
        default_model: "llama-3.3-70b-versatile",
        default_base_url: "https://api.groq.com/openai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "xAI",
        provider_type: "open_ai_compatible",
        default_model: "grok-3",
        default_base_url: "https://api.x.ai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Perplexity",
        provider_type: "open_ai_compatible",
        default_model: "sonar-pro",
        default_base_url: "https://api.perplexity.ai",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Cohere",
        provider_type: "open_ai_compatible",
        default_model: "command-r-plus",
        default_base_url: "https://api.cohere.com/v2",
        needs_api_key: true,
    },
    ProviderDef {
        name: "AI21 Labs",
        provider_type: "open_ai_compatible",
        default_model: "jamba-1.5-large",
        default_base_url: "https://api.ai21.com/studio/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Hugging Face",
        provider_type: "open_ai_compatible",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        default_base_url: "https://api-inference.huggingface.co/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Replicate",
        provider_type: "open_ai_compatible",
        default_model: "meta/llama-3.3-70b-instruct",
        default_base_url: "https://api.replicate.com/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Requesty",
        provider_type: "open_ai_compatible",
        default_model: "anthropic/claude-sonnet-4-20250514",
        default_base_url: "https://router.requesty.ai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Cerebras",
        provider_type: "open_ai_compatible",
        default_model: "llama-3.3-70b",
        default_base_url: "https://api.cerebras.ai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "SambaNova",
        provider_type: "open_ai_compatible",
        default_model: "Meta-Llama-3.3-70B-Instruct",
        default_base_url: "https://api.sambanova.ai/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Ollama",
        provider_type: "ollama",
        default_model: "llama3.2",
        default_base_url: "http://localhost:11434",
        needs_api_key: false,
    },
    ProviderDef {
        name: "vLLM",
        provider_type: "open_ai_compatible",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        default_base_url: "http://localhost:8000/v1",
        needs_api_key: false,
    },
    ProviderDef {
        name: "LM Studio",
        provider_type: "open_ai_compatible",
        default_model: "local-model",
        default_base_url: "http://localhost:1234/v1",
        needs_api_key: false,
    },
    ProviderDef {
        name: "Qwen",
        provider_type: "open_ai_compatible",
        default_model: "qwen-max",
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "MiniMax",
        provider_type: "open_ai_compatible",
        default_model: "MiniMax-Text-01",
        default_base_url: "https://api.minimax.chat/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Zhipu AI",
        provider_type: "open_ai_compatible",
        default_model: "glm-4-plus",
        default_base_url: "https://open.bigmodel.cn/api/paas/v4",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Moonshot AI",
        provider_type: "open_ai_compatible",
        default_model: "moonshot-v1-128k",
        default_base_url: "https://api.moonshot.cn/v1",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Baidu Qianfan",
        provider_type: "open_ai_compatible",
        default_model: "ernie-4.0-8k",
        default_base_url: "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop",
        needs_api_key: true,
    },
    ProviderDef {
        name: "Claude Code CLI",
        provider_type: "claude_code",
        default_model: "claude-sonnet-4-20250514",
        default_base_url: "",
        needs_api_key: false,
    },
    ProviderDef {
        name: "GitHub Copilot CLI",
        provider_type: "copilot_cli",
        default_model: "gpt-4o",
        default_base_url: "",
        needs_api_key: false,
    },
    ProviderDef {
        name: "Gemini CLI",
        provider_type: "gemini_cli",
        default_model: "gemini-2.5-pro",
        default_base_url: "",
        needs_api_key: false,
    },
    ProviderDef {
        name: "OpenAI Codex CLI",
        provider_type: "codex_cli",
        default_model: "codex-mini-latest",
        default_base_url: "",
        needs_api_key: false,
    },
];

// ─── Wizard State ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct WizardState {
    provider_idx: usize,
    api_key: String,
    base_url: String,
    model: String,
    workspace_dir: String,
    create_workspace: bool,
    host: String,
    port: String,
    web_port: String,
    require_auth: bool,
    admin_username: String,
    admin_password: String,
    admin_password_hash: String,
    admin_key_raw: String,
    admin_key_hash: String,
    sandbox_commands: Vec<(String, bool)>,
    custom_commands: String,
    excluded_commands: String,
    allow_all_commands: bool,
    allow_pipes: bool,
    tool_timeout: String,
    timeout_unit: TimeoutUnit,
    /// Max file size (MB) for the coding agent's file ops. Default 10.
    max_file_size_mb: String,
    /// Max concurrent shell processes. Default 5.
    max_concurrent_shells: String,
    /// Number of file operations to keep in the undo history. Default 50.
    undo_history_size: String,
    /// Telegram bot token (BotFather). Empty disables Telegram.
    telegram_bot_token: String,
    /// Discord bot token (Developer Portal). Empty disables Discord.
    discord_bot_token: String,
    /// WhatsApp phone number (E.164, e.g. +14155552671). Empty disables.
    whatsapp_phone: String,
    /// CLI providers only: false when the CLI is not installed/logged-in
    /// and the user chose to skip — disables "Start server" on the done screen.
    cli_provider_ready: bool,
    /// Claude Code only: true once xenoclaw has been injected into
    /// ~/.claude/settings.json as an MCP server.
    mcp_registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TimeoutUnit {
    Seconds,
    Minutes,
    Hours,
}

impl Default for WizardState {
    fn default() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let workspace = home.join(".xenoclaw").join("workspace");

        Self {
            provider_idx: 0,
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            workspace_dir: workspace.to_string_lossy().to_string(),
            create_workspace: false,
            host: "127.0.0.1".to_string(),
            port: "9090".to_string(),
            web_port: "8080".to_string(),
            require_auth: true,
            admin_username: "admin".to_string(),
            admin_password: String::new(),
            admin_password_hash: String::new(),
            admin_key_raw: String::new(),
            admin_key_hash: String::new(),
            sandbox_commands: vec![
                ("git".to_string(), false),
                ("cargo".to_string(), false),
                ("python3".to_string(), false),
                ("npm".to_string(), false),
                ("node".to_string(), false),
                ("curl".to_string(), false),
                ("wget".to_string(), false),
                ("docker".to_string(), false),
                ("make".to_string(), false),
                ("ssh".to_string(), false),
            ],
            custom_commands: String::new(),
            excluded_commands: String::new(),
            allow_all_commands: false,
            allow_pipes: false,
            tool_timeout: "30".to_string(),
            timeout_unit: TimeoutUnit::Seconds,
            max_file_size_mb: "10".to_string(),
            max_concurrent_shells: "5".to_string(),
            undo_history_size: "50".to_string(),
            telegram_bot_token: String::new(),
            discord_bot_token: String::new(),
            whatsapp_phone: String::new(),
            cli_provider_ready: true,
            mcp_registered: false,
        }
    }
}

// ─── Banner ──────────────────────────────────────────────────────────────────
//
// The big banner uses uniform-width ASCII block letters. Each glyph is exactly
// 8 cells wide and 6 rows tall; with 8 letters that's 64 cells total, fitting
// in any terminal ≥ 70 cols (we use 80 as the threshold to leave breathing
// room). The pieces are assembled at render time so we can verify width.

const BANNER_LINES: [&str; 8] = [
    r" /$$   /$$                                /$$$$$$  /$$                        ",
    r"| $$  / $$                               /$$__  $$| $$                        ",
    r"|  $$/ $$/  /$$$$$$  /$$$$$$$   /$$$$$$ | $$  \__/| $$  /$$$$$$  /$$  /$$  /$$",
    r" \  $$$$/  /$$__  $$| $$__  $$ /$$__  $$| $$      | $$ |____  $$| $$ | $$ | $$",
    r"  >$$  $$ | $$$$$$$$| $$  \ $$| $$  \ $$| $$      | $$  /$$$$$$$| $$ | $$ | $$",
    r" /$$/\  $$| $$_____/| $$  | $$| $$  | $$| $$    $$| $$ /$$__  $$| $$ | $$ | $$",
    r"| $$  \ $$|  $$$$$$$| $$  | $$|  $$$$$$/|  $$$$$$/| $$|  $$$$$$$|  $$$$/$$$$/",
    r"|__/  |__/ \_______/|__/  |__/ \______/  \______/ |__/ \_______/ \_____/\___/ ",
];

/// Compact banner used when terminal width < 80 cols.
const COMPACT_BANNER: &str = "[ X E N O C L A W ]";

// ─── Terminal Capability Detection ───────────────────────────────────────────

/// Returns true if backgrounds should be painted.
///
/// BG uses ANSI 256-color (AnsiValue(233)), which requires at least a
/// 256-color capable terminal. Disabled automatically on Color16 terminals
/// (TERM=xterm, linux, vt100, etc.) where AnsiValue sequences are unreliable.
/// Force-override with XENOCLAW_FORCE_BG=0 (off) or =1 (on).
fn bg_enabled() -> bool {
    if std::env::var("XENOCLAW_FORCE_BG").as_deref() == Ok("0") {
        return false;
    }
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
    // Skip BG on basic 16-color terminals — AnsiValue(233) won't render.
    color_depth() != ColorDepth::Color16
}

/// Conditionally paint a background — no-op when `bg_enabled()` is false.
fn bg(stdout: &mut io::Stdout, color: Color) -> io::Result<()> {
    if bg_enabled() {
        stdout.queue(SetBackgroundColor(color))?;
    }
    Ok(())
}

// ─── Rendering Helpers ───────────────────────────────────────────────────────

fn print_header(stdout: &mut io::Stdout, step: u8) -> io::Result<()> {
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(red()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("XENOCLAW"))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(dim()))?
        .queue(Print(" │ "))?
        .queue(SetForegroundColor(white()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Init"))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(dim()))?
        .queue(Print(format!("  {step} of {TOTAL_STEPS}")))?
        .queue(Print("\r\n"))?
        .queue(SetForegroundColor(rule_fg()))?
        .queue(Print(RULE))?
        .queue(Print("\r\n\r\n"))?;
    stdout.flush()
}

fn print_banner(stdout: &mut io::Stdout) -> io::Result<()> {
    let (cols, _) = terminal::size().unwrap_or((80, 24));

    bg(stdout, BG)?;

    // Compute the actual widest banner line so the threshold is exact.
    // Some lines are ~83 chars — using a hardcoded 80 would show the full
    // banner at widths where it overflows and wraps.
    let max_banner_width = BANNER_LINES
        .iter()
        .map(|l| UnicodeWidthStr::width(*l))
        .max()
        .unwrap_or(80) as u16;

    if cols >= max_banner_width {
        // Full banner — 6 rows of red blocks.
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?;
        for line in BANNER_LINES {
            stdout.queue(Print(line))?;
            stdout.queue(Print("\r\n"))?;
        }
        stdout.queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else {
        // Compact banner for narrow terminals.
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("  {COMPACT_BANNER}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    }

    stdout
        .queue(Print("\r\n"))?
        .queue(SetForegroundColor(text()))?
        .queue(Print("                       Agent Runtime\r\n"))?
        .queue(SetForegroundColor(rule_fg()))?
        .queue(Print(format!(" {RULE}\r\n")))?;
    stdout.flush()
}

fn print_footer(stdout: &mut io::Stdout, hint: &str) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let hint_width = UnicodeWidthStr::width(hint);
    let cols = cols as usize;

    stdout
        .queue(cursor::MoveTo(0, rows.saturating_sub(1)))?
        .queue(SetForegroundColor(dim()))?;
    bg(stdout, BG)?;
    stdout.queue(Print(hint))?;

    // Pad the rest of the footer line so the BG color (if any) extends to
    // the right edge. Computed using display width, not byte length, so
    // emoji/wide-char hints align correctly.
    if hint_width < cols {
        let padding = cols - hint_width;
        stdout.queue(Print(" ".repeat(padding)))?;
    }
    stdout.flush()
}

/// Clear the screen and fill it with the brand background color.
///
/// Two-step: first issue `Clear(All)` (which honors BG on well-behaved
/// terminals), then manually fill every cell except the very last one — that
/// final cell is intentionally skipped because writing to the bottom-right
/// cell causes a scroll on most terminals, which would push our header up.
fn clear_screen(stdout: &mut io::Stdout) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let cols = cols as usize;
    let rows = rows as usize;

    stdout.queue(cursor::MoveTo(0, 0))?;
    bg(stdout, BG)?;
    stdout.queue(Clear(ClearType::All))?;

    if bg_enabled() {
        bg(stdout, BG)?;
        let blank_line = " ".repeat(cols);
        // Fill every full row except the last.
        for _ in 0..rows.saturating_sub(1) {
            stdout.queue(Print(&blank_line))?;
        }
        // Last row: fill all but the final cell to avoid scroll.
        if cols > 1 {
            stdout.queue(Print(" ".repeat(cols - 1)))?;
        }
    }

    stdout.queue(cursor::MoveTo(0, 0))?;
    bg(stdout, BG)?;
    stdout.flush()
}

fn print_success(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(green()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("  ✓ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout.queue(Print("\r\n"))?;
    stdout.flush()
}

fn print_error(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(red_bright()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("  ✗ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout.queue(Print("\r\n"))?;
    stdout.flush()
}

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        "●".repeat(key.len())
    } else {
        let prefix = &key[..4];
        format!("{prefix}●●●●●●●●●●●●")
    }
}

// ─── Text Input Helper ───────────────────────────────────────────────────────

/// A simple text input buffer with cursor position for arrow key navigation.
#[derive(Debug, Clone)]
struct TextInput {
    text: String,
    cursor: usize,
}

impl TextInput {
    fn new(initial: &str) -> Self {
        let len = initial.len();
        Self {
            text: initial.to_string(),
            cursor: len,
        }
    }

    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.text.remove(prev);
            self.cursor = prev;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor += self.text[self.cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Render the text with a cursor character inserted at the cursor position.
    fn display(&self) -> String {
        let before = &self.text[..self.cursor];
        let after = &self.text[self.cursor..];
        format!("{before}█{after}")
    }

    fn value(&self) -> &str {
        &self.text
    }
}

// ─── Step Outcomes ───────────────────────────────────────────────────────────

enum StepOutcome {
    Next,
    Back,
    Quit,
}

// ─── Public Entry Point ──────────────────────────────────────────────────────

/// Run the first-run setup wizard.
pub async fn run_wizard(config_path: &Path) -> Result<()> {
    // Warn loudly when the wizard is about to write to a path the installed
    // systemd service will not read. The default with the systemd unit is
    // /etc/xenoclaw/config.toml — if a user runs `xenoclaw -s` unprivileged,
    // the wizard would silently update ~/.xenoclaw/config.toml instead, and
    // every login would keep failing with "Invalid …" until they noticed.
    let system_config = Path::new("/etc/xenoclaw/config.toml");
    if system_config.exists() && config_path != system_config {
        eprintln!(
            "Warning: a system install at {} exists, but this wizard would write to {}.\n\
             The xenoclaw-agent service reads the system config, so updates here will be ignored.\n\
             Re-run with `sudo xenoclaw -s --config /etc/xenoclaw/config.toml` to update the system\n\
             config, or pass --config <path> to confirm you really mean to write somewhere else.\n",
            system_config.display(),
            config_path.display(),
        );
        eprint!("Continue writing to {}? [y/N] ", config_path.display());
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut answer = String::new();
        stdin.lock().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Ok(());
        }
    }

    // Terminal size check — accept down to 60×20, but the experience is better at 80×24+.
    let (cols, rows) = terminal::size()?;
    if cols < 60 || rows < 20 {
        eprintln!(
            "xenoclaw setup requires a terminal at least 60×20. \
             Current size: {cols}×{rows}. Please resize and try again."
        );
        std::process::exit(1);
    }

    let mut stdout = io::stdout();
    stdout.queue(EnterAlternateScreen)?;
    stdout.flush()?;
    terminal::enable_raw_mode()?;

    let result = run_wizard_inner(config_path).await;

    terminal::disable_raw_mode()?;
    stdout.queue(LeaveAlternateScreen)?;
    stdout.queue(ResetColor)?;
    stdout.queue(cursor::Show)?;
    stdout.flush()?;
    println!();
    result
}

async fn run_wizard_inner(config_path: &Path) -> Result<()> {
    let mut state = WizardState::default();
    let mut step: u8 = 1;

    loop {
        let outcome = match step {
            1 => step_welcome(&state).await?,
            2 => step_provider(&mut state).await?,
            3 => step_workspace(&mut state).await?,
            4 => step_server(&mut state).await?,
            5 => step_api_key(&mut state).await?,
            6 => step_sandbox(&mut state).await?,
            7 => step_messaging(&mut state).await?,
            8 => step_review(&state, config_path).await?,
            9 => step_done(&state, config_path).await?,
            _ => break,
        };

        match outcome {
            StepOutcome::Next => {
                if step >= TOTAL_STEPS {
                    break;
                }
                step += 1;
            }
            StepOutcome::Back => {
                if step > 1 {
                    step -= 1;
                }
            }
            StepOutcome::Quit => {
                if confirm_quit().await? {
                    break;
                }
            }
        }
    }

    Ok(())
}

// ─── Step 1: Welcome ─────────────────────────────────────────────────────────

fn render_welcome(stdout: &mut io::Stdout) -> io::Result<()> {
    clear_screen(stdout)?;
    print_header(stdout, 1)?;

    stdout.queue(Print("\r\n"))?;
    print_banner(stdout)?;
    stdout.queue(Print("\r\n"))?;

    // Feature highlights. Each row: red glyph + white feature name + dim tail.
    let features: &[(&str, &str, &str)] = &[
        (
            ">",
            "Dual-mode runtime",
            "General 24/7 agent + Coding agent (hot-swap)",
        ),
        (
            ">",
            "Multi-provider failover",
            "27 LLM providers, priority routing",
        ),
        (
            ">",
            "Messaging bridges",
            "Telegram, Discord & WhatsApp — chat from anywhere",
        ),
        (
            ">",
            "WASM plugin system",
            "sandboxed extensions with hot-reload",
        ),
        (
            ">",
            "Sandboxed execution",
            "RBAC, filesystem & network allowlists",
        ),
        (
            ">",
            "Always-on supervision",
            "auto-restart, health checks, SIGHUP hot-reload",
        ),
    ];

    for (glyph, name, tail) in features {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("  {glyph}  ")))?
            .queue(SetForegroundColor(white()))?
            .queue(Print(*name))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("  — {tail}\r\n")))?;
    }

    stdout.queue(Print("\r\n"))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(text()))?
        .queue(SetAttribute(Attribute::Italic))?
        .queue(Print(
            "  Self-hosted. Your infrastructure. Your data. Your rules.\r\n",
        ))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;

    stdout.queue(Print("\r\n"))?;
    stdout.flush()?;

    print_footer(stdout, "  [Enter] Begin setup    [Esc] Cancel")?;
    Ok(())
}

async fn step_welcome(_state: &WizardState) -> Result<StepOutcome> {
    let mut stdout = io::stdout();
    render_welcome(&mut stdout)?;

    loop {
        match event::read()? {
            Event::Key(key) => match key.code {
                KeyCode::Enter => return Ok(StepOutcome::Next),
                KeyCode::Esc => return Ok(StepOutcome::Quit),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            },
            // Hyprland (and other compositors) send a resize event when the
            // terminal window is tiled/untiled or fullscreen is toggled.
            // Re-render so nothing is cut off at the new dimensions.
            Event::Resize(_, _) => {
                render_welcome(&mut stdout)?;
            }
            _ => {}
        }
    }
}

// ─── Step 2: LLM Provider ────────────────────────────────────────────────────

async fn step_provider(state: &mut WizardState) -> Result<StepOutcome> {
    let mut selected: usize = state.provider_idx;
    let page_size: usize = 12;
    let mut scroll_offset: usize = 0;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Select your LLM provider\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        // Ensure selected item is visible
        if selected < scroll_offset {
            scroll_offset = selected;
        } else if selected >= scroll_offset + page_size {
            scroll_offset = selected - page_size + 1;
        }

        let end = (scroll_offset + page_size).min(PROVIDERS.len());
        for (i, p) in PROVIDERS.iter().enumerate().take(end).skip(scroll_offset) {
            let num = i + 1;
            if i == selected {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(red()))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > [{num:>2}]  ")))?
                    .queue(SetForegroundColor(white()))?
                    .queue(Print(format!("{:<20} ", p.name)))?
                    .queue(SetForegroundColor(text()))?
                    .queue(Print(format!("{}\r\n", p.default_model)))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout.queue(SetForegroundColor(dim()))?.queue(Print(format!(
                    "     [{num:>2}]  {:<20} {}\r\n",
                    p.name, p.default_model
                )))?;
            }
        }

        if end < PROVIDERS.len() {
            bg(&mut stdout, BG)?;
            stdout.queue(SetForegroundColor(dim()))?.queue(Print(format!(
                "\r\n     … and {} more (scroll down)\r\n",
                PROVIDERS.len() - end
            )))?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [↑↓/jk] Navigate  [Enter] Choose  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < PROVIDERS.len() - 1 {
                        selected += 1;
                    }
                }
                KeyCode::Enter => {
                    state.provider_idx = selected;
                    let p = &PROVIDERS[selected];
                    state.base_url = p.default_base_url.to_string();
                    state.model = p.default_model.to_string();
                    if p.needs_api_key {
                        let outcome = step_provider_details(state).await?;
                        match outcome {
                            StepOutcome::Next => return Ok(StepOutcome::Next),
                            StepOutcome::Back => continue,
                            StepOutcome::Quit => return Ok(StepOutcome::Quit),
                        }
                    } else if p.default_base_url.is_empty() {
                        // CLI-backed provider (Claude Code CLI, Copilot CLI) — no API key
                        // or base URL needed; show an info/model screen instead.
                        let outcome = step_cli_provider(state).await?;
                        match outcome {
                            StepOutcome::Next => return Ok(StepOutcome::Next),
                            StepOutcome::Back => continue,
                            StepOutcome::Quit => return Ok(StepOutcome::Quit),
                        }
                    } else {
                        return Ok(StepOutcome::Next);
                    }
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

/// Step 2b — CLI-backed providers (Claude Code CLI, GitHub Copilot CLI).
///
/// Checks whether the selected CLI is installed and authenticated, offers to
/// fix either problem interactively, and (for Claude Code) injects XenoClaw
/// into the CLI's MCP server list before proceeding.
///
/// Sets `state.cli_provider_ready = false` if the user skips a failing check,
/// which greys out "Start server" on the final done screen.
async fn step_cli_provider(state: &mut WizardState) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let cli_type = match p.provider_type {
        "claude_code" => CliType::ClaudeCode,
        "copilot_cli" => CliType::CopilotCli,
        "gemini_cli" => CliType::GeminiCli,
        "codex_cli" => CliType::CodexCli,
        _ => CliType::ClaudeCode,
    };
    let cli_name = CliHealth::display_name(cli_type);

    let mut model_input = TextInput::new(&state.model);
    let mut editing = false;

    // ── Phase 0: show "Checking…" while we run the async probes ─────────────
    {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   {cli_name}\r\n\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print("   Checking status…\r\n"))?;
        stdout.flush()?;
    }

    let mut health = CliHealth::check(cli_type).await;

    // ── Main render + interaction loop ───────────────────────────────────────
    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;

        // Title
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   {cli_name}\r\n\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        // ── Status rows ──────────────────────────────────────────────────────

        // Installed row
        render_cli_status_row(
            &mut stdout,
            "Installed",
            health.installed,
            health.version.as_deref(),
        )?;

        // Authenticated row (only meaningful if installed)
        if health.installed {
            render_cli_status_row(&mut stdout, "Authenticated", health.logged_in, None)?;
        } else {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(dim()))?
                .queue(Print("   —  Authenticated   (skipped — not installed)\r\n"))?;
        }

        // MCP row (Claude Code / Gemini CLI — shown once registered)
        if CliHealth::supports_mcp(cli_type) && state.mcp_registered {
            let mcp_path = match cli_type {
                CliType::GeminiCli => "~/.gemini/settings.json",
                _ => "~/.claude/settings.json",
            };
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(green()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   ✓  "))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(white()))?
                .queue(Print("MCP configured  "))?
                .queue(SetForegroundColor(dim()))?
                .queue(Print(format!("{mcp_path}\r\n")))?;
        }

        stdout.queue(Print("\r\n"))?;
        bg(&mut stdout, BG)?;

        // ── Action block ─────────────────────────────────────────────────────

        if health.is_ready() {
            // All good — show model field and let user continue
            render_text_row(&mut stdout, true, "Model", &model_input, editing)?;
            stdout.queue(Print("\r\n"))?;
            stdout.flush()?;
            print_footer(
                &mut stdout,
                "  [Enter] Continue  [E] Edit model  [Esc] Back  [Ctrl-C] Quit",
            )?;

            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Enter if editing => {
                        editing = false;
                    }
                    KeyCode::Enter => {
                        state.model = model_input.value().to_string();
                        state.cli_provider_ready = true;

                        // Inject MCP config for supported CLIs (Claude Code, Gemini CLI)
                        if CliHealth::supports_mcp(cli_type) && !state.mcp_registered {
                            match CliHealth::inject_mcp(cli_type) {
                                Ok(()) => {
                                    state.mcp_registered = true;
                                }
                                Err(e) => {
                                    // Non-fatal: log and continue without MCP
                                    let _ = e; // shown on review screen
                                }
                            }
                        }
                        return Ok(StepOutcome::Next);
                    }
                    KeyCode::Char('e') | KeyCode::Char('E') if !editing => {
                        editing = true;
                    }
                    KeyCode::Left if editing => model_input.move_left(),
                    KeyCode::Right if editing => model_input.move_right(),
                    KeyCode::Home if editing => model_input.move_home(),
                    KeyCode::End if editing => model_input.move_end(),
                    KeyCode::Backspace if editing => model_input.backspace(),
                    KeyCode::Delete if editing => model_input.delete(),
                    KeyCode::Char(c) if editing => model_input.insert(c),
                    KeyCode::Esc => return Ok(StepOutcome::Back),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(StepOutcome::Quit);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => continue,
                _ => {}
            }
        } else if !health.installed {
            // ── Not installed ────────────────────────────────────────────────
            if health.can_auto_install {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(amber()))?
                    .queue(Print(format!(
                        "   {} is available — XenoClaw can install for you.\r\n\r\n",
                        health.install_prereq
                    )))?;
                bg(&mut stdout, BG)?;
                stdout.flush()?;
                print_footer(
                    &mut stdout,
                    "  [I] Install now  [S] Skip (serve disabled)  [Esc] Back",
                )?;
            } else {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(amber()))?
                    .queue(Print(format!(
                        "   Cannot auto-install — {} not found in PATH.\r\n",
                        health.install_prereq
                    )))?
                    .queue(SetForegroundColor(dim()))?
                    .queue(Print(format!(
                        "   Manual install:  {}\r\n\r\n",
                        health.install_instructions
                    )))?;
                bg(&mut stdout, BG)?;
                stdout.flush()?;
                print_footer(
                    &mut stdout,
                    "  [S] Skip (serve disabled)  [Esc] Back  [Ctrl-C] Quit",
                )?;
            }

            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('i') | KeyCode::Char('I') if health.can_auto_install => {
                        // Suspend TUI and run the install command
                        let (bin, args) = CliHealth::install_command(cli_type);
                        cli_run_interactive(&mut stdout, "Installing", bin, args)?;
                        // Re-check after install
                        health = CliHealth::check(cli_type).await;
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        state.cli_provider_ready = false;
                        state.model = model_input.value().to_string();
                        return Ok(StepOutcome::Next);
                    }
                    KeyCode::Esc => return Ok(StepOutcome::Back),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(StepOutcome::Quit);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => continue,
                _ => {}
            }
        } else {
            // ── Installed but not logged in ──────────────────────────────────
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(amber()))?
                .queue(Print(format!(
                    "   {cli_name} is installed but you are not logged in.\r\n\r\n"
                )))?;
            bg(&mut stdout, BG)?;
            stdout.flush()?;
            print_footer(
                &mut stdout,
                "  [L] Login now  [S] Skip (serve disabled)  [Esc] Back  [Ctrl-C] Quit",
            )?;

            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('l') | KeyCode::Char('L') => {
                        // Suspend TUI and hand off to the CLI's own login flow
                        let (bin, args) = CliHealth::login_command(cli_type);
                        cli_run_interactive(&mut stdout, "Login", bin, args)?;
                        health = CliHealth::check(cli_type).await;
                    }
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        state.cli_provider_ready = false;
                        state.model = model_input.value().to_string();
                        return Ok(StepOutcome::Next);
                    }
                    KeyCode::Esc => return Ok(StepOutcome::Back),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(StepOutcome::Quit);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => continue,
                _ => {}
            }
        }
    }
}

// ─── CLI step helpers ────────────────────────────────────────────────────────

/// Render a single ✓ / ✗ status row for the CLI check screen.
fn render_cli_status_row(
    stdout: &mut io::Stdout,
    label: &str,
    ok: bool,
    extra: Option<&str>,
) -> io::Result<()> {
    bg(stdout, BG)?;
    if ok {
        stdout
            .queue(SetForegroundColor(green()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   ✓  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        stdout.queue(SetForegroundColor(white()))?.queue(Print(format!("{label:<16}")))?;
        if let Some(v) = extra {
            stdout.queue(SetForegroundColor(dim()))?.queue(Print(format!("  {v}")))?;
        }
    } else {
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   ✗  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("{label:<16}")))?;
    }
    stdout.queue(Print("\r\n"))?;
    stdout.flush()
}

/// Suspend the TUI, run an external command with inherited stdio, then resume.
///
/// Used for both install (e.g. `npm install -g …`) and login flows
/// (e.g. `claude auth login`, `gh auth login`). The command runs with full
/// terminal access so OAuth device-code prompts, browser redirects, and
/// progress bars all work normally.
fn cli_run_interactive(
    stdout: &mut io::Stdout,
    verb: &str,
    bin: &str,
    args: &[&str],
) -> io::Result<()> {
    // Leave the wizard's alternate screen
    terminal::disable_raw_mode()?;
    stdout.queue(crossterm::terminal::LeaveAlternateScreen)?;
    stdout.queue(crossterm::style::ResetColor)?;
    stdout.flush()?;

    println!();
    println!("─── XenoClaw: {verb} ───────────────────────────────────────────");
    println!("Running: {bin} {}", args.join(" "));
    println!("Complete the flow below, then XenoClaw will resume.");
    println!();

    let _ = std::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status();

    println!();
    println!("─── Returning to XenoClaw setup ────────────────────────────────");
    println!("Press Enter to continue…");

    // Wait for Enter so the user can read any output before we wipe the screen
    let _ = std::io::stdin().read_line(&mut String::new());

    // Restore the wizard's alternate screen
    stdout.queue(crossterm::terminal::EnterAlternateScreen)?;
    stdout.flush()?;
    terminal::enable_raw_mode()?;
    Ok(())
}

async fn step_provider_details(state: &mut WizardState) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let mut inputs = [
        TextInput::new(&state.api_key),
        TextInput::new(&state.base_url),
        TextInput::new(&state.model),
    ];
    let labels = ["API Key", "Base URL", "Model"];
    let masked = [true, false, false];
    let mut field: usize = 0;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Provider: ".to_string()))?
            .queue(SetForegroundColor(white()))?
            .queue(Print(format!("{}\r\n\r\n", p.name)))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        for (i, input) in inputs.iter().enumerate() {
            let display = if masked[i] && !input.value().is_empty() {
                mask_key(input.value())
            } else if i == field {
                input.display()
            } else {
                input.value().to_string()
            };
            if i == field {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(red()))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > {:<10} ", labels[i])))?
                    .queue(SetForegroundColor(dim()))?
                    .queue(Print("│ "))?
                    .queue(SetForegroundColor(white()))?
                    .queue(Print(format!("{display}\r\n")))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(dim()))?
                    .queue(Print(format!("     {:<10} │ {display}\r\n", labels[i])))?;
            }
        }

        stdout.queue(Print("\r\n"))?;
        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [Tab] Next field  [←→] Move cursor  [Enter] Confirm  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => {
                    field = (field + 1) % 3;
                }
                KeyCode::BackTab => {
                    field = if field == 0 { 2 } else { field - 1 };
                }
                KeyCode::Enter => {
                    state.api_key = inputs[0].value().to_string();
                    state.base_url = inputs[1].value().to_string();
                    state.model = inputs[2].value().to_string();
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Left => inputs[field].move_left(),
                KeyCode::Right => inputs[field].move_right(),
                KeyCode::Home => inputs[field].move_home(),
                KeyCode::End => inputs[field].move_end(),
                KeyCode::Backspace => inputs[field].backspace(),
                KeyCode::Delete => inputs[field].delete(),
                KeyCode::Char(c) => inputs[field].insert(c),
                _ => {}
            }
        }
    }
}

// ─── Step 3: Workspace ───────────────────────────────────────────────────────

async fn step_workspace(state: &mut WizardState) -> Result<StepOutcome> {
    let mut input = TextInput::new(&state.workspace_dir);

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 3)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Workspace directory\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout.queue(SetForegroundColor(dim()))?.queue(Print(
            "   Contains SOUL.md, IDENTITY.md, memory/, skills/, etc.\r\n\r\n",
        ))?;
        bg(&mut stdout, BG)?;

        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   > "))?
            .queue(SetForegroundColor(white()))?
            .queue(Print(format!("{}\r\n\r\n", input.display())))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        let path = Path::new(input.value());
        if path.exists() && path.is_dir() {
            let count = std::fs::read_dir(path).map(|d| d.count()).unwrap_or(0);
            stdout
                .queue(SetForegroundColor(green()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   \u{2713} ".to_string()))?
                .queue(SetForegroundColor(text()))?
                .queue(Print(format!("Found  ({count} entries)\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            stdout
                .queue(SetForegroundColor(amber()))?
                .queue(Print("   ◆ Does not exist yet.\r\n"))?
                .queue(SetForegroundColor(white()))?
                .queue(Print("     ["))?
                .queue(SetForegroundColor(red()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("C"))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetForegroundColor(white()))?;
            bg(&mut stdout, BG)?;
            stdout
                .queue(Print("] Create stub workspace    ["))?
                .queue(SetForegroundColor(red()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("S"))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetForegroundColor(white()))?;
            bg(&mut stdout, BG)?;
            stdout.queue(Print("] Skip for now\r\n"))?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [←→] Cursor  [Enter] Confirm  [C] Create  [S] Skip  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    state.workspace_dir = input.value().to_string();
                    state.create_workspace = !Path::new(input.value()).exists();
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Char('c') | KeyCode::Char('C') if !Path::new(input.value()).exists() => {
                    state.workspace_dir = input.value().to_string();
                    state.create_workspace = true;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    state.workspace_dir = input.value().to_string();
                    state.create_workspace = false;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Left => input.move_left(),
                KeyCode::Right => input.move_right(),
                KeyCode::Home => input.move_home(),
                KeyCode::End => input.move_end(),
                KeyCode::Backspace => input.backspace(),
                KeyCode::Delete => input.delete(),
                KeyCode::Char(c) => input.insert(c),
                _ => {}
            }
        }
    }
}

// ─── Step 4: Server ──────────────────────────────────────────────────────────

async fn step_server(state: &mut WizardState) -> Result<StepOutcome> {
    let mut host_input = TextInput::new(&state.host);
    let mut port_input = TextInput::new(&state.port);
    let mut web_port_input = TextInput::new(&state.web_port);
    let mut require_auth = state.require_auth;
    let mut field: u8 = 0; // 0=host, 1=api port, 2=web port, 3=auth

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 4)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Server Bindings\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        render_field(&mut stdout, field == 0, "Bind host", &host_input)?;
        render_field(&mut stdout, field == 1, "API port", &port_input)?;
        render_field(&mut stdout, field == 2, "Web UI port", &web_port_input)?;

        stdout.queue(Print("\r\n"))?;

        // Auth toggle
        let auth_str = if require_auth { "Yes" } else { "No" };
        let auth_color = if require_auth { green() } else { amber() };
        if field == 3 {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(red()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   > Require authentication?  "))?
                .queue(SetForegroundColor(auth_color))?
                .queue(Print(format!("[{auth_str}]\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(dim()))?
                .queue(Print("     Require authentication?  "))?
                .queue(SetForegroundColor(auth_color))?
                .queue(Print(format!("[{auth_str}]\r\n")))?;
        }
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "  [Tab] Next  [←→] Cursor  [Space] Toggle  [Enter] Confirm  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => {
                    field = (field + 1) % 4;
                }
                KeyCode::BackTab => {
                    field = if field == 0 { 3 } else { field - 1 };
                }
                KeyCode::Enter => {
                    state.host = host_input.value().to_string();
                    state.port = port_input.value().to_string();
                    state.web_port = web_port_input.value().to_string();
                    state.require_auth = require_auth;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') if field == 3 => {
                    require_auth = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') if field == 3 => {
                    require_auth = false;
                }
                KeyCode::Char(' ') if field == 3 => {
                    require_auth = !require_auth;
                }
                KeyCode::Left => match field {
                    0 => host_input.move_left(),
                    1 => port_input.move_left(),
                    2 => web_port_input.move_left(),
                    _ => {}
                },
                KeyCode::Right => match field {
                    0 => host_input.move_right(),
                    1 => port_input.move_right(),
                    2 => web_port_input.move_right(),
                    _ => {}
                },
                KeyCode::Home => match field {
                    0 => host_input.move_home(),
                    1 => port_input.move_home(),
                    2 => web_port_input.move_home(),
                    _ => {}
                },
                KeyCode::End => match field {
                    0 => host_input.move_end(),
                    1 => port_input.move_end(),
                    2 => web_port_input.move_end(),
                    _ => {}
                },
                KeyCode::Backspace => match field {
                    0 => host_input.backspace(),
                    1 => port_input.backspace(),
                    2 => web_port_input.backspace(),
                    _ => {}
                },
                KeyCode::Delete => match field {
                    0 => host_input.delete(),
                    1 => port_input.delete(),
                    2 => web_port_input.delete(),
                    _ => {}
                },
                KeyCode::Char(c) => match field {
                    0 => host_input.insert(c),
                    1 => {
                        if c.is_ascii_digit() {
                            port_input.insert(c);
                        }
                    }
                    2 => {
                        if c.is_ascii_digit() {
                            web_port_input.insert(c);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

/// Render a labeled text input field (active = red arrow + white value).
fn render_field(
    stdout: &mut io::Stdout,
    active: bool,
    label: &str,
    input: &TextInput,
) -> io::Result<()> {
    if active {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label:<14} ")))?
            .queue(SetForegroundColor(dim()))?
            .queue(Print("│ "))?
            .queue(SetForegroundColor(white()))?
            .queue(Print(format!("{}\r\n", input.display())))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("     {label:<14} │ {}\r\n", input.value())))?;
    }
    Ok(())
}

// ─── Step 5: Authentication ──────────────────────────────────────────────────

async fn step_api_key(state: &mut WizardState) -> Result<StepOutcome> {
    if state.admin_key_raw.is_empty() {
        let auth = ApiKeyAuthenticator::new();
        state.admin_key_raw = format!("xc_{}", auth.generate_key(40));
        state.admin_key_hash = auth.hash_key(&state.admin_key_raw);
    }

    let mut username_input = TextInput::new(&state.admin_username);
    let mut password_input = TextInput::new(&state.admin_password);
    let mut field: u8 = 0; // 0=viewing key, 1=username, 2=password

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 5)?;

        // API Key section
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   API Key  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print("(programmatic access)\r\n\r\n"))?;
        bg(&mut stdout, BG)?;

        // Key callout box — red border, white key text on charcoal.
        let box_w: usize = 56;
        let border = "─".repeat(box_w);
        let warn_text = "⚠ SAVE THIS NOW";
        let warn_w = UnicodeWidthStr::width(warn_text);
        // dim_pad fills the rest of the inner box after the warning text.
        // Inner layout: 1 (space) + warn_w + dim_pad + 1 (trailing space before │) = box_w
        let dim_pad = box_w.saturating_sub(2 + warn_w);
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(Print(format!("   ┌{border}┐\r\n")))?
            .queue(Print("   │ "))?
            .queue(SetForegroundColor(amber()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(warn_text))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(dim()))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print(format!(
                "{:<dim_pad$}",
                " — will not be shown again.",
            )))?
            .queue(SetForegroundColor(red()))?
            .queue(Print(" │\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!(
                "{:<width$}",
                state.admin_key_raw,
                width = box_w - 2
            )))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(red()))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("│\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print(format!("   └{border}┘\r\n")))?
            .queue(SetForegroundColor(dim()))?
            .queue(Print("   [R] Regenerate key\r\n\r\n"))?;
        bg(&mut stdout, BG)?;

        // Web UI login section
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Web UI Login  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print("(username + password)\r\n\r\n"))?;
        bg(&mut stdout, BG)?;

        render_field(&mut stdout, field == 1, "Username", &username_input)?;

        // Password — masked when not editing
        if field == 2 {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(red()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   > {:<14} ", "Password")))?
                .queue(SetForegroundColor(dim()))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("{}\r\n", password_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            let masked = if password_input.value().is_empty() {
                "(not set — password login disabled)".to_string()
            } else {
                "●".repeat(password_input.value().len())
            };
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(dim()))?
                .queue(Print(format!("     {:<14} │ {masked}\r\n", "Password")))?;
        }

        stdout.queue(Print("\r\n"))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "   Both API key and password auth work for the web UI.\r\n",
            ))?
            .queue(Print(
                "   Leave password empty to disable password login.\r\n",
            ))?;

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [Tab] Next  [←→] Cursor  [R] Regen key  [Enter] Accept  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => {
                    field = (field + 1) % 3;
                }
                KeyCode::BackTab => {
                    field = if field == 0 { 2 } else { field - 1 };
                }
                KeyCode::Enter => {
                    state.admin_username = username_input.value().to_string();
                    state.admin_password = password_input.value().to_string();
                    if !state.admin_password.is_empty() {
                        state.admin_password_hash = hash_password(&state.admin_password);
                    }
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('r') | KeyCode::Char('R') if field == 0 => {
                    let auth = ApiKeyAuthenticator::new();
                    state.admin_key_raw = format!("xc_{}", auth.generate_key(40));
                    state.admin_key_hash = auth.hash_key(&state.admin_key_raw);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Left => match field {
                    1 => username_input.move_left(),
                    2 => password_input.move_left(),
                    _ => {}
                },
                KeyCode::Right => match field {
                    1 => username_input.move_right(),
                    2 => password_input.move_right(),
                    _ => {}
                },
                KeyCode::Home => match field {
                    1 => username_input.move_home(),
                    2 => password_input.move_home(),
                    _ => {}
                },
                KeyCode::End => match field {
                    1 => username_input.move_end(),
                    2 => password_input.move_end(),
                    _ => {}
                },
                KeyCode::Backspace => match field {
                    1 => username_input.backspace(),
                    2 => password_input.backspace(),
                    _ => {}
                },
                KeyCode::Delete => match field {
                    1 => username_input.delete(),
                    2 => password_input.delete(),
                    _ => {}
                },
                KeyCode::Char(c) if field >= 1 => match field {
                    1 => username_input.insert(c),
                    2 => password_input.insert(c),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

/// One-line summary of which messaging bridges are configured, for the
/// review screen. Shows "none" when all three are blank.
fn messaging_summary(state: &WizardState) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !state.telegram_bot_token.is_empty() {
        parts.push("Telegram");
    }
    if !state.discord_bot_token.is_empty() {
        parts.push("Discord");
    }
    if !state.whatsapp_phone.is_empty() {
        parts.push("WhatsApp");
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

/// Build the `[messaging]` block(s) for the generated config.
///
/// Each provider is emitted only when its field is populated, so a blank
/// wizard run leaves [messaging] entirely absent and the runtime defaults
/// (no bridges) apply. The bot tokens are TOML-escaped to handle the
/// characters real tokens contain (`:`, backslashes from copy-paste, etc.).
fn build_messaging_section(state: &WizardState) -> String {
    let any = !state.telegram_bot_token.is_empty()
        || !state.discord_bot_token.is_empty()
        || !state.whatsapp_phone.is_empty();
    if !any {
        return String::new();
    }

    let mut out = String::from("\n[messaging]\n");
    if !state.telegram_bot_token.is_empty() {
        out.push_str(&format!(
            "\n[messaging.telegram]\nbot_token = {}\n",
            toml_basic_string(&state.telegram_bot_token),
        ));
    }
    if !state.discord_bot_token.is_empty() {
        out.push_str(&format!(
            "\n[messaging.discord]\nbot_token = {}\n",
            toml_basic_string(&state.discord_bot_token),
        ));
    }
    if !state.whatsapp_phone.is_empty() {
        out.push_str(&format!(
            "\n[messaging.whatsapp]\nphone_number = {}\n",
            toml_basic_string(&state.whatsapp_phone),
        ));
    }
    out
}

/// TOML basic-string encoding. Conservative — handles the characters bot
/// tokens and phone numbers actually contain.
fn toml_basic_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Pick a web UI directory to bake into the generated config.
///
/// `common::config::default_web_dir()` falls back to the literal `./web/dist`
/// when `XENOCLAW_WEB_DIR` isn't set — which is true when the wizard is run
/// interactively from a shell, even though the systemd unit later sets the
/// env var. Writing that relative path into the TOML clobbers the install
/// script's correct value and breaks `web.dir` lookup at service start.
///
/// Prefer, in order:
///   1. `$XENOCLAW_WEB_DIR` if set
///   2. `/opt/xenoclaw/web` if it exists (the install.sh layout)
///   3. `./web/dist` canonicalized if it exists (dev checkout)
///   4. `/opt/xenoclaw/web` as the most useful default for VPS users
fn resolve_wizard_web_dir() -> PathBuf {
    if let Some(env) = std::env::var_os("XENOCLAW_WEB_DIR") {
        return PathBuf::from(env);
    }
    let installed = PathBuf::from("/opt/xenoclaw/web");
    if installed.exists() {
        return installed;
    }
    let dev = PathBuf::from("./web/dist");
    if dev.exists() {
        if let Ok(abs) = std::fs::canonicalize(&dev) {
            return abs;
        }
    }
    installed
}

/// Hash a password using bcrypt.
fn hash_password(password: &str) -> String {
    bcrypt::hash(password, bcrypt::DEFAULT_COST).unwrap_or_else(|_| {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        format!("sha256:{}", hex::encode(hasher.finalize()))
    })
}

// ─── Step 6: Sandbox ─────────────────────────────────────────────────────────

async fn step_sandbox(state: &mut WizardState) -> Result<StepOutcome> {
    let mut selected: usize = 0;
    let mut custom_input = TextInput::new(&state.custom_commands);
    let mut exclude_input = TextInput::new(&state.excluded_commands);
    let mut timeout_input = TextInput::new(&state.tool_timeout);
    let mut max_file_size_input = TextInput::new(&state.max_file_size_mb);
    let mut max_concurrent_shells_input = TextInput::new(&state.max_concurrent_shells);
    let mut undo_history_input = TextInput::new(&state.undo_history_size);
    let mut editing: Option<usize> = None;

    loop {
        let cmd_count = state.sandbox_commands.len();
        let row_allow_all = 0;
        let row_pipes = cmd_count + 1;
        let row_custom = cmd_count + 2;
        let row_exclude = cmd_count + 3;
        let row_timeout = cmd_count + 4;
        let row_max_file_size = cmd_count + 5;
        let row_max_concurrent_shells = cmd_count + 6;
        let row_undo_history = cmd_count + 7;
        let total_rows = cmd_count + 8;

        let unit_label = match state.timeout_unit {
            TimeoutUnit::Seconds => "seconds",
            TimeoutUnit::Minutes => "minutes",
            TimeoutUnit::Hours => "hours",
        };

        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 6)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Shell Sandbox\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout.queue(SetForegroundColor(dim()))?.queue(Print(
            "   Space toggles, Enter edits text, Tab cycles unit\r\n\r\n",
        ))?;
        bg(&mut stdout, BG)?;

        // Allow all
        let all_check = if state.allow_all_commands { "✓" } else { " " };
        render_toggle_row(
            &mut stdout,
            selected == row_allow_all,
            state.allow_all_commands,
            &format!("[{all_check}] * (allow ALL commands)"),
        )?;

        stdout.queue(Print("\r\n"))?;

        // Individual commands
        for (i, (cmd, enabled)) in state.sandbox_commands.iter().enumerate() {
            let on = *enabled || state.allow_all_commands;
            let check = if on { "✓" } else { " " };
            let row = i + 1;
            let is_active = selected == row && editing.is_none();
            render_toggle_row(&mut stdout, is_active, on, &format!("[{check}] {cmd}"))?;
        }

        stdout.queue(Print("\r\n"))?;

        // Allow pipes
        let pipe_check = if state.allow_pipes { "✓" } else { " " };
        render_toggle_row(
            &mut stdout,
            selected == row_pipes,
            state.allow_pipes,
            &format!("[{pipe_check}] Allow pipes & operators (|, &&, ||, ;, >)"),
        )?;

        stdout.queue(Print("\r\n"))?;

        let is_editing_custom = editing == Some(row_custom);
        render_text_row(
            &mut stdout,
            selected == row_custom || is_editing_custom,
            "Add commands (comma-separated, wildcards: python*):",
            &custom_input,
            is_editing_custom,
        )?;

        let is_editing_exclude = editing == Some(row_exclude);
        render_text_row(
            &mut stdout,
            selected == row_exclude || is_editing_exclude,
            "Exclude commands (comma-separated):",
            &exclude_input,
            is_editing_exclude,
        )?;

        stdout.queue(Print("\r\n"))?;

        // Timeout with unit
        let is_editing_timeout = editing == Some(row_timeout);
        if selected == row_timeout || is_editing_timeout {
            let display = if is_editing_timeout {
                timeout_input.display()
            } else {
                timeout_input.value().to_string()
            };
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(red()))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   > Timeout: "))?
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("{display}  ")))?
                .queue(SetForegroundColor(dim()))?
                .queue(Print(format!("{unit_label}  [Tab to change unit]\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            bg(&mut stdout, BG)?;
            stdout.queue(SetForegroundColor(dim()))?.queue(Print(format!(
                "     Timeout: {}  {unit_label}\r\n",
                timeout_input.value()
            )))?;
        }

        stdout.queue(Print("\r\n"))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print("   Coding limits\r\n"))?;
        bg(&mut stdout, BG)?;

        // Max file size (MB) — for the coding agent's file read/write ops
        render_numeric_row(
            &mut stdout,
            selected == row_max_file_size,
            editing == Some(row_max_file_size),
            "Max file size",
            &max_file_size_input,
            "MB",
        )?;

        // Max concurrent shells — caps how many shell commands run at once
        render_numeric_row(
            &mut stdout,
            selected == row_max_concurrent_shells,
            editing == Some(row_max_concurrent_shells),
            "Max concurrent shells",
            &max_concurrent_shells_input,
            "processes",
        )?;

        // Undo history size — how many file ops can be reverted
        render_numeric_row(
            &mut stdout,
            selected == row_undo_history,
            editing == Some(row_undo_history),
            "Undo history",
            &undo_history_input,
            "entries",
        )?;

        stdout.flush()?;
        let footer = if editing.is_some() {
            "  [←→] Cursor  [Enter/Tab] Done  [Esc] Cancel"
        } else {
            "  [↑↓] Nav  [Space] Toggle  [Enter] Edit/Next  [Esc] Back"
        };
        print_footer(&mut stdout, footer)?;

        if let Event::Key(key) = event::read()? {
            // Text editing mode
            if let Some(edit_row) = editing {
                let input = if edit_row == row_custom {
                    &mut custom_input
                } else if edit_row == row_exclude {
                    &mut exclude_input
                } else if edit_row == row_timeout {
                    &mut timeout_input
                } else if edit_row == row_max_file_size {
                    &mut max_file_size_input
                } else if edit_row == row_max_concurrent_shells {
                    &mut max_concurrent_shells_input
                } else {
                    &mut undo_history_input
                };
                let is_timeout = edit_row == row_timeout;
                let is_numeric = is_timeout
                    || edit_row == row_max_file_size
                    || edit_row == row_max_concurrent_shells
                    || edit_row == row_undo_history;

                match key.code {
                    KeyCode::Left => input.move_left(),
                    KeyCode::Right => input.move_right(),
                    KeyCode::Home => input.move_home(),
                    KeyCode::End => input.move_end(),
                    KeyCode::Backspace => input.backspace(),
                    KeyCode::Delete => input.delete(),
                    KeyCode::Enter => {
                        editing = None;
                    }
                    KeyCode::Esc => {
                        editing = None;
                    }
                    KeyCode::Tab if is_timeout => {
                        state.timeout_unit = match state.timeout_unit {
                            TimeoutUnit::Seconds => TimeoutUnit::Minutes,
                            TimeoutUnit::Minutes => TimeoutUnit::Hours,
                            TimeoutUnit::Hours => TimeoutUnit::Seconds,
                        };
                    }
                    KeyCode::Tab => {
                        editing = None;
                    }
                    KeyCode::Char(c) => {
                        if is_numeric {
                            if c.is_ascii_digit() {
                                input.insert(c);
                            }
                        } else {
                            input.insert(c);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            // Normal navigation
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < total_rows - 1 {
                        selected += 1;
                    }
                }
                KeyCode::Char(' ') => {
                    if selected == row_allow_all {
                        state.allow_all_commands = !state.allow_all_commands;
                    } else if selected >= 1 && selected <= cmd_count {
                        state.sandbox_commands[selected - 1].1 =
                            !state.sandbox_commands[selected - 1].1;
                    } else if selected == row_pipes {
                        state.allow_pipes = !state.allow_pipes;
                    }
                }
                KeyCode::Enter => {
                    if selected == row_custom
                        || selected == row_exclude
                        || selected == row_timeout
                        || selected == row_max_file_size
                        || selected == row_max_concurrent_shells
                        || selected == row_undo_history
                    {
                        editing = Some(selected);
                    } else {
                        state.custom_commands = custom_input.value().to_string();
                        state.excluded_commands = exclude_input.value().to_string();
                        state.tool_timeout = timeout_input.value().to_string();
                        state.max_file_size_mb = max_file_size_input.value().to_string();
                        state.max_concurrent_shells =
                            max_concurrent_shells_input.value().to_string();
                        state.undo_history_size = undo_history_input.value().to_string();
                        return Ok(StepOutcome::Next);
                    }
                }
                KeyCode::Tab if selected == row_timeout => {
                    state.timeout_unit = match state.timeout_unit {
                        TimeoutUnit::Seconds => TimeoutUnit::Minutes,
                        TimeoutUnit::Minutes => TimeoutUnit::Hours,
                        TimeoutUnit::Hours => TimeoutUnit::Seconds,
                    };
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

/// Render a numeric field with a trailing unit label. Active when selected;
/// goes white-on-active and shows a cursor block while editing.
fn render_numeric_row(
    stdout: &mut io::Stdout,
    active: bool,
    editing: bool,
    label: &str,
    input: &TextInput,
    unit: &str,
) -> io::Result<()> {
    if active || editing {
        let display = if editing {
            input.display()
        } else {
            input.value().to_string()
        };
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label}: ")))?
            .queue(SetForegroundColor(white()))?
            .queue(Print(format!("{display}  ")))?
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("{unit}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else {
        bg(stdout, BG)?;
        stdout.queue(SetForegroundColor(dim()))?.queue(Print(format!(
            "     {label}: {}  {unit}\r\n",
            input.value()
        )))?;
    }
    Ok(())
}

/// Render a checkbox-style toggle row. Active → red arrow + white text.
/// Enabled-but-inactive → text color (visible but not selected).
/// Disabled-and-inactive → dim.
fn render_toggle_row(
    stdout: &mut io::Stdout,
    active: bool,
    enabled: bool,
    label: &str,
) -> io::Result<()> {
    if active {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else if enabled {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(text()))?
            .queue(Print(format!("     {label}\r\n")))?;
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("     {label}\r\n")))?;
    }
    Ok(())
}

fn render_text_row(
    stdout: &mut io::Stdout,
    active: bool,
    label: &str,
    input: &TextInput,
    editing: bool,
) -> io::Result<()> {
    if active {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        if editing {
            stdout
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("     {}\r\n", input.display())))?;
        } else {
            let val = if input.value().is_empty() {
                "(press Enter to type)"
            } else {
                input.value()
            };
            stdout
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("     {val}\r\n")))?;
        }
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("     {label}\r\n")))?;
        let val = if input.value().is_empty() {
            "(none)"
        } else {
            input.value()
        };
        stdout.queue(Print(format!("     {val}\r\n")))?;
    }
    Ok(())
}

// ─── Step 7: Messaging ───────────────────────────────────────────────────────
//
// Optional third-party chat bridges (Telegram, Discord, WhatsApp). All three
// fields are optional — leaving a field blank disables that provider. The
// telegram bot token is the BotFather "<id>:<hex>" string; Discord is the
// Developer Portal bot token; WhatsApp is the E.164 phone number that the
// xenoclaw process drives via the messaging-bridge stack.

async fn step_messaging(state: &mut WizardState) -> Result<StepOutcome> {
    let mut telegram_input = TextInput::new(&state.telegram_bot_token);
    let mut discord_input = TextInput::new(&state.discord_bot_token);
    let mut whatsapp_input = TextInput::new(&state.whatsapp_phone);
    let mut field: u8 = 0; // 0=telegram, 1=discord, 2=whatsapp

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 7)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Messaging Bridges\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "   Optional — leave any field blank to skip that provider.\r\n\r\n",
            ))?;
        bg(&mut stdout, BG)?;

        render_field(&mut stdout, field == 0, "Telegram token", &telegram_input)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "                  ↳ Get from @BotFather: /newbot, then copy the HTTP API token.\r\n",
            ))?;
        bg(&mut stdout, BG)?;
        stdout.queue(Print("\r\n"))?;

        render_field(&mut stdout, field == 1, "Discord token", &discord_input)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "                  ↳ Discord Developer Portal → Bot → Reset/Copy Token.\r\n",
            ))?;
        bg(&mut stdout, BG)?;
        stdout.queue(Print("\r\n"))?;

        render_field(&mut stdout, field == 2, "WhatsApp phone", &whatsapp_input)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "                  ↳ E.164 number the bridge will pair with (e.g. +14155552671).\r\n",
            ))?;
        bg(&mut stdout, BG)?;

        stdout.flush()?;

        print_footer(
            &mut stdout,
            "  [Tab] Next  [←→] Cursor  [Enter] Confirm  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => {
                    field = (field + 1) % 3;
                }
                KeyCode::BackTab => {
                    field = if field == 0 { 2 } else { field - 1 };
                }
                KeyCode::Enter => {
                    state.telegram_bot_token = telegram_input.value().trim().to_string();
                    state.discord_bot_token = discord_input.value().trim().to_string();
                    state.whatsapp_phone = whatsapp_input.value().trim().to_string();
                    let any = !state.telegram_bot_token.is_empty()
                        || !state.discord_bot_token.is_empty()
                        || !state.whatsapp_phone.is_empty();
                    if !any {
                        return Ok(StepOutcome::Next);
                    }
                    match show_messaging_validation(state).await? {
                        ValidationOutcome::Proceed => return Ok(StepOutcome::Next),
                        ValidationOutcome::Edit => continue,
                        ValidationOutcome::Quit => return Ok(StepOutcome::Quit),
                    }
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Left => match field {
                    0 => telegram_input.move_left(),
                    1 => discord_input.move_left(),
                    2 => whatsapp_input.move_left(),
                    _ => {}
                },
                KeyCode::Right => match field {
                    0 => telegram_input.move_right(),
                    1 => discord_input.move_right(),
                    2 => whatsapp_input.move_right(),
                    _ => {}
                },
                KeyCode::Home => match field {
                    0 => telegram_input.move_home(),
                    1 => discord_input.move_home(),
                    2 => whatsapp_input.move_home(),
                    _ => {}
                },
                KeyCode::End => match field {
                    0 => telegram_input.move_end(),
                    1 => discord_input.move_end(),
                    2 => whatsapp_input.move_end(),
                    _ => {}
                },
                KeyCode::Backspace => match field {
                    0 => telegram_input.backspace(),
                    1 => discord_input.backspace(),
                    2 => whatsapp_input.backspace(),
                    _ => {}
                },
                KeyCode::Delete => match field {
                    0 => telegram_input.delete(),
                    1 => discord_input.delete(),
                    2 => whatsapp_input.delete(),
                    _ => {}
                },
                KeyCode::Char(c) => match field {
                    0 => telegram_input.insert(c),
                    1 => discord_input.insert(c),
                    2 => whatsapp_input.insert(c),
                    _ => {}
                },
                _ => {}
            }
        }
    }
}

// ─── Step 7.5: Messaging token validation ───────────────────────────────────

#[derive(Debug, Clone)]
enum ValidationStatus {
    Ok,
    Failed(String),
    Skipped,
}

#[derive(Debug, Clone)]
struct ValidationResults {
    telegram: ValidationStatus,
    discord: ValidationStatus,
    whatsapp: ValidationStatus,
}

enum ValidationOutcome {
    Proceed,
    Edit,
    Quit,
}

/// Display "Validating…" then a results screen for the three messaging providers.
/// Returns whether the user wants to proceed, go back and edit, or quit entirely.
async fn show_messaging_validation(state: &WizardState) -> Result<ValidationOutcome> {
    // Phase 1: "Validating…" screen
    {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 7)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Validating messaging tokens…\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "   Contacting Telegram and Discord (10s timeout).\r\n",
            ))?;
        bg(&mut stdout, BG)?;
        stdout.flush()?;
    }

    let results = validate_messaging_tokens(state).await;

    // Phase 2: results screen, loop on input
    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 7)?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Token Validation\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;

        render_validation_row(&mut stdout, "Telegram", &results.telegram)?;
        render_validation_row(&mut stdout, "Discord", &results.discord)?;
        render_validation_row(&mut stdout, "WhatsApp", &results.whatsapp)?;

        bg(&mut stdout, BG)?;
        stdout.queue(Print("\r\n"))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(
                "   Failures may just mean the host is offline — you can still save\r\n",
            ))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print("   and fix tokens later by re-running the wizard.\r\n"))?;
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "  [Enter] Continue  [Esc] Back to edit  [Ctrl+C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => return Ok(ValidationOutcome::Proceed),
                KeyCode::Esc => return Ok(ValidationOutcome::Edit),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(ValidationOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

fn render_validation_row(
    stdout: &mut io::Stdout,
    label: &str,
    status: &ValidationStatus,
) -> io::Result<()> {
    bg(stdout, BG)?;
    let (icon, icon_color, detail) = match status {
        ValidationStatus::Ok => ("✓", green(), "valid".to_string()),
        ValidationStatus::Failed(reason) => ("✗", red(), reason.clone()),
        ValidationStatus::Skipped => ("·", dim(), "not configured".to_string()),
    };
    stdout
        .queue(SetForegroundColor(icon_color))?
        .queue(Print(format!("   {icon}  ")))?
        .queue(SetForegroundColor(white()))?
        .queue(Print(format!("{label:<10}")))?
        .queue(SetForegroundColor(dim()))?
        .queue(Print(format!("  {detail}\r\n")))?;
    Ok(())
}

/// Run all three provider validations concurrently and collect results.
async fn validate_messaging_tokens(state: &WizardState) -> ValidationResults {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let (tg, dc, wa) = tokio::join!(
        validate_telegram_token(&client, &state.telegram_bot_token),
        validate_discord_token(&client, &state.discord_bot_token),
        async { validate_whatsapp_phone(&state.whatsapp_phone) },
    );

    ValidationResults {
        telegram: tg,
        discord: dc,
        whatsapp: wa,
    }
}

/// Hit Telegram's `getMe` — succeeds iff `ok: true` in the JSON body.
async fn validate_telegram_token(client: &reqwest::Client, token: &str) -> ValidationStatus {
    if token.is_empty() {
        return ValidationStatus::Skipped;
    }
    let url = format!("https://api.telegram.org/bot{token}/getMe");
    match client.get(&url).send().await {
        Ok(r) => {
            let status = r.status();
            if status.is_success() {
                match r.json::<serde_json::Value>().await {
                    Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => {
                        ValidationStatus::Ok
                    }
                    Ok(_) => ValidationStatus::Failed("API replied ok=false".to_string()),
                    Err(e) => ValidationStatus::Failed(format!("Bad JSON: {e}")),
                }
            } else if status.as_u16() == 401 {
                ValidationStatus::Failed("Unauthorized — token is wrong".to_string())
            } else {
                ValidationStatus::Failed(format!("HTTP {status}"))
            }
        }
        Err(e) if e.is_timeout() => {
            ValidationStatus::Failed("Timed out (host offline?)".to_string())
        }
        Err(e) => ValidationStatus::Failed(format!("Network: {e}")),
    }
}

/// Hit Discord's `users/@me` with a Bot token. 401 = bad token, 200 = good.
async fn validate_discord_token(client: &reqwest::Client, token: &str) -> ValidationStatus {
    if token.is_empty() {
        return ValidationStatus::Skipped;
    }
    let resp = client
        .get("https://discord.com/api/v10/users/@me")
        .header("Authorization", format!("Bot {token}"))
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => ValidationStatus::Ok,
        Ok(r) if r.status().as_u16() == 401 => {
            ValidationStatus::Failed("Unauthorized — token is wrong".to_string())
        }
        Ok(r) => ValidationStatus::Failed(format!("HTTP {}", r.status())),
        Err(e) if e.is_timeout() => {
            ValidationStatus::Failed("Timed out (host offline?)".to_string())
        }
        Err(e) => ValidationStatus::Failed(format!("Network: {e}")),
    }
}

/// Local E.164 format check — `+` followed by 7-15 digits, leading digit 1-9.
/// The bridge will discover unreachable numbers when it pairs; the wizard can
/// at least catch obvious format mistakes without making an API call.
fn validate_whatsapp_phone(phone: &str) -> ValidationStatus {
    if phone.is_empty() {
        return ValidationStatus::Skipped;
    }
    if !phone.starts_with('+') {
        return ValidationStatus::Failed("Must start with + (E.164)".to_string());
    }
    let digits = &phone[1..];
    if digits.len() < 7 || digits.len() > 15 {
        return ValidationStatus::Failed(format!(
            "Must have 7-15 digits after +, got {}",
            digits.len()
        ));
    }
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return ValidationStatus::Failed("Only digits allowed after +".to_string());
    }
    if digits.starts_with('0') {
        return ValidationStatus::Failed("Country code cannot start with 0".to_string());
    }
    ValidationStatus::Ok
}

// ─── Step 8: Review ──────────────────────────────────────────────────────────

async fn step_review(state: &WizardState, config_path: &Path) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let sandbox_str = if state.allow_all_commands {
        "* (all commands)".to_string()
    } else {
        let mut cmds: Vec<&str> = state
            .sandbox_commands
            .iter()
            .filter(|(_, e)| *e)
            .map(|(c, _)| c.as_str())
            .collect();
        if !state.custom_commands.is_empty() {
            cmds.push(&state.custom_commands);
        }
        if cmds.is_empty() {
            "none".to_string()
        } else {
            let mut s = cmds.join(", ");
            if state.allow_pipes {
                s.push_str(" (+pipes)");
            }
            s
        }
    };
    let workspace_note = if state.create_workspace {
        " (will be created)"
    } else {
        ""
    };
    let auth_str = if state.require_auth {
        "required"
    } else {
        "disabled"
    };

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 8)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Review your configuration\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        let rows: &[(&str, String)] = &[
            ("Provider", p.name.to_string()),
            ("Model", state.model.clone()),
            (
                "API key",
                format!(
                    "{}   (stored as SHA-256 hash)",
                    mask_key(&state.admin_key_raw)
                ),
            ),
            (
                "Login",
                format!(
                    "{}   {}",
                    state.admin_username,
                    if state.admin_password.is_empty() {
                        "(password disabled)"
                    } else {
                        "(password set)"
                    }
                ),
            ),
            (
                "Workspace",
                format!("{}{workspace_note}", state.workspace_dir),
            ),
            ("Server", format!("{}:{}", state.host, state.port)),
            ("Web UI", format!("{}:{}", state.host, state.web_port)),
            ("Auth", auth_str.to_string()),
            ("Sandbox", sandbox_str.clone()),
            (
                "Coding",
                format!(
                    "file ≤ {} MB, ≤ {} shells, undo {}",
                    state.max_file_size_mb,
                    state.max_concurrent_shells,
                    state.undo_history_size,
                ),
            ),
            ("Messaging", messaging_summary(state)),
        ];

        for (label, value) in rows {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(dim()))?
                .queue(Print(format!("   {label:<11} ")))?
                .queue(SetForegroundColor(rule_fg()))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("{value}\r\n")))?;
        }

        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("\r\n"))?
            .queue(SetForegroundColor(rule_fg()))?
            .queue(Print(format!("   {RULE}\r\n\r\n")))?
            .queue(SetForegroundColor(red()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Write config.toml?  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(dim()))?
            .queue(Print(format!("→ {}\r\n", config_path.display())))?;
        bg(&mut stdout, BG)?;
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "  [Enter] Write    [Esc] Go back    [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => match write_config(state, config_path).await {
                    Ok(()) => return Ok(StepOutcome::Next),
                    Err(e) => {
                        let mut stdout = io::stdout();
                        print_error(&mut stdout, &format!("Failed to write config: {e}"))?;
                        bg(&mut stdout, BG)?;
                        stdout
                            .queue(SetForegroundColor(dim()))?
                            .queue(Print("   Press any key to try again…\r\n"))?;
                        stdout.flush()?;
                        event::read()?;
                    }
                },
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

// ─── Step 9: Done ────────────────────────────────────────────────────────────

async fn step_done(state: &WizardState, config_path: &Path) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let mut selected: u8 = 0;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 9)?;

        print_success(&mut stdout, "Setup complete — agent ready")?;
        stdout.queue(Print("\r\n"))?;

        let summary: &[(&str, String)] = &[
            ("Provider", p.name.to_string()),
            ("Model", state.model.clone()),
            ("Config", format!("{}  (written)", config_path.display())),
            (
                "Workspace",
                format!(
                    "{}  ({})",
                    state.workspace_dir,
                    if state.create_workspace {
                        "initialized"
                    } else {
                        "exists"
                    }
                ),
            ),
        ];
        for (label, value) in summary {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(dim()))?
                .queue(Print(format!("   {label:<11} ")))?
                .queue(SetForegroundColor(rule_fg()))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(white()))?
                .queue(Print(format!("{value}\r\n")))?;
        }

        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("\r\n"))?
            .queue(SetForegroundColor(rule_fg()))?
            .queue(Print(format!("   {RULE}\r\n\r\n")))?;
        bg(&mut stdout, BG)?;

        // Key reveal box
        let box_w: usize = 56;
        let border = "─".repeat(box_w);
        let warn2_text = "⚠ Your admin API key";
        let warn2_w = UnicodeWidthStr::width(warn2_text);
        let dim_pad2 = box_w.saturating_sub(2 + warn2_w);
        stdout
            .queue(SetForegroundColor(red()))?
            .queue(Print(format!("   ┌{border}┐\r\n")))?
            .queue(Print("   │ "))?
            .queue(SetForegroundColor(amber()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(warn2_text))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(dim()))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print(format!("{:<dim_pad2$}", " (shown once)")))?
            .queue(SetForegroundColor(red()))?
            .queue(Print(" │\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!(
                "{:<width$}",
                state.admin_key_raw,
                width = box_w - 2
            )))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(red()))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("│\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print(format!("   └{border}┘\r\n\r\n")))?;
        bg(&mut stdout, BG)?;

        stdout
            .queue(SetForegroundColor(white()))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   What's next?\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        // "xenoclaw serve" is greyed out and unselectable when the CLI
        // provider failed its health checks and the user skipped fixing them.
        let serve_blocked = !state.cli_provider_ready;

        // Clamp selection: if serve is blocked and user is on option 0, move to 1.
        if serve_blocked && selected == 0 {
            selected = 1;
        }

        let options: &[(&str, &str)] = &[
            (
                "xenoclaw serve",
                if serve_blocked {
                    "CLI provider not ready — fix install/login first"
                } else {
                    "start the agent runtime"
                },
            ),
            ("Exit", "configure more later"),
        ];

        for (i, (cmd, desc)) in options.iter().enumerate() {
            let blocked = i == 0 && serve_blocked;
            if blocked {
                // Greyed-out, non-selectable row
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(dim()))?
                    .queue(Print(format!("     [{}]  {cmd:<18} ", i + 1)))?
                    .queue(SetForegroundColor(red()))?
                    .queue(SetAttribute(Attribute::Italic))?
                    .queue(Print(format!("{desc}\r\n")))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else if i as u8 == selected {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(red()))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > [{}]  ", i + 1)))?
                    .queue(SetForegroundColor(white()))?
                    .queue(Print(format!("{cmd:<18} ")))?
                    .queue(SetForegroundColor(text()))?
                    .queue(Print(format!("{desc}\r\n")))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(dim()))?
                    .queue(Print(format!("     [{}]  {cmd:<18} {desc}\r\n", i + 1)))?;
            }
        }

        // Warn if serve is blocked
        if serve_blocked {
            stdout.queue(Print("\r\n"))?;
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(amber()))?
                .queue(Print(
                    "   ⚠  CLI provider not ready. Re-run `xenoclaw setup` after\r\n",
                ))?
                .queue(Print("      installing/logging in to enable serve.\r\n"))?;
            bg(&mut stdout, BG)?;
        }

        stdout.flush()?;
        let footer = if serve_blocked {
            "  [↑↓/jk] Navigate  [Enter] Exit  [2] Exit"
        } else {
            "  [↑↓/jk] Navigate  [Enter] Launch  [1/2] Quick select"
        };
        print_footer(&mut stdout, footer)?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if !serve_blocked {
                        selected = 0;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = 1;
                }
                KeyCode::Char('1') if !serve_blocked => {
                    return launch_or_exit(0);
                }
                KeyCode::Char('2') => {
                    return launch_or_exit(1);
                }
                KeyCode::Enter => {
                    return launch_or_exit(selected);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

fn launch_or_exit(selected: u8) -> Result<StepOutcome> {
    if selected == 0 {
        terminal::disable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.queue(LeaveAlternateScreen)?;
        stdout.queue(ResetColor)?;
        stdout.flush()?;
        println!("\r\nStarting xenoclaw serve...\r\n");

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new("xenoclaw").arg("serve").exec();
            eprintln!("Failed to exec xenoclaw serve: {err}");
            std::process::exit(1);
        }

        #[cfg(not(unix))]
        {
            let status = std::process::Command::new("xenoclaw").arg("serve").status();
            match status {
                Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                Err(e) => {
                    eprintln!("Failed to start xenoclaw serve: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(StepOutcome::Next)
}

// ─── Quit Confirmation ───────────────────────────────────────────────────────

fn render_confirm_quit(stdout: &mut io::Stdout) -> io::Result<()> {
    clear_screen(stdout)?;

    // Paint a red-tinted callout for the warning
    bg(stdout, BG)?;
    stdout.queue(Print("\r\n\r\n"))?;
    if bg_enabled() {
        stdout.queue(SetBackgroundColor(red_deep()))?;
    }
    stdout
        .queue(SetForegroundColor(white()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("   Quit setup?   "))?
        .queue(SetAttribute(Attribute::Reset))?;
    if bg_enabled() {
        stdout.queue(SetBackgroundColor(BG))?;
    }
    stdout
        .queue(SetForegroundColor(text()))?
        .queue(Print("  Config has not been written.\r\n\r\n"))?
        .queue(SetForegroundColor(dim()))?
        .queue(Print("   ["))?
        .queue(SetForegroundColor(red()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Enter"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(dim()))?;
    bg(stdout, BG)?;
    stdout
        .queue(Print("] Quit    ["))?
        .queue(SetForegroundColor(white()))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Esc"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(dim()))?;
    bg(stdout, BG)?;
    stdout.queue(Print("] Cancel\r\n"))?;
    stdout.flush()
}

async fn confirm_quit() -> Result<bool> {
    let mut stdout = io::stdout();
    render_confirm_quit(&mut stdout)?;

    loop {
        match event::read()? {
            Event::Key(key) => match key.code {
                KeyCode::Enter => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            },
            Event::Resize(_, _) => {
                render_confirm_quit(&mut stdout)?;
            }
            _ => {}
        }
    }
}

// ─── Config Writer ───────────────────────────────────────────────────────────

async fn write_config(state: &WizardState, config_path: &Path) -> Result<()> {
    let p = &PROVIDERS[state.provider_idx];

    let xenoclaw_home = config_path.parent().unwrap_or_else(|| Path::new("."));
    let data_dir = xenoclaw_home.join("data");
    let log_dir = xenoclaw_home.join("logs");

    let mut all_cmds: Vec<String> = Vec::new();
    if state.allow_all_commands {
        all_cmds.push("*".to_string());
    } else {
        for (cmd, enabled) in &state.sandbox_commands {
            if *enabled {
                all_cmds.push(cmd.clone());
            }
        }
        if !state.custom_commands.is_empty() {
            for cmd in state.custom_commands.split(',') {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    all_cmds.push(trimmed.to_string());
                }
            }
        }
    }

    let command_allowlist = if all_cmds.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = all_cmds.iter().map(|c| format!("\"{c}\"")).collect();
        items.join(", ")
    };

    let is_cli_provider = p.default_base_url.is_empty() && !p.needs_api_key;

    let api_key_line = if is_cli_provider {
        String::new()
    } else if p.needs_api_key && !state.api_key.is_empty() {
        format!("api_key       = \"{}\"", state.api_key)
    } else {
        "api_key       = \"\"".to_string()
    };

    let base_url_line = format!("base_url      = \"{}\"", state.base_url);

    let timeout_value: u32 = state.tool_timeout.parse().unwrap_or(30);
    let timeout: u32 = match state.timeout_unit {
        TimeoutUnit::Seconds => timeout_value,
        TimeoutUnit::Minutes => timeout_value * 60,
        TimeoutUnit::Hours => timeout_value * 3600,
    };

    let toml_content = format!(
        r#"# XenoClaw configuration — generated by `xenoclaw setup`

[general]
agent_name = "xenoclaw"
data_dir   = "{data_dir}"
log_dir    = "{log_dir}"

[[llm.providers]]
name          = "{provider_name}"
provider_type = "{provider_type}"
{api_key_line}
{base_url_line}
model         = "{model}"
priority      = 1
timeout_seconds = 30
max_tokens    = 4096

[security]
session_timeout_minutes = 30
max_failed_attempts     = 5
lockout_minutes         = 15
admin_username          = "{admin_username}"
admin_password_hash     = "{admin_password_hash}"
admin_key_hash          = "{admin_key_hash}"

[security.resource_limits]
max_memory_mb    = 512
max_cpu_percent  = 80
max_processes    = 10

[scheduler]

[web]
host = "{host}"
port = {port}
dir  = "{web_dir}"

[api]
host = "{host}"
port = {port}

[plugins]

[monitoring]
"#,
        provider_name = p.name.to_lowercase().replace(' ', "-"),
        provider_type = p.provider_type,
        api_key_line = api_key_line,
        base_url_line = base_url_line,
        model = state.model,
        host = state.host,
        port = state.port,
        web_dir = resolve_wizard_web_dir().display(),
        data_dir = data_dir.display(),
        log_dir = log_dir.display(),
        admin_username = state.admin_username,
        admin_password_hash = state.admin_password_hash,
        admin_key_hash = state.admin_key_hash,
    );

    let coding_section = {
        let mut blocklist_items: Vec<String> = vec!["\"rm -rf\"".to_string(), "\"dd\"".to_string()];
        if !state.allow_pipes {
            blocklist_items.extend([
                "\"|\"".to_string(),
                "\"&&\"".to_string(),
                "\"||\"".to_string(),
                "\";\"".to_string(),
                "\">\"".to_string(),
                "\">>\"".to_string(),
            ]);
        }
        if !state.excluded_commands.is_empty() {
            for cmd in state.excluded_commands.split(',') {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    blocklist_items.push(format!("\"{trimmed}\""));
                }
            }
        }
        let blocklist = blocklist_items.join(", ");
        let max_file_size_mb: u32 = state.max_file_size_mb.parse().unwrap_or(10);
        let max_concurrent_shells: u8 = state.max_concurrent_shells.parse().unwrap_or(5);
        let undo_history_size: usize = state.undo_history_size.parse().unwrap_or(50);
        format!(
            r#"
[coding]
workspace_dirs       = ["{workspace}"]
repository_dirs      = ["{workspace}"]
command_allowlist    = [{allowlist}]
command_blocklist    = [{blocklist}]
max_file_size_mb     = {max_file_size_mb}
max_concurrent_shells = {max_concurrent_shells}
shell_timeout_seconds = {timeout}
undo_history_size    = {undo_history_size}
"#,
            workspace = state.workspace_dir,
            allowlist = command_allowlist,
            blocklist = blocklist,
            timeout = timeout,
            max_file_size_mb = max_file_size_mb,
            max_concurrent_shells = max_concurrent_shells,
            undo_history_size = undo_history_size,
        )
    };

    let messaging_section = build_messaging_section(state);

    let full_content = format!("{toml_content}{coding_section}{messaging_section}");

    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tokio::fs::create_dir_all(&data_dir).await?;
    tokio::fs::create_dir_all(&log_dir).await?;

    tokio::fs::write(config_path, &full_content).await?;

    common::config::load_config_from_str(&full_content)
        .map_err(|e| anyhow::anyhow!("Validation failed: {e}"))?;

    let ws_path = PathBuf::from(&state.workspace_dir);
    tokio::fs::create_dir_all(&ws_path).await?;
    tokio::fs::create_dir_all(ws_path.join("memory")).await?;
    tokio::fs::create_dir_all(ws_path.join("skills")).await?;

    let stubs = [
        (
            "SOUL.md",
            "# Soul\r\n\r\nDefine your agent's core values and ethical constraints here.\r\n",
        ),
        (
            "IDENTITY.md",
            "# Identity\r\n\r\nDefine your agent's name, persona, and tone here.\r\n",
        ),
        (
            "TOOLS.md",
            "# Tools\r\n\r\nTool catalogue will be populated automatically.\r\n",
        ),
        (
            "USER.md",
            "# User Context\r\n\r\nPersistent user context (name, projects, preferences).\r\n",
        ),
        (
            "MEMORY.md",
            "# Long-Term Memory\r\n\r\nAppend-only summary memory.\r\n",
        ),
        (
            "AGENTS.md",
            "# Known Agents\r\n\r\nOther agents and delegation instructions.\r\n",
        ),
        (
            "BOOTSTRAP.md",
            "# Bootstrap\r\n\r\nFirst-run onboarding instructions.\r\n\r\nonboarding_complete = false\r\n",
        ),
    ];

    for (name, content) in stubs {
        let path = ws_path.join(name);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }

    Ok(())
}
