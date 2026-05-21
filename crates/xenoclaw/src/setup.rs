//! First-run setup wizard for XenoClaw.
//!
//! An inline terminal wizard using crossterm's alternate screen.
//!
//! ## Rendering strategy
//!
//! The wizard adapts to terminal capabilities to look correct everywhere:
//!
//! - **True-color terminals (kitty, alacritty, wezterm, modern xterm)**: full
//!   24-bit RGB palette with charcoal BG, red primary, white highlights.
//! - **SSH / dumb terminals**: BG fills are skipped because background-color
//!   sequences are commonly stripped over SSH (PuTTY, mosh, screen, tmux
//!   without truecolor); we paint FG colors only and let the terminal's own
//!   background show through. Detected via `SSH_CONNECTION`, `SSH_TTY`,
//!   `TERM=dumb`, and `NO_COLOR`.
//! - **Narrow terminals (< 80 cols)**: the big ASCII banner is replaced with
//!   a compact `[ XENOCLAW ]` block so nothing wraps.
//!
//! All text is laid out using `unicode-width` for emoji-correct padding.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

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

// ─── Color Palette — Xeno Brand (X3NO: black, red, charcoal accents) ─────────
//
// All colors use explicit RGB to prevent terminal theme remapping (Kitty,
// Alacritty, etc. honor true-color literals; ANSI 16 colors get re-themed).
// Background detection happens at runtime — see `bg_enabled()`.

/// Primary brand red — headers, ► cursor, active selections, "XENOCLAW" banner.
const RED: Color = Color::Rgb {
    r: 255,
    g: 56,
    b: 56,
};
/// Bright crimson for emphasis (commit boxes, key reveals).
const RED_BRIGHT: Color = Color::Rgb {
    r: 255,
    g: 96,
    b: 96,
};
/// Deep blood-red for backgrounds of error/warning callouts.
const RED_DEEP: Color = Color::Rgb {
    r: 120,
    g: 20,
    b: 20,
};
/// Pure white — body text, values, key contents (the "pop" color).
const WHITE: Color = Color::Rgb {
    r: 240,
    g: 240,
    b: 240,
};
/// Soft white-grey — secondary labels, hints.
const TEXT: Color = Color::Rgb {
    r: 200,
    g: 200,
    b: 200,
};
/// Medium grey — inactive menu items, dim text.
const DIM: Color = Color::Rgb {
    r: 130,
    g: 130,
    b: 135,
};
/// Dark grey — horizontal rules, borders.
const RULE_FG: Color = Color::Rgb {
    r: 75,
    g: 75,
    b: 80,
};
/// Success green — ✓ checkmarks ONLY (sparingly, for visual confirmation).
const GREEN: Color = Color::Rgb {
    r: 80,
    g: 220,
    b: 100,
};
/// Amber — warnings, "save this now" callouts.
const AMBER: Color = Color::Rgb {
    r: 255,
    g: 176,
    b: 0,
};

/// Charcoal background — ANSI 256-color #233 (#121212).
/// Using AnsiValue instead of Rgb so the background renders reliably on SSH
/// sessions and web terminals that don't forward 24-bit color support.
const BG: Color = Color::AnsiValue(233);

const TOTAL_STEPS: u8 = 8;
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
/// Disabled only for truly incapable terminals:
/// - `NO_COLOR` env var set — universal opt-out standard
/// - `TERM=dumb` — no color support at all
///
/// SSH sessions are NOT suppressed: BG now uses ANSI 256-color which passes
/// through every SSH layer cleanly. Override with `XENOCLAW_FORCE_BG=0` to
/// disable explicitly, or `XENOCLAW_FORCE_BG=1` to force-enable.
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
    true
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
        .queue(SetForegroundColor(RED))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("XENOCLAW"))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print(" │ "))?
        .queue(SetForegroundColor(WHITE))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Init"))?
        .queue(SetAttribute(Attribute::Reset))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print(format!("  {step} of {TOTAL_STEPS}")))?
        .queue(Print("\r\n"))?
        .queue(SetForegroundColor(RULE_FG))?
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
            .queue(SetForegroundColor(RED))?
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("  {COMPACT_BANNER}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    }

    stdout
        .queue(Print("\r\n"))?
        .queue(SetForegroundColor(TEXT))?
        .queue(Print("                       Agent Runtime\r\n"))?
        .queue(SetForegroundColor(RULE_FG))?
        .queue(Print(format!(" {RULE}\r\n")))?;
    stdout.flush()
}

