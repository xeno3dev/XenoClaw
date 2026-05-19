//! First-run setup wizard for XenoClaw.
//!
//! An inline terminal wizard (no alternate screen) that guides the user
//! through initial configuration and produces a valid config.toml.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    style::{Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor, ResetColor},
    terminal::{self, Clear, ClearType},
    QueueableCommand,
};

use security_layer::auth::ApiKeyAuthenticator;

// ─── Color Palette ───────────────────────────────────────────────────────────
/// Headers, brand, ► cursor, ✓/✗ symbols, prompts
const BRAND: Color = Color::Green;
/// Body text, menu labels, values, key box contents
const SUCCESS: Color = Color::Green;
/// Dim/secondary text, horizontal rules, footer hints, inactive menu items
const DIM: Color = Color::Rgb { r: 0, g: 80, b: 0 };

const TOTAL_STEPS: u8 = 8;
const RULE: &str = "───────────────────────────────────────────────────────";

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
    ProviderDef { name: "Google Gemini", provider_type: "openai_compatible", default_model: "gemini-2.5-pro", default_base_url: "https://generativelanguage.googleapis.com/v1beta/openai", needs_api_key: true },
    ProviderDef { name: "OpenAI", provider_type: "openai_compatible", default_model: "gpt-4o", default_base_url: "https://api.openai.com/v1", needs_api_key: true },
    ProviderDef { name: "AWS Bedrock", provider_type: "openai_compatible", default_model: "anthropic.claude-sonnet-4-20250514-v1:0", default_base_url: "https://bedrock-runtime.us-east-1.amazonaws.com", needs_api_key: true },
    ProviderDef { name: "OpenRouter", provider_type: "openai_compatible", default_model: "anthropic/claude-sonnet-4-20250514", default_base_url: "https://openrouter.ai/api/v1", needs_api_key: true },
    ProviderDef { name: "Together AI", provider_type: "openai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo", default_base_url: "https://api.together.xyz/v1", needs_api_key: true },
    ProviderDef { name: "Mistral AI", provider_type: "openai_compatible", default_model: "mistral-large-latest", default_base_url: "https://api.mistral.ai/v1", needs_api_key: true },
    ProviderDef { name: "Fireworks AI", provider_type: "openai_compatible", default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct", default_base_url: "https://api.fireworks.ai/inference/v1", needs_api_key: true },
    ProviderDef { name: "DeepSeek", provider_type: "openai_compatible", default_model: "deepseek-chat", default_base_url: "https://api.deepseek.com/v1", needs_api_key: true },
    ProviderDef { name: "Groq", provider_type: "openai_compatible", default_model: "llama-3.3-70b-versatile", default_base_url: "https://api.groq.com/openai/v1", needs_api_key: true },
    ProviderDef { name: "xAI", provider_type: "openai_compatible", default_model: "grok-3", default_base_url: "https://api.x.ai/v1", needs_api_key: true },
    ProviderDef { name: "Perplexity", provider_type: "openai_compatible", default_model: "sonar-pro", default_base_url: "https://api.perplexity.ai", needs_api_key: true },
    ProviderDef { name: "Cohere", provider_type: "openai_compatible", default_model: "command-r-plus", default_base_url: "https://api.cohere.com/v2", needs_api_key: true },
    ProviderDef { name: "AI21 Labs", provider_type: "openai_compatible", default_model: "jamba-1.5-large", default_base_url: "https://api.ai21.com/studio/v1", needs_api_key: true },
    ProviderDef { name: "Hugging Face", provider_type: "openai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct", default_base_url: "https://api-inference.huggingface.co/v1", needs_api_key: true },
    ProviderDef { name: "Replicate", provider_type: "openai_compatible", default_model: "meta/llama-3.3-70b-instruct", default_base_url: "https://api.replicate.com/v1", needs_api_key: true },
    ProviderDef { name: "Requesty", provider_type: "openai_compatible", default_model: "anthropic/claude-sonnet-4-20250514", default_base_url: "https://router.requesty.ai/v1", needs_api_key: true },
    ProviderDef { name: "Cerebras", provider_type: "openai_compatible", default_model: "llama-3.3-70b", default_base_url: "https://api.cerebras.ai/v1", needs_api_key: true },
    ProviderDef { name: "SambaNova", provider_type: "openai_compatible", default_model: "Meta-Llama-3.3-70B-Instruct", default_base_url: "https://api.sambanova.ai/v1", needs_api_key: true },
    ProviderDef { name: "Ollama", provider_type: "ollama", default_model: "llama3.2", default_base_url: "http://localhost:11434", needs_api_key: false },
    ProviderDef { name: "vLLM", provider_type: "openai_compatible", default_model: "meta-llama/Llama-3.3-70B-Instruct", default_base_url: "http://localhost:8000/v1", needs_api_key: false },
    ProviderDef { name: "LM Studio", provider_type: "openai_compatible", default_model: "local-model", default_base_url: "http://localhost:1234/v1", needs_api_key: false },
    ProviderDef { name: "Qwen", provider_type: "openai_compatible", default_model: "qwen-max", default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1", needs_api_key: true },
    ProviderDef { name: "MiniMax", provider_type: "openai_compatible", default_model: "MiniMax-Text-01", default_base_url: "https://api.minimax.chat/v1", needs_api_key: true },
    ProviderDef { name: "Zhipu AI", provider_type: "openai_compatible", default_model: "glm-4-plus", default_base_url: "https://open.bigmodel.cn/api/paas/v4", needs_api_key: true },
    ProviderDef { name: "Moonshot AI", provider_type: "openai_compatible", default_model: "moonshot-v1-128k", default_base_url: "https://api.moonshot.cn/v1", needs_api_key: true },
    ProviderDef { name: "Baidu Qianfan", provider_type: "openai_compatible", default_model: "ernie-4.0-8k", default_base_url: "https://aip.baidubce.com/rpc/2.0/ai_custom/v1/wenxinworkshop", needs_api_key: true },
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
    require_auth: bool,
    admin_key_raw: String,
    admin_key_hash: String,
    sandbox_commands: Vec<(String, bool)>,
    tool_timeout: String,
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
            port: "3000".to_string(),
            require_auth: true,
            admin_key_raw: String::new(),
            admin_key_hash: String::new(),
            sandbox_commands: vec![
                ("git".to_string(), false),
                ("cargo".to_string(), false),
                ("python3".to_string(), false),
                ("npm".to_string(), false),
                ("curl".to_string(), false),
                ("wget".to_string(), false),
            ],
            tool_timeout: "30".to_string(),
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
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("XenoClaw"))?
        .queue(Print("  "))?
        .queue(Print("Init"))?
        .queue(Print(format!("  {} of {}", step, TOTAL_STEPS)))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(Print("\n"))?
        .queue(SetForegroundColor(DIM))?
        .queue(Print(RULE))?
        .queue(ResetColor)?
        .queue(Print("\n\n"))?;
    stdout.flush()
}

