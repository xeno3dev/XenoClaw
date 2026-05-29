//! XenoClaw Skill Library — auto-created, progressive-loading skill documents.
//!
//! Skills are markdown files with YAML front matter stored under `~/.xenoclaw/skills/`.
//! The system has three components:
//! - `SkillStore`: reads/writes skill docs from disk with a 60-second cache
//! - `SkillLoader`: three-level progressive disclosure (name-only → full → ref files)
//! - `SkillCurator`: background agent that prunes/consolidates the skill library

pub mod curator;
pub mod loader;
pub mod skill;
pub mod store;

pub use curator::SkillCurator;
pub use loader::SkillLoader;
pub use skill::{SkillDoc, SkillError, SkillFrontMatter};
pub use store::SkillStore;
