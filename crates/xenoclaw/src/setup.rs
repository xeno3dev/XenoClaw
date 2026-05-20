//! First-run setup wizard for XenoClaw.
//!
//! An inline terminal wizard using crossterm's alternate screen.
//! through initial configuration and produces a valid config.toml.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor, ResetColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    QueueableCommand,
};

use security_layer::auth::ApiKeyAuthenticator;

// ─── Color Palette ───────────────────────────────────────────────────────────
// Using explicit RGB to prevent terminal theme remapping (Kitty, Alacritty, etc.)
/// Headers, brand, ► cursor, ✓/✗ symbols, prompts (bright green, bold)
const BRAND: Color = Color::Rgb { r: 0, g: 255, b: 0 };
/// Body text, menu labels, values, key box contents (green)
const SUCCESS: Color = Color::Rgb { r: 0, g: 220, b: 0 };
/// Dim/secondary text, horizontal rules, footer hints, inactive menu items
const DIM: Color = Color::Rgb { r: 0, g: 120, b: 0 };

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
    ProviderDef { name: "Anthropic", provider_type: "anthropic", default_model: "claude-sonnet-4-20250514", default_base_url: "https://api.anthropic.com", needs_api_key: true },
    ProviderDef { name: "Google Gemini", provider_type: "open_ai_compatible", default_model: "gemini-2.5-pro", default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai", needs_api_key: true },
    ProviderDef { name: "OpenAI", provider_type: "open_ai_compatible", default_model: "gpt-4o", default_base_url: "https://api.openai.com/v1", needs_api_key: true },
    ProviderDef { name: "AWS Bedrock", provider_type: "open_ai_compatible", default_model: "anthropic.claude-sonnet-4-20250514-v1:0", default_base_url: "https://bedrock-runtime.us-east-1.amazonaws.com", needs_api_key: true },
    ProviderDef { name: "OpenRouter", provider_type: "open_ai_compatible", default_model: "anthropic/claude-sonnet-4-20250514", default_base_url: "https://openrouter.ai/api/v1", needs_api_key: true },
    ProviderDef { name: "Together AI", provider_type: "open_ai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo", default_base_url: "https://api.together.xyz/v1", needs_api_key: true },
    ProviderDef { name: "Mistral AI", provider_type: "open_ai_compatible", default_model: "mistral-large-latest", default_base_url: "https://api.mistral.ai/v1", needs_api_key: true },
    ProviderDef { name: "Fireworks AI", provider_type: "open_ai_compatible", default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct", default_base_url: "https://api.fireworks.ai/inference/v1", needs_api_key: true },
    ProviderDef { name: "DeepSeek", provider_type: "open_ai_compatible", default_model: "deepseek-chat", default_base_url: "https://api.deepseek.com/v1", needs_api_key: true },
    ProviderDef { name: "Groq", provider_type: "open_ai_compatible", default_model: "llama-3.3-70b-versatile", default_base_url: "https://api.groq.com/openai/v1", needs_api_key: true },
    ProviderDef { name: "xAI", provider_type: "open_ai_compatible", default_model: "grok-3", default_base_url: "https://api.x.ai/v1", needs_api_key: true },
    ProviderDef { name: "Perplexity", provider_type: "open_ai_compatible", default_model: "sonar-pro", default_base_url: "https://api.perplexity.ai", needs_api_key: true },
    ProviderDef { name: "Cohere", provider_type: "open_ai_compatible", default_model: "command-r-plus", default_base_url: "https://api.cohere.com/v2", needs_api_key: true },
    ProviderDef { name: "AI21 Labs", provider_type: "open_ai_compatible", default_model: "jamba-1.5-large", default_base_url: "https://api.ai21.com/studio/v1", needs_api_key: true },
    ProviderDef { name: "Hugging Face", provider_type: "open_ai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct", default_base_url: "https://api-inference.huggingface.co/v1", needs_api_key: true },
    ProviderDef { name: "Replicate", provider_type: "open_ai_compatible", default_model: "meta/llama-3.3-70b-instruct", default_base_url: "https://api.replicate.com/v1", needs_api_key: true },
    ProviderDef { name: "Requesty", provider_type: "open_ai_compatible", default_model: "anthropic/claude-sonnet-4-20250514", default_base_url: "https://router.requesty.ai/v1", needs_api_key: true },
    ProviderDef { name: "Cerebras", provider_type: "open_ai_compatible", default_model: "llama-3.3-70b", default_base_url: "https://api.cerebras.ai/v1", needs_api_key: true },
    ProviderDef { name: "SambaNova", provider_type: "open_ai_compatible", default_model: "Meta-Llama-3.3-70B-Instruct", default_base_url: "https://api.sambanova.ai/v1", needs_api_key: true },
    ProviderDef { name: "Ollama", provider_type: "ollama", default_model: "llama3.2", default_base_url: "http://localhost:11434", needs_api_key: false },
    ProviderDef { name: "vLLM", provider_type: "open_ai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct", default_base_url: "http://localhost:8000/v1", needs_api_key: false },
    ProviderDef { name: "LM Studio", provider_type: "open_ai_compatible", default_model: "local-model", default_base_url: "http://localhost:1234/v1", needs_api_key: false },
    ProviderDef { name: "Qwen", provider_type: "open_ai_compatible", default_model: "qwen-max", default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1", needs_api_key: true },
    ProviderDef { name: "MiniMax", provider_type: "open_ai_compatible", default_model: "MiniMax-Text-01", default_base_url: "https://api.minimax.chat/v1", needs_api_key: true },
    ProviderDef { name: "Zhipu AI", provider_type: "open_ai_compatible", default_model: "glm-4-plus", default_base_url: "https://open.bigmodel.cn/api/paas/v4", needs_api_key: true },
    ProviderDef { name: "Moonshot AI", provider_type: "open_ai_compatible", default_model: "moonshot-v1-128k", default_base_url: "https://api.moonshot.cn/v1", needs_api_key: true },
    ProviderDef { name: "Baidu Qianfan", provider_type: "open_ai_compatible", default_model: "ernie-4.0-8k", default_base_url: "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop", needs_api_key: true },
    ProviderDef { name: "Claude Code CLI", provider_type: "claude_code", default_model: "claude-sonnet-4-20250514", default_base_url: "", needs_api_key: false },
    ProviderDef { name: "GitHub Copilot CLI", provider_type: "copilot_cli", default_model: "gpt-4o", default_base_url: "", needs_api_key: false },
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

// ─── Rendering Helpers ───────────────────────────────────────────────────────

const BANNER: &str = r#"██╗  ██╗███████╗███╗   ██╗ ██████╗  ██████╗██╗      █████╗ ██╗    ██╗
╚██╗██╔╝██╔════╝████╗  ██║██╔═══██╗██╔════╝██║     ██╔══██╗██║    ██║
 ╚███╔╝ █████╗  ██╔██╗ ██║██║   ██║██║     ██║     ███████║██║ █╗ ██║
 ██╔██╗ ██╔══╝  ██║╚████║║██║   ██║██║     ██║     ██╔══██║██║███╗██║
██╔╝ ██╗███████╗██║ ╚███║ ╚██████╔╝╚██████╗███████╗██║  ██║╚███╔███╔╝
╚═╝  ╚═╝╚══════╝╚═╝  ╚══╝ ╚═════╝  ╚═════╝╚══════╝╚═╝  ╚═╝ ╚══╝╚══╝"#;

fn print_header(stdout: &mut io::Stdout, step: u8) -> io::Result<()> {
    stdout
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("XenoClaw"))?
        .queue(Print("  "))?
        .queue(Print("Init"))?
        .queue(Print(format!("  {} of {}", step, TOTAL_STEPS)))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print("\n"))?
        .queue(SetForegroundColor(DIM))?
        .queue(Print(RULE))?
        .queue(Print("\n\n"))?;
    stdout.flush()
}

fn print_banner(stdout: &mut io::Stdout) -> io::Result<()> {
    stdout.queue(SetBackgroundColor(BG))?;
    stdout.queue(SetForegroundColor(BRAND))?;
    stdout.queue(SetAttribute(Attribute::Bold))?;
    for line in BANNER.lines() {
        stdout.queue(Print(format!("  {line}\n")))?;
    }
    stdout.queue(SetAttribute(Attribute::Reset))?;
    stdout.queue(SetBackgroundColor(BG))?;
    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print("                    Agent Runtime\n"))?;
    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print(format!("  {RULE}\n")))?;
    stdout.flush()
}

