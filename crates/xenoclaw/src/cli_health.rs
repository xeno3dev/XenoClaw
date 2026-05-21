//! CLI provider health checks, auto-install, login assistance, and MCP config injection.
//!
//! Supports four CLI-backed LLM providers:
//!   - Claude Code  (`claude`)      — installed via npm
//!   - GitHub Copilot CLI (`gh copilot`) — installed as a gh extension
//!   - Gemini CLI   (`gemini`)      — installed via npm
//!   - OpenAI Codex CLI (`codex`)  — installed via npm
//!
//! Each check is intentionally non-invasive: we never modify state silently.
//! All mutations (install, login, MCP inject) are explicit calls from the wizard.

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

// ─── CLI type ────────────────────────────────────────────────────────────────

/// Which CLI-backed provider to operate on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliType {
    ClaudeCode,
    CopilotCli,
    GeminiCli,
    CodexCli,
}

// ─── Health snapshot ─────────────────────────────────────────────────────────

/// Point-in-time health status of a CLI provider.
#[derive(Debug, Clone)]
pub struct CliHealth {
    /// Binary found in PATH and executable.
    pub installed: bool,
    /// CLI reports an authenticated session.
    pub logged_in: bool,
    /// Auto-install is possible (prerequisite tool is available).
    pub can_auto_install: bool,
    /// Name of the prerequisite needed for auto-install (e.g. `"npm"`).
    pub install_prereq: &'static str,
    /// Full manual install command shown when auto-install is unavailable.
    pub install_instructions: &'static str,
    /// Version string returned by the CLI, if installed.
    pub version: Option<String>,
}

impl CliHealth {
    /// Run all health checks for the given CLI type. Never panics.
    pub async fn check(cli: CliType) -> Self {
        match cli {
            CliType::ClaudeCode => check_claude_code().await,
            CliType::CopilotCli => check_copilot_cli().await,
            CliType::GeminiCli => check_gemini_cli().await,
            CliType::CodexCli => check_codex_cli().await,
        }
    }

    /// `true` when installed **and** authenticated — ready for use.
    pub fn is_ready(&self) -> bool {
        self.installed && self.logged_in
    }