fn print_banner(stdout: &mut io::Stdout) -> io::Result<()> {
    stdout.queue(SetForegroundColor(BRAND))?;
    stdout.queue(SetAttribute(Attribute::Bold))?;
    for line in BANNER.lines() {
        stdout.queue(Print(format!("  {line}\n")))?;
    }
    stdout.queue(SetAttribute(Attribute::Reset))?;
    stdout.queue(ResetColor)?;
    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print("                    Agent Runtime\n"))?
        .queue(ResetColor)?;
    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print(format!("  {RULE}\n")))?
        .queue(ResetColor)?;
    stdout.flush()
}

fn print_footer(stdout: &mut io::Stdout, hint: &str) -> io::Result<()> {
    let (_, rows) = terminal::size()?;
    stdout
        .queue(cursor::MoveTo(0, rows - 1))?
        .queue(SetForegroundColor(DIM))?
        .queue(Print(hint))?
        .queue(ResetColor)?;
    stdout.flush()
}

fn clear_screen(stdout: &mut io::Stdout) -> io::Result<()> {
    stdout
        .queue(SetBackgroundColor(Color::Black))?
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?;
    stdout.flush()
}

fn print_success(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    stdout
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("✓ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(ResetColor)?
        .queue(Print("\n"))?;
    stdout.flush()
}

fn print_error(stdout: &mut io::Stdout, msg: &str) -> io::Result<()> {
    stdout
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print(format!("✗ {msg}")))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(ResetColor)?
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

    terminal::enable_raw_mode()?;
    let result = run_wizard_inner(config_path).await;
    terminal::disable_raw_mode()?;

    // Print a newline so the shell prompt starts clean
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
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Dual-mode: General 24/7 agent + Coding agent (hot-swap)\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🔀"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Multi-provider LLM failover (27 providers supported)\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  💬"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Telegram, Discord & WhatsApp bridge — chat from anywhere\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🧩"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" WASM plugin system with hot-reload\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  🛡"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Sandboxed execution, RBAC, filesystem & network allowlists\n"))?
        .queue(SetForegroundColor(BRAND))?
        .queue(SetAttribute(Attribute::Bold))?
        .queue(Print("  📡"))?
        .queue(SetAttribute(Attribute::Reset))?
        .queue(SetForegroundColor(SUCCESS))?
        .queue(Print(" Always-on: auto-restart, health checks, SIGHUP hot-reload\n"))?
        .queue(ResetColor)?;

    stdout.queue(Print("\n"))?;
    stdout
        .queue(SetForegroundColor(DIM))?
        .queue(Print("  Self-hosted. Your infrastructure. Your data. Your rules.\n"))?
        .queue(ResetColor)?;

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
            .queue(ResetColor)?;

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
                    .queue(ResetColor)?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!(
                        "     [{num:>2}]  {:<20} {}\n",
                        p.name, p.default_model
                    )))?
                    .queue(ResetColor)?;
            }
        }

        if end < PROVIDERS.len() {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!(
                    "\n   ... and {} more (scroll down)\n",
                    PROVIDERS.len() - end
                )))?
                .queue(ResetColor)?;
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
    let mut api_key = state.api_key.clone();
    let mut base_url = state.base_url.clone();
    let mut model = state.model.clone();
    let mut field: u8 = 0; // 0=api_key, 1=base_url, 2=model

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 2)?;

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print(format!("   Provider: {}\n\n", p.name)))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(ResetColor)?;

        let fields: Vec<(&str, &str, bool)> = vec![
            ("API Key", &api_key, true),
            ("Base URL", &base_url, false),
            ("Model", &model, false),
        ];

        for (i, (label, value, masked)) in fields.iter().enumerate() {
            let display = if *masked && !value.is_empty() {
                mask_key(value)
            } else {
                value.to_string()
            };
            if i as u8 == field {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► {label:<10} > {display}█\n")))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(ResetColor)?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     {label:<10} > {display}\n")))?
                    .queue(ResetColor)?;
            }
        }

        stdout.queue(Print("\n"))?;
        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[Tab] Next field  [Enter] Confirm  [Esc] Back  [Ctrl-C] Quit",
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
                    state.api_key = api_key;
                    state.base_url = base_url;
                    state.model = model;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Backspace => {
                    let target = match field {
                        0 => &mut api_key,
                        1 => &mut base_url,
                        _ => &mut model,
                    };
                    target.pop();
                }
                KeyCode::Char(c) => {
                    let target = match field {
                        0 => &mut api_key,
                        1 => &mut base_url,
                        _ => &mut model,
                    };
                    target.push(c);
                }
                _ => {}
            }
        }
    }
}