/// Background color for the wizard UI — near-black (#0a0a0a).
/// We fill every cell explicitly to ensure this works on VNC, xterm, and
/// terminals that don't honor background color on Clear(All).
const BG: Color = Color::Rgb { r: 10, g: 10, b: 10 };

fn print_footer(stdout: &mut io::Stdout, hint: &str) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    stdout
        .queue(cursor::MoveTo(0, rows - 1))?
        .queue(SetForegroundColor(DIM))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print(hint))?;
    // Fill the rest of the footer line with background
    let remaining = cols as usize - hint.len().min(cols as usize);
    if remaining > 0 {
        stdout.queue(Print(" ".repeat(remaining)))?;
    }
    stdout.queue(SetBackgroundColor(BG))?;
    stdout.flush()
}

fn clear_screen(stdout: &mut io::Stdout) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    // Clear first (honors BG on well-behaved terminals), then fill every cell
    // manually as a fallback for terminals (VNC, xterm) that don't propagate
    // SetBackgroundColor through Clear(All).
    stdout.queue(cursor::MoveTo(0, 0))?;
    stdout.queue(SetBackgroundColor(BG))?;
    stdout.queue(Clear(ClearType::All))?;
    stdout.queue(SetBackgroundColor(BG))?;
    let blank_line = " ".repeat(cols as usize);
    for _ in 0..rows {
        stdout.queue(Print(&blank_line))?;
    }
    stdout
        .queue(cursor::MoveTo(0, 0))?
        .queue(SetBackgroundColor(BG))?;
    stdout.flush()
}