fn print_footer(stdout: &mut io::Stdout, hint: &str) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let hint_width = UnicodeWidthStr::width(hint);
    let cols = cols as usize;

    stdout
        .queue(cursor::MoveTo(0, rows.saturating_sub(1)))?
        .queue(SetForegroundColor(DIM))?;
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
        .queue(SetForegroundColor(GREEN))?
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
        .queue(SetForegroundColor(RED_BRIGHT))?
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
            7 => step_review(&state, config_path).await?,
            8 => step_done(&state, config_path).await?,
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("  {glyph}  ")))?
            .queue(SetForegroundColor(WHITE))?
            .queue(Print(*name))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("  — {tail}\r\n")))?;
    }

    stdout.queue(Print("\r\n"))?;
    bg(stdout, BG)?;
    stdout
        .queue(SetForegroundColor(TEXT))?
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
            .queue(SetForegroundColor(WHITE))?
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
                    .queue(SetForegroundColor(RED))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > [{num:>2}]  ")))?
                    .queue(SetForegroundColor(WHITE))?
                    .queue(Print(format!("{:<20} ", p.name)))?
                    .queue(SetForegroundColor(TEXT))?
                    .queue(Print(format!("{}\r\n", p.default_model)))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout.queue(SetForegroundColor(DIM))?.queue(Print(format!(
                    "     [{num:>2}]  {:<20} {}\r\n",
                    p.name, p.default_model
                )))?;
            }
        }

        if end < PROVIDERS.len() {
            bg(&mut stdout, BG)?;
            stdout.queue(SetForegroundColor(DIM))?.queue(Print(format!(
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
/// These providers don't need an API key or base URL — they shell out to a
/// locally installed CLI. Show which CLI is required, let the user confirm or
/// change the model, then proceed.
async fn step_cli_provider(state: &mut WizardState) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let mut model_input = TextInput::new(&state.model);
    let mut editing = false;

    let (cli_name, cli_check_cmd, cli_hint) = if p.provider_type == "claude_code" {
        (
            "Claude Code CLI",
            "claude --version",
            "Run `claude auth` to authenticate if not already done.",
        )
    } else {
        (
            "GitHub Copilot CLI",
            "gh copilot --version",
            "Run `gh extension install github/gh-copilot` if not installed.",
        )
    };

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   {}\r\n", p.name)))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout.queue(SetForegroundColor(DIM))?.queue(Print(
            "   No API key or base URL required — uses a local CLI.\r\n\r\n",
        ))?;
        bg(&mut stdout, BG)?;

        // Requirement callout
        stdout
            .queue(SetForegroundColor(AMBER))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Requires: "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(WHITE))?
            .queue(Print(format!("{cli_name}  ")))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("({cli_check_cmd})\r\n")))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("   {cli_hint}\r\n\r\n")))?;
        bg(&mut stdout, BG)?;

        // Model field
        render_text_row(&mut stdout, true, "Model", &model_input, editing)?;
        stdout.queue(Print("\r\n"))?;

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [Enter] Confirm  [E] Edit model  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter if editing => {
                    editing = false;
                }
                KeyCode::Enter => {
                    state.model = model_input.value().to_string();
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('e') | KeyCode::Char('E') if !editing => {
                    editing = true;
                }
                KeyCode::Left if editing => {
                    model_input.move_left();
                }
                KeyCode::Right if editing => {
                    model_input.move_right();
                }
                KeyCode::Backspace if editing => {
                    model_input.backspace();
                }
                KeyCode::Delete if editing => {
                    model_input.delete();
                }
                KeyCode::Char(c) if editing => {
                    model_input.insert(c);
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Provider: ".to_string()))?
            .queue(SetForegroundColor(WHITE))?
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
                    .queue(SetForegroundColor(RED))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > {:<10} ", labels[i])))?
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print("│ "))?
                    .queue(SetForegroundColor(WHITE))?
                    .queue(Print(format!("{display}\r\n")))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(DIM))?
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
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Workspace directory\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout.queue(SetForegroundColor(DIM))?.queue(Print(
            "   Contains SOUL.md, IDENTITY.md, memory/, skills/, etc.\r\n\r\n",
        ))?;
        bg(&mut stdout, BG)?;

        stdout
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   > "))?
            .queue(SetForegroundColor(WHITE))?
            .queue(Print(format!("{}\r\n\r\n", input.display())))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        let path = Path::new(input.value());
        if path.exists() && path.is_dir() {
            let count = std::fs::read_dir(path).map(|d| d.count()).unwrap_or(0);
            stdout
                .queue(SetForegroundColor(GREEN))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   \u{2713} ".to_string()))?
                .queue(SetForegroundColor(TEXT))?
                .queue(Print(format!("Found  ({count} entries)\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            stdout
                .queue(SetForegroundColor(AMBER))?
                .queue(Print("   ◆ Does not exist yet.\r\n"))?
                .queue(SetForegroundColor(WHITE))?
                .queue(Print("     ["))?
                .queue(SetForegroundColor(RED))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("C"))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetForegroundColor(WHITE))?;
            bg(&mut stdout, BG)?;
            stdout
                .queue(Print("] Create stub workspace    ["))?
                .queue(SetForegroundColor(RED))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("S"))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetForegroundColor(WHITE))?;
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
            .queue(SetForegroundColor(WHITE))?
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
        let auth_color = if require_auth { GREEN } else { AMBER };
        if field == 3 {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(RED))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   > Require authentication?  "))?
                .queue(SetForegroundColor(auth_color))?
                .queue(Print(format!("[{auth_str}]\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(DIM))?
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label:<14} ")))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print("│ "))?
            .queue(SetForegroundColor(WHITE))?
            .queue(Print(format!("{}\r\n", input.display())))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   API Key  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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
            .queue(SetForegroundColor(RED))?
            .queue(Print(format!("   ┌{border}┐\r\n")))?
            .queue(Print("   │ "))?
            .queue(SetForegroundColor(AMBER))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(warn_text))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(DIM))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print(format!(
                "{:<dim_pad$}",
                " — will not be shown again.",
            )))?
            .queue(SetForegroundColor(RED))?
            .queue(Print(" │\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!(
                "{:<width$}",
                state.admin_key_raw,
                width = box_w - 2
            )))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(RED))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("│\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print(format!("   └{border}┘\r\n")))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print("   [R] Regenerate key\r\n\r\n"))?;
        bg(&mut stdout, BG)?;

        // Web UI login section
        stdout
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Web UI Login  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print("(username + password)\r\n\r\n"))?;
        bg(&mut stdout, BG)?;

        render_field(&mut stdout, field == 1, "Username", &username_input)?;

        // Password — masked when not editing
        if field == 2 {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(RED))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   > {:<14} ", "Password")))?
                .queue(SetForegroundColor(DIM))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(WHITE))?
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
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("     {:<14} │ {masked}\r\n", "Password")))?;
        }

        stdout.queue(Print("\r\n"))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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
    let mut editing: Option<usize> = None;

    loop {
        let cmd_count = state.sandbox_commands.len();
        let row_allow_all = 0;
        let row_pipes = cmd_count + 1;
        let row_custom = cmd_count + 2;
        let row_exclude = cmd_count + 3;
        let row_timeout = cmd_count + 4;
        let total_rows = cmd_count + 5;

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
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Shell Sandbox\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout.queue(SetForegroundColor(DIM))?.queue(Print(
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
                .queue(SetForegroundColor(RED))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print("   > Timeout: "))?
                .queue(SetForegroundColor(WHITE))?
                .queue(Print(format!("{display}  ")))?
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("{unit_label}  [Tab to change unit]\r\n")))?
                .queue(SetAttribute(Attribute::Reset))?;
            bg(&mut stdout, BG)?;
        } else {
            bg(&mut stdout, BG)?;
            stdout.queue(SetForegroundColor(DIM))?.queue(Print(format!(
                "     Timeout: {}  {unit_label}\r\n",
                timeout_input.value()
            )))?;
        }

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
                } else {
                    &mut timeout_input
                };
                let is_timeout = edit_row == row_timeout;

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
                        if is_timeout {
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
                    if selected == row_custom || selected == row_exclude || selected == row_timeout
                    {
                        editing = Some(selected);
                    } else {
                        state.custom_commands = custom_input.value().to_string();
                        state.excluded_commands = exclude_input.value().to_string();
                        state.tool_timeout = timeout_input.value().to_string();
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
    } else if enabled {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(TEXT))?
            .queue(Print(format!("     {label}\r\n")))?;
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {label}\r\n")))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(stdout, BG)?;
        if editing {
            stdout
                .queue(SetForegroundColor(WHITE))?
                .queue(Print(format!("     {}\r\n", input.display())))?;
        } else {
            let val = if input.value().is_empty() {
                "(press Enter to type)"
            } else {
                input.value()
            };
            stdout
                .queue(SetForegroundColor(WHITE))?
                .queue(Print(format!("     {val}\r\n")))?;
        }
    } else {
        bg(stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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

// ─── Step 7: Review ──────────────────────────────────────────────────────────

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
        print_header(&mut stdout, 7)?;

        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(WHITE))?
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
        ];

        for (label, value) in rows {
            bg(&mut stdout, BG)?;
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("   {label:<11} ")))?
                .queue(SetForegroundColor(RULE_FG))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(WHITE))?
                .queue(Print(format!("{value}\r\n")))?;
        }

        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("\r\n"))?
            .queue(SetForegroundColor(RULE_FG))?
            .queue(Print(format!("   {RULE}\r\n\r\n")))?
            .queue(SetForegroundColor(RED))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Write config.toml?  "))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(SetForegroundColor(DIM))?
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
                            .queue(SetForegroundColor(DIM))?
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