// ─── Step 3: Workspace ───────────────────────────────────────────────────────

async fn step_workspace(state: &mut WizardState) -> Result<StepOutcome> {
    let mut input = state.workspace_dir.clone();

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
            .queue(Print(format!("   > {input}█\n\n")))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(ResetColor)?;

        let path = Path::new(&input);
        if path.exists() && path.is_dir() {
            let count = std::fs::read_dir(path)
                .map(|d| d.count())
                .unwrap_or(0);
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ✓  Found: {input}  ({count} files)\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(ResetColor)?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print("   Directory does not exist.\n"))?
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print("   [C] Create stub workspace    [S] Skip for now\n"))?
                .queue(ResetColor)?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[Enter] Confirm  [C] Create  [S] Skip  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    state.workspace_dir = input.clone();
                    state.create_workspace = !Path::new(&input).exists();
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Char('c') | KeyCode::Char('C') if !Path::new(&input).exists() => {
                    state.workspace_dir = input.clone();
                    state.create_workspace = true;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    state.workspace_dir = input.clone();
                    state.create_workspace = false;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Backspace => { input.pop(); }
                KeyCode::Char(c) => { input.push(c); }
                _ => {}
            }
        }
    }
}

// ─── Step 4: Server ──────────────────────────────────────────────────────────