    /// Returns `(binary, args)` to run the interactive login command.
    pub fn login_command(cli: CliType) -> (&'static str, &'static [&'static str]) {
        match cli {
            CliType::ClaudeCode => ("claude", &["auth", "login"]),
            CliType::CopilotCli => ("gh", &["auth", "login"]),
            CliType::GeminiCli => ("gemini", &["auth"]),
            CliType::CodexCli => ("codex", &["auth"]),
        }
    }

    /// Returns `(binary, args)` to run the install command.
    pub fn install_command(cli: CliType) -> (&'static str, &'static [&'static str]) {
        match cli {
            // Native installer replaced npm as of Feb 2026; piped through sh so
            // a shell is required (sh is universally available on Linux/macOS).
            CliType::ClaudeCode => {
                ("sh", &["-c", "curl -fsSL https://claude.ai/install.sh | sh"])
            }
            CliType::CopilotCli => ("gh", &["extension", "install", "github/gh-copilot"]),
            CliType::GeminiCli => ("npm", &["install", "-g", "@google/gemini-cli"]),
            CliType::CodexCli => ("npm", &["install", "-g", "@openai/codex"]),
        }
    }

    /// Human-readable CLI display name.
    pub fn display_name(cli: CliType) -> &'static str {
        match cli {
            CliType::ClaudeCode => "Claude Code CLI",
            CliType::CopilotCli => "GitHub Copilot CLI",
            CliType::GeminiCli => "Gemini CLI",
            CliType::CodexCli => "OpenAI Codex CLI",
        }
    }

    /// Whether injecting an MCP config entry makes sense for this CLI.
    pub fn supports_mcp(cli: CliType) -> bool {
        matches!(cli, CliType::ClaudeCode | CliType::GeminiCli)
    }

    /// Inject XenoClaw's MCP entry for the given CLI. Only valid when `supports_mcp` is true.
    pub fn inject_mcp(cli: CliType) -> Result<()> {
        match cli {
            CliType::ClaudeCode => inject_mcp_config(),
            CliType::GeminiCli => inject_gemini_mcp_config(),
            _ => Ok(()),
        }
    }

    /// Remove XenoClaw's MCP entry for the given CLI.
    pub fn remove_mcp(cli: CliType) -> Result<()> {
        match cli {
            CliType::ClaudeCode => remove_mcp_config(),
            CliType::GeminiCli => remove_gemini_mcp_config(),
            _ => Ok(()),
        }
    }
}

// ─── Per-CLI checks ──────────────────────────────────────────────────────────

async fn check_claude_code() -> CliHealth {
    // Install: claude --version
    let ver = run_silent("claude", &["--version"]).await;
    let installed = ver.is_some();
    let version = ver.map(|v| v.trim().to_string());

    // Login: `claude auth status` exits 0 when authenticated.
    // Fall back to credential file detection for older CLI versions.
    let logged_in = if installed {
        if run_silent("claude", &["auth", "status"]).await.is_some() {
            true
        } else {
            has_claude_credentials()
        }
    } else {
        false
    };

    // Native installer (curl) replaced npm as of Feb 2026.
    // curl is available on virtually every system; npm is no longer required.
    let can_auto_install = run_silent("curl", &["--version"]).await.is_some();

    CliHealth {
        installed,
        logged_in,
        can_auto_install,
        install_prereq: "curl",
        install_instructions: "curl -fsSL https://claude.ai/install.sh | sh",
        version,
    }
}

async fn check_copilot_cli() -> CliHealth {
    // gh must be present first.
    let gh_ok = run_silent("gh", &["--version"]).await.is_some();

    // The copilot extension shows up in `gh extension list`.
    let installed = if gh_ok {
        run_silent("gh", &["extension", "list"])
            .await
            .map(|out| out.contains("copilot"))
            .unwrap_or(false)
    } else {
        false
    };

    let version = if installed {
        run_silent("gh", &["copilot", "--version"])
            .await
            .map(|v| v.trim().to_string())
    } else {
        None
    };

    // Login: `gh auth status` exits 0 when authenticated.
    let logged_in = if gh_ok {
        run_silent("gh", &["auth", "status"]).await.is_some()
    } else {
        false
    };

    // Can auto-install if gh itself is present (extension install uses gh).
    let can_auto_install = gh_ok;

    CliHealth {
        installed,
        logged_in,
        can_auto_install,
        install_prereq: "gh",
        install_instructions: "gh extension install github/gh-copilot",
        version,
    }
}

async fn check_gemini_cli() -> CliHealth {
    // Install: gemini --version
    let ver = run_silent("gemini", &["--version"]).await;
    let installed = ver.is_some();
    let version = ver.map(|v| v.trim().to_string());

    // Login: look for OAuth credential file; `gemini auth` writes here on success.
    let logged_in = if installed {
        has_gemini_credentials()
    } else {
        false
    };

    // Auto-install requires npm.
    let can_auto_install = run_silent("npm", &["--version"]).await.is_some();

    CliHealth {
        installed,
        logged_in,
        can_auto_install,
        install_prereq: "npm",
        install_instructions: "npm install -g @google/gemini-cli",
        version,
    }
}

async fn check_codex_cli() -> CliHealth {
    // Install: codex --version
    let ver = run_silent("codex", &["--version"]).await;
    let installed = ver.is_some();
    let version = ver.map(|v| v.trim().to_string());

    // Login: Codex authenticates via OPENAI_API_KEY or a stored API key file.
    let logged_in = if installed {
        std::env::var("OPENAI_API_KEY").map(|k| !k.is_empty()).unwrap_or(false)
            || has_codex_credentials()
    } else {
        false
    };

    // Auto-install requires npm.
    let can_auto_install = run_silent("npm", &["--version"]).await.is_some();

    CliHealth {
        installed,
        logged_in,
        can_auto_install,
        install_prereq: "npm",
        install_instructions: "npm install -g @openai/codex",
        version,
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Run a command, capturing stdout. Returns `Some(stdout)` on exit 0, `None` otherwise.
async fn run_silent(cmd: &str, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        None
    }
}

/// Fallback login detection: look for Claude credential files on disk.
fn has_claude_credentials() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = PathBuf::from(home);
    home.join(".config/claude/.credentials.json").exists()
        || home.join(".claude/.credentials.json").exists()
        || home.join(".claude/credentials.json").exists()
        // Claude Code ≥1.x stores auth here
        || home.join(".config/claude/auth.json").exists()
}

/// Detect Gemini CLI credentials written by `gemini auth`.
fn has_gemini_credentials() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = PathBuf::from(home);
    home.join(".gemini/oauth_creds.json").exists()
        || home.join(".config/gemini/credentials.json").exists()
        || home.join(".gemini/credentials.json").exists()
}

