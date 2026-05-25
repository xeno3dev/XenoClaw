//! Progressive skill loader — three-level disclosure to bound token usage.
//!
//! - **Level 0** (`level0_index`): name + description only, injected into every request.
//! - **Level 1** (`load_skill`): full `SKILL.md` content, on demand.
//! - **Level 2** (`load_ref`): a specific file from `refs/`, on demand.

use std::sync::Arc;

use crate::store::SkillStore;

/// Progressive loader wrapping a shared `SkillStore`.
pub struct SkillLoader {
    store: Arc<SkillStore>,
}

impl SkillLoader {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self { store }
    }

    /// Return a compact skill index string for Level 0 injection.
    ///
    /// Keeps total output under ~3 000 tokens by listing only name and
    /// one-line description for each skill.
    pub async fn level0_index(&self) -> String {
        let skills = match self.store.list().await {
            Ok(s) => s,
            Err(_) => return String::new(),
        };

        if skills.is_empty() {
            return "Available skills: (none)".to_string();
        }

        let mut lines = vec!["Available skills:".to_string()];
        for skill in &skills {
            lines.push(format!(
                "- {}: {}",
                skill.front_matter.name, skill.front_matter.description,
            ));
        }
        lines.join("\n")
    }

    /// Load the full `SKILL.md` content for a named skill (Level 1).
    pub async fn load_skill(&self, name: &str) -> Option<String> {
        self.store
            .get(name)
            .await
            .ok()
            .flatten()
            .map(|doc| doc.render())
    }

    /// Load a specific ref file within a skill's `refs/` directory (Level 2).
    ///
    /// The `ref_file` parameter is validated to prevent path traversal.
    pub async fn load_ref(&self, skill: &str, ref_file: &str) -> Option<String> {
        // Guard against path traversal
        if ref_file.contains("..") || ref_file.starts_with('/') || ref_file.contains('\\') {
            return None;
        }
        let path = self
            .store
            .skills_dir()
            .join(skill)
            .join("refs")
            .join(ref_file);
        tokio::fs::read_to_string(&path).await.ok()
    }
}
