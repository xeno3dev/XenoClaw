//! Autonomous skill curator — scheduled background agent that prunes and
//! consolidates the skill library.
//!
//! Runs on a cron schedule (default: `0 0 * * 0`, weekly Sunday midnight).
//! On each pass it calls the LLM with the full skill index and processes
//! `KEEP / PRUNE / CONSOLIDATE` directives returned in the response.
//! A markdown report is written to `<logs_dir>/curator/YYYY-MM-DD.md`.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use cron::Schedule;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use llm_router::{
    types::{ChatMessage, ChatRole, CompletionRequest},
    LlmRouter,
};

use crate::skill::SkillDoc;
use crate::store::SkillStore;

/// A parsed action from the curator LLM response.
enum CuratorAction {
    Keep(String),
    Prune {
        slug: String,
        reason: String,
    },
    Consolidate {
        from_slugs: Vec<String>,
        to_slug: String,
        merged_content: String,
    },
}

/// Background skill curator.
pub struct SkillCurator {
    store: Arc<SkillStore>,
    llm_router: Arc<RwLock<LlmRouter>>,
    schedule_expr: String,
    logs_dir: PathBuf,
}

impl SkillCurator {
    pub fn new(
        store: Arc<SkillStore>,
        llm_router: Arc<RwLock<LlmRouter>>,
        schedule_expr: String,
        logs_dir: PathBuf,
    ) -> Self {
        Self {
            store,
            llm_router,
            schedule_expr,
            logs_dir,
        }
    }

