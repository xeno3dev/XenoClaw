//! Persistent skill store backed by the `~/.xenoclaw/skills/` directory.
//!
//! Skills are stored as `<skills_dir>/<slug>/SKILL.md`.  The store maintains
//! a 60-second in-memory cache of the full skill list; any write invalidates
//! the cache immediately.  Archived (deleted) skills are moved to
//! `<skills_dir>/.archive/<slug>/` and never permanently deleted.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::warn;

use crate::skill::{SkillDoc, SkillError};

const CACHE_TTL: Duration = Duration::from_secs(60);

/// Validate that a slug is safe to use as a filesystem component.
///
/// A valid slug is: non-empty, not absolute, only one path component,
/// and does not start with `.`.
fn validate_slug(slug: &str) -> Result<(), SkillError> {
    let path = std::path::Path::new(slug);
    if slug.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || slug.starts_with('.')
    {
        return Err(SkillError::InvalidFrontMatter(format!(
            "invalid skill slug: {slug}"
        )));
    }
    Ok(())
}

struct CachedIndex {
    skills: Vec<SkillDoc>,
    built_at: Instant,
}

/// Thread-safe store for loading and saving skill documents.
pub struct SkillStore {
    skills_dir: PathBuf,
    cache: Arc<RwLock<Option<CachedIndex>>>,
}

impl std::fmt::Debug for SkillStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillStore")
            .field("skills_dir", &self.skills_dir)
            .finish_non_exhaustive()
    }
}

impl SkillStore {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Return the configured skills directory.
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    /// Create the skills directory tree (including `.archive/`) if absent.
    pub async fn ensure_dir(&self) -> Result<(), SkillError> {
        tokio::fs::create_dir_all(&self.skills_dir).await?;
        tokio::fs::create_dir_all(self.skills_dir.join(".archive")).await?;
        Ok(())
    }

    /// List all skills, using the cache when fresh.
    pub async fn list(&self) -> Result<Vec<SkillDoc>, SkillError> {
        {
            let cache = self.cache.read().await;
            if let Some(ref c) = *cache {
                if c.built_at.elapsed() < CACHE_TTL {
                    return Ok(c.skills.clone());
                }
            }
        }

        let skills = self.load_from_disk().await?;

        {
            let mut cache = self.cache.write().await;
            *cache = Some(CachedIndex {
                skills: skills.clone(),
                built_at: Instant::now(),
            });
        }

        Ok(skills)
    }

    /// Load all skills from disk (bypasses cache).
    async fn load_from_disk(&self) -> Result<Vec<SkillDoc>, SkillError> {
        let mut skills = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.skills_dir).await {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden dirs (.archive, etc.)
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            let skill_path = entry.path().join("SKILL.md");
            if let Ok(content) = tokio::fs::read_to_string(&skill_path).await {
                match SkillDoc::parse(&content) {
                    Ok(doc) => skills.push(doc),
                    Err(e) => warn!(
                        path = %skill_path.display(),
                        error = %e,
                        "Skipping unparseable skill"
                    ),
                }
            }
            // dir exists but no SKILL.md — skip silently
        }

        skills.sort_by(|a, b| a.front_matter.name.cmp(&b.front_matter.name));
        Ok(skills)
    }

    /// Load a single skill by slug (bypasses cache).
    pub async fn get(&self, name: &str) -> Result<Option<SkillDoc>, SkillError> {
        validate_slug(name)?;
        let path = self.skills_dir.join(name).join("SKILL.md");
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => Ok(Some(SkillDoc::parse(&content)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(SkillError::Io(e)),
        }
    }

    /// Atomically write a skill document to disk, then invalidate the cache.
    pub async fn save(&self, doc: &SkillDoc) -> Result<(), SkillError> {
        validate_slug(&doc.front_matter.name)?;
        let dir = self.skills_dir.join(&doc.front_matter.name);
        tokio::fs::create_dir_all(&dir).await?;

        let final_path = dir.join("SKILL.md");
        let tmp_path = dir.join("SKILL.md.tmp");
        let content = doc.render();

        tokio::fs::write(&tmp_path, content.as_bytes()).await?;
        tokio::fs::rename(&tmp_path, &final_path).await?;

        self.invalidate_cache().await;
        Ok(())
    }

    /// Move a skill to `.archive/<slug>/`.  Returns `true` if the skill existed.
    pub async fn archive(&self, name: &str) -> Result<bool, SkillError> {
        validate_slug(name)?;
        let src = self.skills_dir.join(name);
        if !src.exists() {
            return Ok(false);
        }
        tokio::fs::create_dir_all(self.skills_dir.join(".archive")).await?;
        let dst = self.skills_dir.join(".archive").join(name);
        tokio::fs::rename(&src, &dst).await?;
        self.invalidate_cache().await;
        Ok(true)
    }

    /// Drop the in-memory cache so the next `list()` reads from disk.
    pub async fn invalidate_cache(&self) {
        *self.cache.write().await = None;
    }
}