// ─── Step 8: Done ────────────────────────────────────────────────────────────

async fn step_done(state: &WizardState, config_path: &Path) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let mut selected: u8 = 0;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 8)?;

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
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("   {label:<11} ")))?
                .queue(SetForegroundColor(RULE_FG))?
                .queue(Print("│ "))?
                .queue(SetForegroundColor(WHITE))?
                .queue(Print(format!("{value}\r\n")))?;
        }

        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("\r\n"))?
            .queue(SetForegroundColor(RULE_FG))?
            .queue(Print(format!("   {RULE}\r\n\r\n")))?;
        bg(&mut stdout, BG)?;

        // Key reveal box
        let box_w: usize = 56;
        let border = "─".repeat(box_w);
        let warn2_text = "⚠ Your admin API key";
        let warn2_w = UnicodeWidthStr::width(warn2_text);
        let dim_pad2 = box_w.saturating_sub(2 + warn2_w);
        stdout
            .queue(SetForegroundColor(RED))?
            .queue(Print(format!("   ┌{border}┐\r\n")))?
            .queue(Print("   │ "))?
            .queue(SetForegroundColor(AMBER))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(warn2_text))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(DIM))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print(format!("{:<dim_pad2$}", " (shown once)")))?
            .queue(SetForegroundColor(RED))?
            .queue(Print(" │\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!(
                "{:<width$}",
                state.admin_key_raw,
                width = box_w - 2
            )))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetForegroundColor(RED))?;
        bg(&mut stdout, BG)?;
        stdout
            .queue(Print("│\r\n"))?
            .queue(Print(format!("   │{}│\r\n", " ".repeat(box_w))))?
            .queue(Print(format!("   └{border}┘\r\n\r\n")))?;
        bg(&mut stdout, BG)?;

        stdout
            .queue(SetForegroundColor(WHITE))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   What's next?\r\n\r\n"))?
            .queue(SetAttribute(Attribute::Reset))?;
        bg(&mut stdout, BG)?;

        let options = [
            ("xenoclaw serve", "start the agent runtime"),
            ("Exit", "configure more later"),
        ];
        for (i, (cmd, desc)) in options.iter().enumerate() {
            if i as u8 == selected {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(RED))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   > [{}]  ", i + 1)))?
                    .queue(SetForegroundColor(WHITE))?
                    .queue(Print(format!("{cmd:<18} ")))?
                    .queue(SetForegroundColor(TEXT))?
                    .queue(Print(format!("{desc}\r\n")))?
                    .queue(SetAttribute(Attribute::Reset))?;
                bg(&mut stdout, BG)?;
            } else {
                bg(&mut stdout, BG)?;
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     [{}]  {cmd:<18} {desc}\r\n", i + 1)))?;
            }
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "  [↑↓/jk] Navigate  [Enter] Launch  [1/2] Quick select",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    selected = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    selected = 1;
                }
                KeyCode::Char('1') => {
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
        stdout.queue(SetBackgroundColor(RED_DEEP))?;
    }
    stdout
        .queue(SetForegroundColor(WHITE))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("   Quit setup?   "))?
        .queue(SetAttribute(Attribute::Reset))?;
    if bg_enabled() {
        stdout.queue(SetBackgroundColor(BG))?;
    }
    stdout
        .queue(SetForegroundColor(TEXT))?
        .queue(Print("  Config has not been written.\r\n\r\n"))?
        .queue(SetForegroundColor(DIM))?
        .queue(Print("   ["))?
        .queue(SetForegroundColor(RED))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Enter"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(DIM))?;
    bg(stdout, BG)?;
    stdout
        .queue(Print("] Quit    ["))?
        .queue(SetForegroundColor(WHITE))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("Esc"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(DIM))?;
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

[security.resource_limits]
max_memory_mb    = 512
max_cpu_percent  = 80
max_processes    = 10

[scheduler]

[web]
host = "{host}"
port = {web_port}

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
        web_port = state.web_port,
        data_dir = data_dir.display(),
        log_dir = log_dir.display(),
        admin_username = state.admin_username,
        admin_password_hash = state.admin_password_hash,
    );

    let coding_section = if !all_cmds.is_empty() {
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
        format!(
            r#"
[coding]
workspace_dirs       = ["{workspace}"]
repository_dirs      = ["{workspace}"]
command_allowlist    = [{allowlist}]
command_blocklist    = [{blocklist}]
max_file_size_mb     = 10
max_concurrent_shells = 5
shell_timeout_seconds = {timeout}
undo_history_size    = 50
"#,
            workspace = state.workspace_dir,
            allowlist = command_allowlist,
            blocklist = blocklist,
            timeout = timeout,
        )
    } else {
        String::new()
    };

    let full_content = format!("{toml_content}{coding_section}");

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