/// Detect OpenAI Codex CLI credentials (API key file written by `codex auth`).
fn has_codex_credentials() -> bool {
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let home = PathBuf::from(home);
    home.join(".openai/credentials").exists()
        || home.join(".config/openai/credentials").exists()
        || home.join(".codex/config.json").exists()
}

// ─── MCP config injection ────────────────────────────────────────────────────

/// Inject XenoClaw into Claude Code's MCP server list.
///
/// Reads (or creates) `~/.claude/settings.json` and upserts:
/// ```json
/// { "mcpServers": { "xenoclaw": { "command": "xenoclaw", "args": ["mcp"] } } }
/// ```
/// Idempotent — safe to call more than once.
pub fn inject_mcp_config() -> Result<()> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    let claude_dir = PathBuf::from(&home).join(".claude");
    let settings_path = claude_dir.join("settings.json");

    // Read existing or start from empty object.
    let mut settings: Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .context("failed to read ~/.claude/settings.json")?;
        serde_json::from_str(&raw).unwrap_or(json!({}))
    } else {
        json!({})
    };

    // Ensure mcpServers is an object.
    if !settings
        .get("mcpServers")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        settings["mcpServers"] = json!({});
    }

    // Upsert the xenoclaw entry.
    settings["mcpServers"]["xenoclaw"] = json!({
        "command": "xenoclaw",
        "args": ["mcp"],
        "env": {}
    });

    std::fs::create_dir_all(&claude_dir).context("failed to create ~/.claude")?;
    let out = serde_json::to_string_pretty(&settings).context("JSON serialization failed")?;
    std::fs::write(&settings_path, out).context("failed to write ~/.claude/settings.json")?;

    debug!(path = %settings_path.display(), "MCP entry injected into Claude Code settings");
    Ok(())
}

/// Remove the XenoClaw MCP entry from Claude Code settings (used on uninstall).
pub fn remove_mcp_config() -> Result<()> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    let settings_path = PathBuf::from(&home).join(".claude").join("settings.json");

    if !settings_path.exists() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&settings_path)
        .context("failed to read ~/.claude/settings.json")?;
    let mut settings: Value = serde_json::from_str(&raw).unwrap_or(json!({}));

    if let Some(mcp) = settings.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        mcp.remove("xenoclaw");
    }

    let out = serde_json::to_string_pretty(&settings).context("JSON serialization failed")?;
    std::fs::write(&settings_path, out).context("failed to write ~/.claude/settings.json")?;
    Ok(())
}

/// Inject XenoClaw into Gemini CLI's MCP server list.
///
/// Reads (or creates) `~/.gemini/settings.json` and upserts:
/// ```json
/// { "mcpServers": { "xenoclaw": { "command": "xenoclaw", "args": ["mcp"] } } }
/// ```
/// Idempotent — safe to call more than once.
pub fn inject_gemini_mcp_config() -> Result<()> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    let gemini_dir = PathBuf::from(&home).join(".gemini");
    let settings_path = gemini_dir.join("settings.json");

    let mut settings: Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .context("failed to read ~/.gemini/settings.json")?;
        serde_json::from_str(&raw).unwrap_or(json!({}))
    } else {
        json!({})
    };

    if !settings
        .get("mcpServers")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        settings["mcpServers"] = json!({});
    }

    settings["mcpServers"]["xenoclaw"] = json!({
        "command": "xenoclaw",
        "args": ["mcp"],
        "env": {}
    });

    std::fs::create_dir_all(&gemini_dir).context("failed to create ~/.gemini")?;
    let out = serde_json::to_string_pretty(&settings).context("JSON serialization failed")?;
    std::fs::write(&settings_path, out).context("failed to write ~/.gemini/settings.json")?;

    debug!(path = %settings_path.display(), "MCP entry injected into Gemini CLI settings");
    Ok(())
}

/// Remove the XenoClaw MCP entry from Gemini CLI settings (used on uninstall).
pub fn remove_gemini_mcp_config() -> Result<()> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    let settings_path = PathBuf::from(&home).join(".gemini").join("settings.json");

    if !settings_path.exists() {
        return Ok(());
    }

    let raw = std::fs::read_to_string(&settings_path)
        .context("failed to read ~/.gemini/settings.json")?;
    let mut settings: Value = serde_json::from_str(&raw).unwrap_or(json!({}));

    if let Some(mcp) = settings.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
        mcp.remove("xenoclaw");
    }

    let out = serde_json::to_string_pretty(&settings).context("JSON serialization failed")?;
    std::fs::write(&settings_path, out).context("failed to write ~/.gemini/settings.json")?;
    Ok(())
}