fn print_success(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    stdout
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("✓ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print("\n"))?;
    stdout.flush()
}

fn print_error(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    stdout
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("✗ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print("\n"))?;
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
        Self { text: initial.to_string(), cursor: len }
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
            self.cursor += self.text[self.cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
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
    // Terminal size check
    let (cols, rows) = terminal::size()?;
    if cols < 80 || rows < 24 {
        eprintln!(
            "xenoclaw setup requires a terminal at least 80×24. \
             Current size: {}×{}. Please resize and try again.",
            cols, rows
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

async fn step_welcome(_state: &WizardState) -> Result<StepOutcome> {
    let mut stdout = io::stdout();
    clear_screen(&mut stdout)?;
    print_header(&mut stdout, 1)?;

    stdout.queue(Print("\n"))?;
    print_banner(&mut stdout)?;

    stdout.queue(Print("\n"))?;
    // XenoClaw-specific feature highlights
    stdout
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  ⚡"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Dual-mode: General 24/7 agent + Coding agent (hot-swap)\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🔀"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Multi-provider LLM failover (27 providers supported)\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  💬"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Telegram, Discord & WhatsApp bridge — chat from anywhere\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🧩"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" WASM plugin system with hot-reload\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🛡"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Sandboxed execution, RBAC, filesystem & network allowlists\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  📡"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetBackgroundColor(BG))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Always-on: auto-restart, health checks, SIGHUP hot-reload\n"))?
        .queue(SetBackgroundColor(BG))?;

    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print("  Self-hosted. Your infrastructure. Your data. Your rules.\n"))?
        .queue(SetBackgroundColor(BG))?;

    stdout.queue(Print("\n"))?;
    stdout.flush()?;

    print_footer(&mut stdout, "[Enter] Begin setup  [Esc] Cancel")?;

    loop {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => return Ok(StepOutcome::Next),
                KeyCode::Esc => return Ok(StepOutcome::Quit),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
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

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Select your LLM provider:\n\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;

        // Ensure selected item is visible
        if selected < scroll_offset {
            scroll_offset = selected;
        } else if selected >= scroll_offset + page_size {
            scroll_offset = selected - page_size + 1;
        }

        let end = (scroll_offset + page_size).min(PROVIDERS.len());
        for i in scroll_offset..end {
            let p = &PROVIDERS[i];
            let num = i + 1;
            if i == selected {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!(
                        "   ► [{num:>2}]  {:<20} {}\n",
                        p.name, p.default_model
                    )))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(SetBackgroundColor(BG))?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!(
                        "     [{num:>2}]  {:<20} {}\n",
                        p.name, p.default_model
                    )))?
                    .queue(SetBackgroundColor(BG))?;
            }
        }

        if end < PROVIDERS.len() {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!(
                    "\n   ... and {} more (scroll down)\n",
                    PROVIDERS.len() - end
                )))?
                .queue(SetBackgroundColor(BG))?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[↑↓/jk] Navigate  [Enter] Choose  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 { selected -= 1; }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < PROVIDERS.len() - 1 { selected += 1; }
                }
                KeyCode::Enter => {
                    state.provider_idx = selected;
                    let p = &PROVIDERS[selected];
                    state.base_url = p.default_base_url.to_string();
                    state.model = p.default_model.to_string();
                    // Sub-screen for API key
                    if p.needs_api_key {
                        let outcome = step_provider_details(state).await?;
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

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   Provider: {}\n\n", p.name)))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;

        for (i, input) in inputs.iter().enumerate() {
            let display = if masked[i] && !input.value().is_empty() {
                mask_key(input.value())
            } else if i == field {
                input.display()
            } else {
                input.value().to_string()
            };
            if i == field {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► {:<10} > {display}\n", labels[i])))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(SetBackgroundColor(BG))?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     {:<10} > {display}\n", labels[i])))?
                    .queue(SetBackgroundColor(BG))?;
            }
        }

        stdout.queue(Print("\n"))?;
        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[Tab] Next field  [←→] Move cursor  [Enter] Confirm  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => { field = (field + 1) % 3; }
                KeyCode::BackTab => { field = if field == 0 { 2 } else { field - 1 }; }
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

        stdout
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(
                "   Workspace directory (contains SOUL.md, IDENTITY.md, etc.):\n\n"
            ))?;
        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   > {}\n\n", input.display())))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;

        let path = Path::new(input.value());
        if path.exists() && path.is_dir() {
            let count = std::fs::read_dir(path)
                .map(|d| d.count())
                .unwrap_or(0);
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ✓  Found: {}  ({count} files)\n", input.value())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print("   Directory does not exist.\n"))?
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print("   [C] Create stub workspace    [S] Skip for now\n"))?
                .queue(SetBackgroundColor(BG))?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[←→] Move cursor  [Enter] Confirm  [C] Create  [S] Skip  [Esc] Back",
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

        let auth_str = if require_auth { "Yes" } else { "No" };

        // Host field
        if field == 0 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   Bind host:      > {}\n", host_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("   Bind host:      > {}\n", host_input.value())))?
                .queue(SetBackgroundColor(BG))?;
        }
        // API Port field
        if field == 1 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   API port:       > {}\n", port_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("   API port:       > {}\n", port_input.value())))?
                .queue(SetBackgroundColor(BG))?;
        }
        // Web UI Port field
        if field == 2 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   Web UI port:    > {}\n", web_port_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("   Web UI port:    > {}\n", web_port_input.value())))?
                .queue(SetBackgroundColor(BG))?;
        }
        stdout.queue(Print("\n"))?;
        // Auth toggle
        if field == 3 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ► Require authentication?  [{auth_str}]\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("     Require authentication?  [{auth_str}]\n")))?
                .queue(SetBackgroundColor(BG))?;
        }
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "[Tab] Next field  [←→] Move cursor  [Enter] Confirm  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => { field = (field + 1) % 4; }
                KeyCode::BackTab => { field = if field == 0 { 3 } else { field - 1 }; }
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
                KeyCode::Left => {
                    match field { 0 => host_input.move_left(), 1 => port_input.move_left(), 2 => web_port_input.move_left(), _ => {} }
                }
                KeyCode::Right => {
                    match field { 0 => host_input.move_right(), 1 => port_input.move_right(), 2 => web_port_input.move_right(), _ => {} }
                }
                KeyCode::Home => {
                    match field { 0 => host_input.move_home(), 1 => port_input.move_home(), 2 => web_port_input.move_home(), _ => {} }
                }
                KeyCode::End => {
                    match field { 0 => host_input.move_end(), 1 => port_input.move_end(), 2 => web_port_input.move_end(), _ => {} }
                }
                KeyCode::Backspace => {
                    match field { 0 => host_input.backspace(), 1 => port_input.backspace(), 2 => web_port_input.backspace(), _ => {} }
                }
                KeyCode::Delete => {
                    match field { 0 => host_input.delete(), 1 => port_input.delete(), 2 => web_port_input.delete(), _ => {} }
                }
                KeyCode::Char(c) => {
                    match field {
                        0 => host_input.insert(c),
                        1 => { if c.is_ascii_digit() { port_input.insert(c); } }
                        2 => { if c.is_ascii_digit() { web_port_input.insert(c); } }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

// ─── Step 5: Authentication ──────────────────────────────────────────────────

async fn step_api_key(state: &mut WizardState) -> Result<StepOutcome> {
    // Generate key if not already generated
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
        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   API Key (for programmatic access)\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;
        stdout.queue(Print(
            "   ┌─────────────────────────────────────────────────┐\n\
             \x20  │  Save this now — it will NOT be shown again.    │\n\
             \x20  │                                                 │\n"
        ))?;
        stdout
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("{:<47}", &state.admin_key_raw)))?
            .queue(SetBackgroundColor(BG))?
            .queue(Print("│\n"))?;
        stdout.queue(Print(
            "   │                                                 │\n\
             \x20  └─────────────────────────────────────────────────┘\n"
        ))?;
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print("   [R] Regenerate key\n\n"))?
            .queue(SetBackgroundColor(BG))?;

        // Web UI login section
        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Web UI Login (username + password)\n\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;

        // Username
        if field == 1 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ► Username:  > {}\n", username_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("     Username:  > {}\n", username_input.value())))?
                .queue(SetBackgroundColor(BG))?;
        }

        // Password
        if field == 2 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ► Password:  > {}\n", password_input.display())))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            let masked = if password_input.value().is_empty() {
                "(not set — password login disabled)".to_string()
            } else {
                "●".repeat(password_input.value().len())
            };
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("     Password:  > {masked}\n")))?
                .queue(SetBackgroundColor(BG))?;
        }

        stdout.queue(Print("\n"))?;
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print("   Both API key and password auth work for the web UI.\n"))?
            .queue(Print("   Leave password empty to disable password login.\n"))?
            .queue(SetBackgroundColor(BG))?;

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[Tab] Next field  [←→] Cursor  [R] Regen key  [Enter] Accept  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => { field = (field + 1) % 3; }
                KeyCode::BackTab => { field = if field == 0 { 2 } else { field - 1 }; }
                KeyCode::Enter => {
                    state.admin_username = username_input.value().to_string();
                    state.admin_password = password_input.value().to_string();
                    if !state.admin_password.is_empty() {
                        // Hash the password with bcrypt
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
                // Text input for username/password fields
                KeyCode::Left => {
                    match field { 1 => username_input.move_left(), 2 => password_input.move_left(), _ => {} }
                }
                KeyCode::Right => {
                    match field { 1 => username_input.move_right(), 2 => password_input.move_right(), _ => {} }
                }
                KeyCode::Home => {
                    match field { 1 => username_input.move_home(), 2 => password_input.move_home(), _ => {} }
                }
                KeyCode::End => {
                    match field { 1 => username_input.move_end(), 2 => password_input.move_end(), _ => {} }
                }
                KeyCode::Backspace => {
                    match field { 1 => username_input.backspace(), 2 => password_input.backspace(), _ => {} }
                }
                KeyCode::Delete => {
                    match field { 1 => username_input.delete(), 2 => password_input.delete(), _ => {} }
                }
                KeyCode::Char(c) if field >= 1 => {
                    match field { 1 => username_input.insert(c), 2 => password_input.insert(c), _ => {} }
                }
                _ => {}
            }
        }
    }
}