    /// Run one curator pass and return the report text.
    pub async fn run_once(&self) -> anyhow::Result<String> {
        let skills = self.store.list().await?;
        if skills.is_empty() {
            let report = format!(
                "# Curator Run — {}\n\nNo skills to review.",
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            );
            self.write_report(&report).await;
            return Ok(report);
        }

        let skill_summary = build_skill_summary(&skills);
        let prompt = format!(
            "Review this skill library. For each skill, output exactly one line:\n\
             KEEP <slug>\n\
             PRUNE <slug> <reason>\n\
             CONSOLIDATE <slug-a> <slug-b> -> <new-slug>\n\n\
             If you output CONSOLIDATE, immediately follow it with the full merged SKILL.md \
             (starting with ---) before the next directive.\n\n\
             Skills:\n{skill_summary}\n\n\
             Output only directives (and any merged SKILL.md content). No explanation."
        );

        let request = CompletionRequest {
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: prompt,
            }],
            tools: Vec::new(),
            max_tokens: Some(4096),
            temperature: Some(0.2),
            stream: false,
        };

        let response = {
            let router = self.llm_router.read().await;
            match router.complete(&request).await {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "Curator LLM call failed");
                    return Err(anyhow::anyhow!("LLM call failed: {e}"));
                }
            }
        };

        let actions = parse_curator_response(&response.content);
        let report = self.process_actions(actions, skills.len()).await;
        self.write_report(&report).await;

        info!(
            event = "curator_run_complete",
            skills_reviewed = skills.len(),
            "Curator run complete"
        );
        Ok(report)
    }

    /// Process parsed curator actions and build the report.
    async fn process_actions(&self, actions: Vec<CuratorAction>, total: usize) -> String {
        let mut lines = vec![
            format!(
                "# Curator Run — {}",
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            ),
            format!("\nSkills reviewed: {total}"),
            String::new(),
        ];

        for action in actions {
            match action {
                CuratorAction::Keep(slug) => {
                    lines.push(format!("- KEEP: {slug}"));
                    if let Ok(Some(mut doc)) = self.store.get(&slug).await {
                        doc.front_matter.updated_at = Utc::now();
                        if let Err(e) = self.store.save(&doc).await {
                            warn!(slug, error = %e, "Failed to touch KEEP skill timestamp");
                        }
                    }
                }
                CuratorAction::Prune { slug, reason } => {
                    lines.push(format!("- PRUNE: {slug} (reason: {reason})"));
                    match self.store.archive(&slug).await {
                        Ok(true) => lines.push(format!("  archived: {slug}")),
                        Ok(false) => lines.push(format!("  not found: {slug}")),
                        Err(e) => lines.push(format!("  error archiving {slug}: {e}")),
                    }
                }
                CuratorAction::Consolidate {
                    from_slugs,
                    to_slug,
                    merged_content,
                } => {
                    lines.push(format!(
                        "- CONSOLIDATE: {} -> {to_slug}",
                        from_slugs.join(", ")
                    ));
                    if merged_content.is_empty() {
                        lines.push("  skipped: no merged SKILL.md provided".to_string());
                        continue;
                    }
                    match SkillDoc::parse(&merged_content) {
                        Ok(mut new_doc) => {
                            new_doc.front_matter.name = to_slug.clone();
                            match self.store.save(&new_doc).await {
                                Ok(_) => {
                                    lines.push(format!("  created: {to_slug}"));
                                    for slug in &from_slugs {
                                        match self.store.archive(slug).await {
                                            Ok(_) => lines.push(format!("  archived: {slug}")),
                                            Err(e) => {
                                                lines.push(format!("  error archiving {slug}: {e}"))
                                            }
                                        }
                                    }
                                }
                                Err(e) => lines.push(format!("  error saving {to_slug}: {e}")),
                            }
                        }
                        Err(e) => lines.push(format!("  error parsing merged SKILL.md: {e}")),
                    }
                }
            }
        }

        lines.join("\n")
    }

    /// Write the curator report to `<logs_dir>/curator/YYYY-MM-DD.md`.
    async fn write_report(&self, report: &str) {
        let dir = self.logs_dir.join("curator");
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            warn!(error = %e, "Failed to create curator log directory");
            return;
        }
        let path = dir.join(format!("{}.md", Utc::now().format("%Y-%m-%d")));
        if let Err(e) = tokio::fs::write(&path, report.as_bytes()).await {
            warn!(error = %e, path = %path.display(), "Failed to write curator report");
        }
    }

    /// Return the date string of the most recent curator run, if any.
    pub async fn last_run_date(&self) -> Option<String> {
        let dir = self.logs_dir.join("curator");
        let mut entries = tokio::fs::read_dir(&dir).await.ok()?;
        let mut dates: Vec<String> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") {
                dates.push(name.trim_end_matches(".md").to_string());
            }
        }
        dates.sort();
        dates.last().cloned()
    }

    /// Spawn the background cron loop.  Returns immediately; the task runs
    /// until the process exits.
    pub async fn start_background(self: Arc<Self>) {
        let schedule = match Schedule::from_str(&self.schedule_expr) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    error = %e,
                    schedule = %self.schedule_expr,
                    "Invalid curator cron expression"
                );
                return;
            }
        };

        info!(
            schedule = %self.schedule_expr,
            "Skill curator background task started"
        );

        loop {
            let next = match schedule.upcoming(chrono::Utc).next() {
                Some(n) => n,
                None => {
                    error!("Curator schedule has no more upcoming runs");
                    break;
                }
            };

            let now = Utc::now();
            let delay = (next - now)
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(3600));

            info!(next_run = %next, "Curator sleeping until next scheduled run");
            tokio::time::sleep(delay).await;

            info!("Skill curator starting scheduled run");
            match self.run_once().await {
                Ok(report) => info!(lines = report.lines().count(), "Skill curator run complete"),
                Err(e) => error!(error = %e, "Skill curator run failed"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Response parser
// ---------------------------------------------------------------------------

fn parse_curator_response(response: &str) -> Vec<CuratorAction> {
    let lines: Vec<&str> = response.lines().collect();
    let mut actions: Vec<CuratorAction> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();

        if let Some(rest) = line.strip_prefix("KEEP ") {
            let slug = rest.trim().to_string();
            if !slug.is_empty() {
                actions.push(CuratorAction::Keep(slug));
            }
            i += 1;
        } else if let Some(rest) = line.strip_prefix("PRUNE ") {
            let (slug, reason) = rest
                .split_once(' ')
                .map(|(s, r)| (s.to_string(), r.trim().to_string()))
                .unwrap_or_else(|| (rest.to_string(), String::new()));
            actions.push(CuratorAction::Prune { slug, reason });
            i += 1;
        } else if let Some(rest) = line.strip_prefix("CONSOLIDATE ") {
            if let Some((from_part, to_slug)) = rest.split_once("->") {
                let from_slugs: Vec<String> =
                    from_part.split_whitespace().map(String::from).collect();
                let to_slug = to_slug.trim().to_string();

                // Collect any immediately following SKILL.md block.
                i += 1;
                let mut skill_lines: Vec<&str> = Vec::new();
                let mut dash_count = 0usize;

                while i < lines.len() {
                    let next = lines[i].trim();
                    // Stop when we hit the next directive
                    if dash_count >= 2
                        && (next.starts_with("KEEP ")
                            || next.starts_with("PRUNE ")
                            || next.starts_with("CONSOLIDATE "))
                    {
                        break;
                    }
                    if next == "---" {
                        dash_count += 1;
                    }
                    skill_lines.push(lines[i]);
                    i += 1;
                    // Stop collecting after we've seen the closing --- and at least one body line
                    if dash_count >= 2 && !skill_lines.is_empty() {
                        // Check for at least one non-dash line after second ---
                        let after_second: Vec<&str> = skill_lines
                            .iter()
                            .rev()
                            .take_while(|l| l.trim() != "---")
                            .copied()
                            .collect();
                        if !after_second.is_empty() {
                            // peek next line
                            if i >= lines.len()
                                || lines[i].trim().starts_with("KEEP ")
                                || lines[i].trim().starts_with("PRUNE ")
                                || lines[i].trim().starts_with("CONSOLIDATE ")
                            {
                                break;
                            }
                        }
                    }
                }

                let merged_content = skill_lines.join("\n");
                actions.push(CuratorAction::Consolidate {
                    from_slugs,
                    to_slug,
                    merged_content,
                });
                // i is already advanced
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    actions
}

fn build_skill_summary(skills: &[SkillDoc]) -> String {
    skills
        .iter()
        .map(|s| {
            let last = s
                .front_matter
                .last_used
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "never".to_string());
            format!(
                "- {} (use_count: {}, last_used: {}): {}",
                s.front_matter.name, s.front_matter.use_count, last, s.front_matter.description,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
