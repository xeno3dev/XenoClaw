//! Skill document model with YAML front matter parsing.
//!
//! Each skill lives in `~/.xenoclaw/skills/<slug>/SKILL.md`.
//! The file begins with a YAML front matter block delimited by `---` lines,
//! followed by the skill body in plain Markdown.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when parsing or manipulating skill documents.
#[derive(Debug, Error)]
pub enum SkillError {
    #[error("invalid front matter: {0}")]
    InvalidFrontMatter(String),
    #[error("missing required field: {0}")]
    MissingField(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("anyhow error: {0}")]
    Other(#[from] anyhow::Error),
}

/// YAML front matter for a skill document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontMatter {
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub use_count: u64,
    pub last_used: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}

/// A parsed skill document (front matter + body).
#[derive(Debug, Clone)]
pub struct SkillDoc {
    pub front_matter: SkillFrontMatter,
    /// Everything after the closing `---` delimiter.
    pub body: String,
}

impl SkillDoc {
    /// Parse a `SKILL.md` file's contents into a `SkillDoc`.
    pub fn parse(content: &str) -> Result<Self, SkillError> {
        let s = content.trim_start();

        if !s.starts_with("---") {
            return Err(SkillError::InvalidFrontMatter(
                "expected opening ---".to_string(),
            ));
        }

        // Skip past the opening "---" and any immediately following newline.
        let after_dashes = &s[3..];
        let open_end = after_dashes
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(after_dashes.len());
        let after_open = &after_dashes[open_end..];

        // Find the closing "---".
        let close = after_open
            .find("\n---")
            .ok_or_else(|| SkillError::InvalidFrontMatter("missing closing ---".to_string()))?;

        let yaml_str = &after_open[..close];
        let after_close = &after_open[close + 4..]; // skip "\n---"
        let body = after_close.trim_start_matches('\n').to_string();

        let front_matter = parse_front_matter(yaml_str)?;
        Ok(Self { front_matter, body })
    }

    /// Render the skill doc back to `SKILL.md` format.
    pub fn render(&self) -> String {
        let fm = &self.front_matter;
        let last_used = fm
            .last_used
            .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_else(|| "null".to_string());
        let tags = if fm.tags.is_empty() {
            "[]".to_string()
        } else {
            format!("[{}]", fm.tags.join(", "))
        };
        format!(
            "---\nname: {name}\ndescription: {desc}\ncreated_at: {created}\nupdated_at: {updated}\nuse_count: {count}\nlast_used: {last_used}\ntags: {tags}\n---\n\n{body}",
            name = fm.name,
            desc = fm.description,
            created = fm.created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            updated = fm.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            count = fm.use_count,
            last_used = last_used,
            tags = tags,
            body = self.body,
        )
    }

    /// Create a fresh skill doc with default metadata.
    pub fn new(name: String, description: String, body: String) -> Self {
        let now = Utc::now();
        Self {
            front_matter: SkillFrontMatter {
                name,
                description,
                created_at: now,
                updated_at: now,
                use_count: 0,
                last_used: None,
                tags: Vec::new(),
            },
            body,
        }
    }
}

// ---------------------------------------------------------------------------
// Front matter parser helpers
// ---------------------------------------------------------------------------

fn parse_front_matter(yaml: &str) -> Result<SkillFrontMatter, SkillError> {
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut created_at: Option<DateTime<Utc>> = None;
    let mut updated_at: Option<DateTime<Utc>> = None;
    let mut use_count: u64 = 0;
    let mut last_used: Option<DateTime<Utc>> = None;
    let mut tags: Vec<String> = Vec::new();

    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let colon = match line.find(':') {
            Some(c) => c,
            None => continue,
        };
        let key = line[..colon].trim();
        let value = line[colon + 1..].trim();

        match key {
            "name" => name = Some(strip_quotes(value)),
            "description" => description = Some(strip_quotes(value)),
            "created_at" => {
                if !value.is_empty() {
                    created_at = Some(parse_dt_or_err(key, value)?);
                }
            }
            "updated_at" => {
                if !value.is_empty() {
                    updated_at = Some(parse_dt_or_err(key, value)?);
                }
            }
            "use_count" => {
                if !value.is_empty() {
                    use_count = value.parse().map_err(|_| {
                        SkillError::InvalidFrontMatter(format!("invalid use_count: {value}"))
                    })?;
                }
            }
            "last_used" => {
                if value != "null" && value != "~" && !value.is_empty() {
                    last_used = Some(parse_dt_or_err(key, value)?);
                }
            }
            "tags" => tags = parse_tag_list(value),
            _ => {}
        }
    }

    Ok(SkillFrontMatter {
        name: name.ok_or_else(|| SkillError::MissingField("name".to_string()))?,
        description: description
            .ok_or_else(|| SkillError::MissingField("description".to_string()))?,
        created_at: created_at.unwrap_or_else(Utc::now),
        updated_at: updated_at.unwrap_or_else(Utc::now),
        use_count,
        last_used,
        tags,
    })
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn parse_dt_or_err(key: &str, value: &str) -> Result<DateTime<Utc>, SkillError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|d| d.with_timezone(&Utc))
        .map_err(|_| SkillError::InvalidFrontMatter(format!("invalid {key}: {value}")))
}

fn parse_tag_list(value: &str) -> Vec<String> {
    let value = value.trim();
    if value == "[]" || value.is_empty() {
        return Vec::new();
    }
    let inner = value.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|s| strip_quotes(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"---
name: deploy-flask
description: Deploy Flask apps to Coolify
created_at: 2024-01-01T00:00:00Z
updated_at: 2024-01-15T10:00:00Z
use_count: 3
last_used: 2024-01-15T10:00:00Z
tags: [deploy, flask]
---

# Deploy Flask to Coolify

Step 1: ...
"#;

    #[test]
    fn round_trip() {
        let doc = SkillDoc::parse(SAMPLE).unwrap();
        assert_eq!(doc.front_matter.name, "deploy-flask");
        assert_eq!(doc.front_matter.use_count, 3);
        assert_eq!(doc.front_matter.tags, vec!["deploy", "flask"]);
        assert!(doc.body.contains("Step 1"));

        let rendered = doc.render();
        let doc2 = SkillDoc::parse(&rendered).unwrap();
        assert_eq!(doc2.front_matter.name, doc.front_matter.name);
        assert_eq!(doc2.front_matter.use_count, doc.front_matter.use_count);
    }

    #[test]
    fn null_last_used() {
        let content = "---\nname: foo\ndescription: bar\ncreated_at: 2024-01-01T00:00:00Z\nupdated_at: 2024-01-01T00:00:00Z\nuse_count: 0\nlast_used: null\ntags: []\n---\n\nbody";
        let doc = SkillDoc::parse(content).unwrap();
        assert!(doc.front_matter.last_used.is_none());
    }
}