/// Hash a password using SHA-256 (for config storage).
/// In production, bcrypt would be used, but that requires the bcrypt crate
/// which is already a workspace dependency. For the setup wizard we store
/// a bcrypt hash.
fn hash_password(password: &str) -> String {
    // Use bcrypt with default cost
    bcrypt::hash(password, bcrypt::DEFAULT_COST).unwrap_or_else(|_| {
        // Fallback to SHA-256 if bcrypt fails
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
    let mut editing: Option<usize> = None; // Which row is being text-edited

    loop {
        let cmd_count = state.sandbox_commands.len();
        // Layout rows:
        // 0 = allow_all
        // 1..=cmd_count = individual commands
        // cmd_count+1 = allow_pipes
        // cmd_count+2 = custom commands (text)
        // cmd_count+3 = excluded commands (text)
        // cmd_count+4 = timeout (text + unit)
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

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Shell Sandbox Configuration\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print("   (Space to toggle, Enter to edit text fields, Tab to cycle unit)\n\n"))?
            .queue(SetBackgroundColor(BG))?;

        // Allow all
        let all_check = if state.allow_all_commands { "✓" } else { " " };
        render_toggle_row(&mut stdout, selected == row_allow_all, &format!("[{all_check}] * (allow ALL commands)"))?;

        stdout.queue(Print("\n"))?;

        // Individual commands
        for (i, (cmd, enabled)) in state.sandbox_commands.iter().enumerate() {
            let check = if *enabled || state.allow_all_commands { "✓" } else { " " };
            let row = i + 1;
            let is_active = selected == row && editing.is_none();
            let color = if state.allow_all_commands && !is_active { SUCCESS } else { DIM };
            if is_active {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► [{check}] {cmd}\n")))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(SetBackgroundColor(BG))?;
            } else {
                stdout
                    .queue(SetForegroundColor(color))?
                    .queue(Print(format!("     [{check}] {cmd}\n")))?
                    .queue(SetBackgroundColor(BG))?;
            }
        }

        stdout.queue(Print("\n"))?;

        // Allow pipes
        let pipe_check = if state.allow_pipes { "✓" } else { " " };
        render_toggle_row(&mut stdout, selected == row_pipes, &format!("[{pipe_check}] Allow pipes & operators (|, &&, ||, ;, >)"))?;

        stdout.queue(Print("\n"))?;

        // Custom commands
        let is_editing_custom = editing == Some(row_custom);
        render_text_row(&mut stdout, selected == row_custom || is_editing_custom, "Add commands (comma-separated, wildcards: python*):", &custom_input, is_editing_custom)?;

        // Excluded commands
        let is_editing_exclude = editing == Some(row_exclude);
        render_text_row(&mut stdout, selected == row_exclude || is_editing_exclude, "Exclude commands (comma-separated):", &exclude_input, is_editing_exclude)?;

        stdout.queue(Print("\n"))?;

        // Timeout with unit
        let is_editing_timeout = editing == Some(row_timeout);
        if selected == row_timeout || is_editing_timeout {
            let display = if is_editing_timeout { timeout_input.display() } else { timeout_input.value().to_string() };
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ► Timeout: > {display}  {unit_label}  [Tab to change unit]\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(SetBackgroundColor(BG))?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("     Timeout: > {}  {unit_label}\n", timeout_input.value())))?
                .queue(SetBackgroundColor(BG))?;
        }

        stdout.flush()?;
        let footer = if editing.is_some() {
            "[←→] Cursor  [Enter/Tab] Done editing  [Esc] Cancel edit"
        } else {
            "[↑↓] Navigate  [Space] Toggle  [Enter] Edit/Next  [Esc] Back"
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
                    KeyCode::Enter => { editing = None; }
                    KeyCode::Esc => { editing = None; }
                    KeyCode::Tab if is_timeout => {
                        // Cycle timeout unit
                        state.timeout_unit = match state.timeout_unit {
                            TimeoutUnit::Seconds => TimeoutUnit::Minutes,
                            TimeoutUnit::Minutes => TimeoutUnit::Hours,
                            TimeoutUnit::Hours => TimeoutUnit::Seconds,
                        };
                    }
                    KeyCode::Tab => { editing = None; }
                    KeyCode::Char(c) => {
                        if is_timeout {
                            if c.is_ascii_digit() { input.insert(c); }
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
                    if selected > 0 { selected -= 1; }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < total_rows - 1 { selected += 1; }
                }
                KeyCode::Char(' ') => {
                    if selected == row_allow_all {
                        state.allow_all_commands = !state.allow_all_commands;
                    } else if selected >= 1 && selected <= cmd_count {
                        state.sandbox_commands[selected - 1].1 = !state.sandbox_commands[selected - 1].1;
                    } else if selected == row_pipes {
                        state.allow_pipes = !state.allow_pipes;
                    }
                }
                KeyCode::Enter => {
                    if selected == row_custom || selected == row_exclude || selected == row_timeout {
                        editing = Some(selected);
                    } else {
                        // Save and advance
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

fn render_toggle_row(stdout: &mut io::Stdout, active: bool, label: &str) -> io::Result<()> {
    if active {
        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   ► {label}\n")))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;
    } else {
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("     {label}\n")))?
            .queue(SetBackgroundColor(BG))?;
    }
    Ok(())
}

fn render_text_row(stdout: &mut io::Stdout, active: bool, label: &str, input: &TextInput, editing: bool) -> io::Result<()> {
    if active {
        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   ► {label}\n")))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?
            .queue(SetForegroundColor(SUCCESS))?;
        if editing {
            stdout.queue(Print(format!("     > {}\n", input.display())))?;
        } else {
            let val = if input.value().is_empty() { "(press Enter to type)" } else { input.value() };
            stdout.queue(Print(format!("     > {val}\n")))?;
        }
        stdout.queue(SetBackgroundColor(BG))?;
    } else {
        stdout
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("     {label}\n")))?;
        let val = if input.value().is_empty() { "(none)" } else { input.value() };
        stdout.queue(Print(format!("     > {val}\n")))?;
        stdout.queue(SetBackgroundColor(BG))?;
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
        // Add custom commands
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
    let auth_str = if state.require_auth { "required" } else { "disabled" };

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 7)?;

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   Review your configuration:\n\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("   Provider:   {}\n", p.name)))?
            .queue(Print(format!("   Model:      {}\n", state.model)))?
            .queue(Print(format!(
                "   API key:    {}  (stored as SHA-256 hash)\n",
                mask_key(&state.admin_key_raw)
            )))?
            .queue(Print(format!("   Login:      {}  {}\n",
                state.admin_username,
                if state.admin_password.is_empty() { "(password disabled)" } else { "(password set)" }
            )))?
            .queue(Print(format!(
                "   Workspace:  {}{}\n",
                state.workspace_dir, workspace_note
            )))?
            .queue(Print(format!(
                "   Server:     {}:{}\n",
                state.host, state.port
            )))?
            .queue(Print(format!("   Auth:       {}\n", auth_str)))?
            .queue(Print(format!("   Sandbox:    {}\n", sandbox_str)))?
            .queue(SetBackgroundColor(BG))?;

        stdout
            .queue(Print("\n"))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("   {RULE}\n")))?
            .queue(SetBackgroundColor(BG))?;

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("\n   Write config.toml?\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "[Enter] Write    [Esc] Go back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // Write config
                    match write_config(state, config_path).await {
                        Ok(()) => return Ok(StepOutcome::Next),
                        Err(e) => {
                            let mut stdout = io::stdout();
                            print_error(&mut stdout, &format!("Failed to write config: {e}"))?;
                            stdout.queue(Print("   Press any key to try again...\n"))?;
                            stdout.flush()?;
                            event::read()?;
                        }
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

// ─── Step 8: Done ────────────────────────────────────────────────────────────

async fn step_done(state: &WizardState, config_path: &Path) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let mut selected: u8 = 0;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 8)?;

        print_success(&mut stdout, "Setup complete — agent ready")?;
        stdout
            .queue(Print("\n"))?
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("   Provider:   {}\n", p.name)))?
            .queue(Print(format!("   Model:      {}\n", state.model)))?
            .queue(Print(format!(
                "   Config:     {}  (written)\n",
                config_path.display()
            )))?
            .queue(Print(format!(
                "   Workspace:  {}  ({})\n",
                state.workspace_dir,
                if state.create_workspace { "initialized" } else { "exists" }
            )))?
            .queue(SetBackgroundColor(BG))?;

        stdout
            .queue(Print("\n"))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("   {RULE}\n")))?
            .queue(SetBackgroundColor(BG))?;

        // Key box
        stdout.queue(Print(
            "\n   ┌─────────────────────────────────────────────────┐\n\
             \x20  │  Your admin API key (shown once):               │\n\
             \x20  │                                                 │\n"
        ))?;
        stdout
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("{:<47}", &state.admin_key_raw)))?
            .queue(SetBackgroundColor(BG))?
            .queue(Print("│\n"))?;
        stdout.queue(Print(
            "   │                                                 │\n\
             \x20  └─────────────────────────────────────────────────┘\n\n"
        ))?;

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("   How do you want to start?\n\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(SetBackgroundColor(BG))?;

        let options = ["xenoclaw serve    start the agent runtime", "Exit              configure more later"];
        for (i, opt) in options.iter().enumerate() {
            if i as u8 == selected {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► [{}]  {opt}\n", i + 1)))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(SetBackgroundColor(BG))?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     [{}]  {opt}\n", i + 1)))?
                    .queue(SetBackgroundColor(BG))?;
            }
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[↑↓/jk] Navigate  [Enter] Launch  [1/2] Quick select",
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
                    selected = 0;
                    return launch_or_exit(selected);
                }
                KeyCode::Char('2') => {
                    selected = 1;
                    return launch_or_exit(selected);
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
        // Launch xenoclaw serve via exec-replace on Unix
        terminal::disable_raw_mode()?;
        let mut stdout = io::stdout();
        stdout.queue(LeaveAlternateScreen)?;
        stdout.queue(ResetColor)?;
        stdout.flush()?;
        println!("\nStarting xenoclaw serve...\n");

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new("xenoclaw")
                .arg("serve")
                .exec();
            // exec() only returns on error
            eprintln!("Failed to exec xenoclaw serve: {err}");
            std::process::exit(1);
        }

        #[cfg(not(unix))]
        {
            let status = std::process::Command::new("xenoclaw")
                .arg("serve")
                .status();
            match status {
                Ok(s) => std::process::exit(s.code().unwrap_or(0)),
                Err(e) => {
                    eprintln!("Failed to start xenoclaw serve: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
    // Exit
    Ok(StepOutcome::Next)
}

// ─── Quit Confirmation ───────────────────────────────────────────────────────

async fn confirm_quit() -> Result<bool> {
    let mut stdout = io::stdout();
    clear_screen(&mut stdout)?;

    stdout
        .queue(SetForegroundColor(SUCCESS))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print("\n   Quit setup? Config has not been written.\n\n"))?
        .queue(SetBackgroundColor(BG))?
        .queue(Print("   [Enter] Quit    [Esc] Cancel\n"))?;
    stdout.flush()?;

    loop {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}

// ─── Config Writer ───────────────────────────────────────────────────────────

async fn write_config(state: &WizardState, config_path: &Path) -> Result<()> {
    let p = &PROVIDERS[state.provider_idx];

    // Resolve ~/.xenoclaw paths for data and logs
    let xenoclaw_home = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
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
        // Add custom commands (comma-separated)
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
        let items: Vec<String> = all_cmds.iter().map(|c| format!("\"{}\"", c)).collect();
        items.join(", ")
    };

    let api_key_line = if p.needs_api_key && !state.api_key.is_empty() {
        format!("api_key       = \"{}\"", state.api_key)
    } else {
        "api_key       = \"\"".to_string()
    };

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
base_url      = "{base_url}"
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
        base_url = state.base_url,
        model = state.model,
        host = state.host,
        port = state.port,
        web_port = state.web_port,
        data_dir = data_dir.display(),
        log_dir = log_dir.display(),
        admin_username = state.admin_username,
        admin_password_hash = state.admin_password_hash,
    );

    // Append coding section if sandbox commands are enabled
    let coding_section = if !all_cmds.is_empty() {
        let mut blocklist_items: Vec<String> = vec![
            "\"rm -rf\"".to_string(),
            "\"dd\"".to_string(),
        ];
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
        // Add user-specified exclusions
        if !state.excluded_commands.is_empty() {
            for cmd in state.excluded_commands.split(',') {
                let trimmed = cmd.trim();
                if !trimmed.is_empty() {
                    blocklist_items.push(format!("\"{}\"", trimmed));
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

    // Ensure ~/.xenoclaw/ directory exists
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Create data and log directories
    tokio::fs::create_dir_all(&data_dir).await?;
    tokio::fs::create_dir_all(&log_dir).await?;

    // Write the config file
    tokio::fs::write(config_path, &full_content).await?;

    // Validate by parsing back
    common::config::load_config_from_str(&full_content)
        .map_err(|e| anyhow::anyhow!("Validation failed: {e}"))?;

    // Create workspace directory (always — it's the default location)
    let ws_path = PathBuf::from(&state.workspace_dir);
    tokio::fs::create_dir_all(&ws_path).await?;
    tokio::fs::create_dir_all(ws_path.join("memory")).await?;
    tokio::fs::create_dir_all(ws_path.join("skills")).await?;

    // Create stub files
    let stubs = [
        ("SOUL.md", "# Soul\n\nDefine your agent's core values and ethical constraints here.\n"),
        ("IDENTITY.md", "# Identity\n\nDefine your agent's name, persona, and tone here.\n"),
        ("TOOLS.md", "# Tools\n\nTool catalogue will be populated automatically.\n"),
        ("USER.md", "# User Context\n\nPersistent user context (name, projects, preferences).\n"),
        ("MEMORY.md", "# Long-Term Memory\n\nAppend-only summary memory.\n"),
        ("AGENTS.md", "# Known Agents\n\nOther agents and delegation instructions.\n"),
        ("BOOTSTRAP.md", "# Bootstrap\n\nFirst-run onboarding instructions.\n\nonboarding_complete = false\n"),
    ];

    for (name, content) in stubs {
        let path = ws_path.join(name);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }

    Ok(())
}
