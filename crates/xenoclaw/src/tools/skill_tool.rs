//! `use_skill` tool — loads the full SKILL.md for a named skill (Level 1).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use agent_core::Tool;
use skills::SkillLoader;

/// Tool that loads a skill's full instructions into the agent's context.
pub struct UseSkillTool {
    loader: Arc<SkillLoader>,
}

impl UseSkillTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str {
        "use_skill"
    }

    fn description(&self) -> &str {
        "Load the full instructions for a skill by name. \
         Use this when the skill index lists a skill that is relevant to the current task."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "The slug name of the skill to load (as listed in the skill index)."
                }
            },
            "required": ["skill_name"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let skill_name = arguments
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: skill_name".to_string())?;

        if skill_name.contains("..") || skill_name.starts_with('/') || skill_name.contains('\\') {
            return Err("Invalid skill_name".to_string());
        }

        match self.loader.load_skill(skill_name).await {
            Some(content) => Ok(content),
            None => Err(format!(
                "Skill '{}' not found in the skill library.",
                skill_name
            )),
        }
    }
}