async fn step_server(state: &mut WizardState) -> Result<StepOutcome> {
    let mut host = state.host.clone();
    let mut port = state.port.clone();
    let mut require_auth = state.require_auth;
    let mut field: u8 = 0; // 0=host, 1=port, 2=auth

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 4)?;

        let host_cursor = if field == 0 { "█" } else { "" };
        let port_cursor = if field == 1 { "█" } else { "" };
        let auth_str = if require_auth { "Yes" } else { "No" };

        // Host field
        if field == 0 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   Bind host:  > {host}{host_cursor}\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(ResetColor)?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("   Bind host:  > {host}\n")))?
                .queue(ResetColor)?;
        }
        // Port field
        if field == 1 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   Port:       > {port}{port_cursor}\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(ResetColor)?;
        } else {
            stdout
                .queue(SetForegroundColor(SUCCESS))?
                .queue(Print(format!("   Port:       > {port}\n")))?
                .queue(ResetColor)?;
        }
        stdout.queue(Print("\n"))?;
        // Auth toggle
        if field == 2 {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!("   ► Require authentication?  [{auth_str}]\n")))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(ResetColor)?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!("     Require authentication?  [{auth_str}]\n")))?
                .queue(ResetColor)?;
        }
        stdout.flush()?;

        print_footer(
            &mut stdout,
            "[Tab] Next field  [Enter] Confirm  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => { field = (field + 1) % 3; }
                KeyCode::BackTab => { field = if field == 0 { 2 } else { field - 1 }; }
                KeyCode::Enter => {
                    state.host = host;
                    state.port = port;
                    state.require_auth = require_auth;
                    return Ok(StepOutcome::Next);
                }
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') if field == 2 => {
                    require_auth = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') if field == 2 => {
                    require_auth = false;
                }
                KeyCode::Char(' ') if field == 2 => {
                    require_auth = !require_auth;
                }
                KeyCode::Backspace => {
                    match field {
                        0 => { host.pop(); }
                        1 => { port.pop(); }
                        _ => {}
                    }
                }
                KeyCode::Char(c) => {
                    match field {
                        0 => host.push(c),
                        1 => {
                            if c.is_ascii_digit() { port.push(c); }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

// ─── Step 5: Admin API Key ───────────────────────────────────────────────────

async fn step_api_key(state: &mut WizardState) -> Result<StepOutcome> {
    // Generate key if not already generated
    if state.admin_key_raw.is_empty() {
        let auth = ApiKeyAuthenticator::new();
        state.admin_key_raw = format!("xc_{}", auth.generate_key(40));
        state.admin_key_hash = auth.hash_key(&state.admin_key_raw);
    }

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 5)?;

        stdout.queue(Print(
            "   ┌─────────────────────────────────────────────────┐\n\
             \x20  │  ⚠  Admin API key — save this now.              │\n\
             \x20  │     It will NOT be shown again.                 │\n\
             \x20  │                                                 │\n"
        ))?;
        stdout
            .queue(Print("   │  "))?
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("{:<47}", &state.admin_key_raw)))?
            .queue(ResetColor)?
            .queue(Print("│\n"))?;
        stdout.queue(Print(
            "   │                                                 │\n\
             \x20  └─────────────────────────────────────────────────┘\n\n"
        ))?;

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[R] Regenerate    [Enter] Accept  [Esc] Back  [Ctrl-C] Quit",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => return Ok(StepOutcome::Next),
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    let auth = ApiKeyAuthenticator::new();
                    state.admin_key_raw = format!("xc_{}", auth.generate_key(40));
                    state.admin_key_hash = auth.hash_key(&state.admin_key_raw);
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

// ─── Step 6: Sandbox ─────────────────────────────────────────────────────────

async fn step_sandbox(state: &mut WizardState) -> Result<StepOutcome> {
    let mut selected: usize = 0;
    let mut in_timeout = false;

    loop {
        let mut stdout = io::stdout();
        clear_screen(&mut stdout)?;
        print_header(&mut stdout, 6)?;

        stdout
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print("   Allowed shell command prefixes:\n"))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print("   (Space to toggle)\n\n"))?
            .queue(ResetColor)?;

        for (i, (cmd, enabled)) in state.sandbox_commands.iter().enumerate() {
            let check = if *enabled { "✓" } else { " " };
            if !in_timeout && i == selected {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► [{check}] {cmd}\n")))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(ResetColor)?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     [{check}] {cmd}\n")))?
                    .queue(ResetColor)?;
            }
        }

        stdout.queue(Print("\n"))?;

        let timeout_cursor = if in_timeout { "█" } else { "" };
        if in_timeout {
            stdout
                .queue(SetForegroundColor(BRAND))?
                .queue(SetAttribute(Attribute::Bold))?
                .queue(Print(format!(
                    "   ► Tool timeout:  > {}{timeout_cursor}  seconds\n",
                    state.tool_timeout
                )))?
                .queue(SetAttribute(Attribute::Reset))?
                .queue(ResetColor)?;
        } else {
            stdout
                .queue(SetForegroundColor(DIM))?
                .queue(Print(format!(
                    "     Tool timeout:  > {}  seconds\n",
                    state.tool_timeout
                )))?
                .queue(ResetColor)?;
        }

        stdout.flush()?;
        print_footer(
            &mut stdout,
            "[↑↓] Navigate  [Space] Toggle  [Tab] To timeout  [Enter] Next  [Esc] Back",
        )?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Tab => { in_timeout = !in_timeout; }
                KeyCode::Up | KeyCode::Char('k') if !in_timeout => {
                    if selected > 0 { selected -= 1; }
                }
                KeyCode::Down | KeyCode::Char('j') if !in_timeout => {
                    if selected < state.sandbox_commands.len() - 1 { selected += 1; }
                }
                KeyCode::Char(' ') if !in_timeout => {
                    state.sandbox_commands[selected].1 = !state.sandbox_commands[selected].1;
                }
                KeyCode::Backspace if in_timeout => {
                    state.tool_timeout.pop();
                }
                KeyCode::Char(c) if in_timeout && c.is_ascii_digit() => {
                    state.tool_timeout.push(c);
                }
                KeyCode::Enter => return Ok(StepOutcome::Next),
                KeyCode::Esc => return Ok(StepOutcome::Back),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(StepOutcome::Quit);
                }
                _ => {}
            }
        }
    }
}

