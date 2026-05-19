//! Workspace system prompt assembly.
//!
//! Reads workspace files (SOUL.md, IDENTITY.md, etc.) and assembles the
//! hardcoded system prompt injected as ChatRole::System in every CompletionRequest.

use std::path::Path;

use anyhow::Result;
use chrono::Utc;
use tracing::trace;

/// Build the system prompt from workspace files.
///
/// Reads SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, USER.md, MEMORY.md,
/// BOOTSTRAP.md, memory/*.md, and skills/*/skill.md from the workspace directory.
/// Missing files are silently skipped (logged at TRACE level).
pub async fn build_system_prompt(workspace_dir: &Path) -> Result<String> {
    let mut sections = Vec::new();

    // Core files
    let soul = read_optional(workspace_dir, "SOUL.md").await;
    let identity = read_optional(workspace_dir, "IDENTITY.md").await;
    let agents = read_optional(workspace_dir, "AGENTS.md").await;
    let tools = read_optional(workspace_dir, "TOOLS.md").await;
    let user = read_optional(workspace_dir, "USER.md").await;
    let memory_long = read_optional(workspace_dir, "MEMORY.md").await;
    let bootstrap = read_optional(workspace_dir, "BOOTSTRAP.md").await;

    // SOUL.md
    if let Some(content) = &soul {
        sections.push(content.clone());
    }

    // IDENTITY.md
    if let Some(content) = &identity {
        sections.push(content.clone());
    }

    sections.push("---".to_string());

    // TOOLS.md
    sections.push("## Your Capabilities".to_string());
    if let Some(content) = &tools {
        sections.push(content.clone());
    }

    // Skills
    let skills_section = build_skills_section(workspace_dir).await;
    if !skills_section.is_empty() {
        sections.push("### Installed Skills (ClawhubHub Plugins)".to_string());
        sections.push(skills_section);
    }

    sections.push("---".to_string());

    // AGENTS.md
    sections.push("## Known Agents".to_string());
    if let Some(content) = &agents {
        sections.push(content.clone());
    }

    sections.push("---".to_string());

    // USER.md
    sections.push("## User Context".to_string());
    if let Some(content) = &user {
        sections.push(content.clone());
    }

    sections.push("---".to_string());

    // Memory
    sections.push("## Memory".to_string());
    sections.push("### Long-Term Summary".to_string());
    if let Some(content) = &memory_long {
        sections.push(content.clone());
    }

    // Session & Topic Memory
    let memory_section = build_memory_section(workspace_dir).await;
    if !memory_section.is_empty() {
        sections.push("### Session & Topic Memory".to_string());
        sections.push(memory_section);
    }

    sections.push("---".to_string());

    // Runtime Status (HEARTBEAT — always generated fresh)
    sections.push("## Runtime Status".to_string());
    sections.push(generate_heartbeat());

    sections.push("---".to_string());

    // Operating Constraints
    sections.push(OPERATING_CONSTRAINTS.to_string());

    // BOOTSTRAP.md (omit if onboarding_complete = true)
    if let Some(content) = &bootstrap {
        if !content.contains("onboarding_complete = true") {
            sections.push(content.clone());
        }
    }

    Ok(sections.join("\n\n"))
}

/// Read a file from the workspace, returning None if it doesn't exist.
async fn read_optional(workspace_dir: &Path, filename: &str) -> Option<String> {
    let path = workspace_dir.join(filename);
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => Some(content),
        Err(_) => {
            trace!(file = %path.display(), "Workspace file not found, skipping");
            None
        }
    }
}

/// Build the skills section from skills/*/skill.md files.
async fn build_skills_section(workspace_dir: &Path) -> String {
    let skills_dir = workspace_dir.join("skills");
    let mut skills = Vec::new();

    let entries = match tokio::fs::read_dir(&skills_dir).await {
        Ok(entries) => entries,
        Err(_) => {
            trace!(dir = %skills_dir.display(), "Skills directory not found");
            return String::new();
        }
    };

    let mut entries = entries;
    let mut dirs = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            dirs.push(entry.file_name().to_string_lossy().to_string());
        }
    }

    // Sort for deterministic ordering
    dirs.sort();

    for dirname in dirs {
        let skill_path = skills_dir.join(&dirname).join("skill.md");
        if let Ok(content) = tokio::fs::read_to_string(&skill_path).await {
            skills.push(format!("### {dirname}\n\n{content}"));
        }
    }

    skills.join("\n\n")
}

/// Build the memory section from memory/*.md files.
async fn build_memory_section(workspace_dir: &Path) -> String {
    let memory_dir = workspace_dir.join("memory");
    let mut memories = Vec::new();

    let entries = match tokio::fs::read_dir(&memory_dir).await {
        Ok(entries) => entries,
        Err(_) => {
            trace!(dir = %memory_dir.display(), "Memory directory not found");
            return String::new();
        }
    };

    let mut entries = entries;
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".md") {
            files.push(name);
        }
    }

    // Sort for deterministic ordering
    files.sort();

    for filename in files {
        let path = memory_dir.join(&filename);
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            let heading = filename.trim_end_matches(".md");
            memories.push(format!("### {heading}\n\n{content}"));
        }
    }

    memories.join("\n\n")
}

/// Generate a fresh HEARTBEAT section (never read from disk).
fn generate_heartbeat() -> String {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    // These values would be populated from actual runtime state in a full implementation.
    // For now, provide baseline values at startup.
    format!(
        "- Uptime: just started\n\
         - Active scheduler jobs: 0\n\
         - Last error: none\n\
         - Timestamp: {now}"
    )
}

const OPERATING_CONSTRAINTS: &str = r#"## Operating Constraints

You are XenoClaw, a self-hosted AI agent runtime operating entirely within the user's own infrastructure. You must:

- Use tools only as described in the catalogue above.
- Emit a valid JSON `tool_use` block before any tool result.
- Never fabricate tool results.
- Append to MEMORY.md when the user shares information worth retaining long-term.
- Update HEARTBEAT.md fields only through the heartbeat tool, never by narrating them.

You must never:
- Exfiltrate data to any endpoint not in the configured network allowlist.
- Execute shell commands outside the sandbox defined in config.toml.
- Modify workspace files other than MEMORY.md, files under memory/, and HEARTBEAT.md.
- Claim capabilities not listed in TOOLS.md or the installed skills above.
- Abandon your identity or pretend to be a different AI system.

When asked to do something outside your tools, say so clearly and explain what tool or skill addition would enable it."#;