// ─── Step 7: Review ──────────────────────────────────────────────────────────

async fn step_review(state: &WizardState, config_path: &Path) -> Result<StepOutcome> {
    let p = &PROVIDERS[state.provider_idx];
    let enabled_cmds: Vec<&str> = state
        .sandbox_commands
        .iter()
        .filter(|(_, e)| *e)
        .map(|(c, _)| c.as_str())
        .collect();
    let sandbox_str = if enabled_cmds.is_empty() {
        "none".to_string()
    } else {
        enabled_cmds.join(", ")
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
            .queue(SetForegroundColor(SUCCESS))?
            .queue(Print(format!("   Provider:   {}\n", p.name)))?
            .queue(Print(format!("   Model:      {}\n", state.model)))?
            .queue(Print(format!(
                "   API key:    {}  (stored as SHA-256 hash)\n",
                mask_key(&state.admin_key_raw)
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
            .queue(ResetColor)?;

        stdout
            .queue(Print("\n"))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("   {RULE}\n")))?
            .queue(ResetColor)?;

        stdout
            .queue(SetForegroundColor(BRAND))?
            .queue(SetAttribute(Attribute::Bold))?
            .queue(Print("\n   Write config.toml?\n"))?
            .queue(SetAttribute(Attribute::Reset))?
            .queue(ResetColor)?;
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
            .queue(ResetColor)?;

        stdout
            .queue(Print("\n"))?
            .queue(SetForegroundColor(DIM))?
            .queue(Print(format!("   {RULE}\n")))?
            .queue(ResetColor)?;

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
            .queue(ResetColor)?
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
            .queue(ResetColor)?;

        let options = ["xenoclaw serve    start the agent runtime", "Exit              configure more later"];
        for (i, opt) in options.iter().enumerate() {
            if i as u8 == selected {
                stdout
                    .queue(SetForegroundColor(BRAND))?
                    .queue(SetAttribute(Attribute::Bold))?
                    .queue(Print(format!("   ► [{}]  {opt}\n", i + 1)))?
                    .queue(SetAttribute(Attribute::Reset))?
                    .queue(ResetColor)?;
            } else {
                stdout
                    .queue(SetForegroundColor(DIM))?
                    .queue(Print(format!("     [{}]  {opt}\n", i + 1)))?
                    .queue(ResetColor)?;
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

    stdout.queue(Print("\n   Quit setup? Config has not been written.\n\n"))?;
    stdout.queue(Print("   [Enter] Quit    [Esc] Cancel\n"))?;
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

    let enabled_cmds: Vec<&str> = state
        .sandbox_commands
        .iter()
        .filter(|(_, e)| *e)
        .map(|(c, _)| c.as_str())
        .collect();

    let command_allowlist = if enabled_cmds.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = enabled_cmds.iter().map(|c| format!("\"{}\"", c)).collect();
        items.join(", ")
    };

    let api_key_line = if p.needs_api_key && !state.api_key.is_empty() {
        format!("api_key       = \"{}\"", state.api_key)
    } else {
        "api_key       = \"\"".to_string()
    };

    let timeout: u32 = state.tool_timeout.parse().unwrap_or(30);

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

[security.resource_limits]
max_memory_mb    = 512
max_cpu_percent  = 80
max_processes    = 10

[scheduler]

[web]

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
        data_dir = data_dir.display(),
        log_dir = log_dir.display(),
    );

    // Append coding section if sandbox commands are enabled
    let coding_section = if !enabled_cmds.is_empty() {
        format!(
            r#"
[coding]
workspace_dirs       = ["{workspace}"]
repository_dirs      = ["{workspace}"]
command_allowlist    = [{allowlist}]
command_blocklist    = ["rm -rf", "dd"]
max_file_size_mb     = 10
max_concurrent_shells = 5
shell_timeout_seconds = {timeout}
undo_history_size    = 50
"#,
            workspace = state.workspace_dir,
            allowlist = command_allowlist,
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
