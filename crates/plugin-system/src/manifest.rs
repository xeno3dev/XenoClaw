//! Plugin manifest parsing and validation.
//!
//! Each plugin must provide a `plugin.toml` manifest file declaring:
//! - `name`: Unique plugin identifier
//! - `version`: Semantic version of the plugin
//! - `api_version`: Required platform API version
//!
//! Optional fields include description, entry_point, and permissions.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::debug;

use common::errors::PluginError;

/// The current API version supported by the platform.
pub const CURRENT_API_VERSION: &str = "0.1.0";

/// A validated plugin manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique name identifying this plugin.
    pub name: String,

    /// Semantic version of the plugin.
    pub version: semver::Version,

    /// Required platform API version.
    pub api_version: semver::Version,

    /// Human-readable description of the plugin.
    #[serde(default)]
    pub description: String,

    /// Entry point file relative to the plugin directory.
    #[serde(default = "default_entry_point")]
    pub entry_point: String,

    /// Permissions requested by the plugin.
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

fn default_entry_point() -> String {
    "plugin.wasm".to_string()
}

/// Permissions a plugin can request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Access to the filesystem (scoped by security layer).
    Filesystem,
    /// Access to the network (scoped by security layer).
    Network,
    /// Ability to spawn processes (scoped by security layer).
    Process,
    /// Access to the memory store (scoped to plugin namespace).
    Memory,
}

impl PluginManifest {
    /// Load and validate a plugin manifest from a TOML file.
    ///
    /// Returns the parsed manifest or a `PluginError::InvalidManifest` if
    /// the file is missing, unreadable, or contains invalid data.
    pub fn load(manifest_path: &Path) -> Result<Self, PluginError> {
        let plugin_name = manifest_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content =
            std::fs::read_to_string(manifest_path).map_err(|e| PluginError::InvalidManifest {
                name: plugin_name.clone(),
                reason: format!("cannot read manifest file: {e}"),
            })?;

        let manifest: PluginManifest =
            toml::from_str(&content).map_err(|e| PluginError::InvalidManifest {
                name: plugin_name.clone(),
                reason: format!("invalid TOML: {e}"),
            })?;

        // Validate required fields are non-empty
        manifest.validate()?;

        debug!(
            plugin = %manifest.name,
            version = %manifest.version,
            api_version = %manifest.api_version,
            "Plugin manifest loaded successfully"
        );

        Ok(manifest)
    }

    /// Validate the manifest fields.
    fn validate(&self) -> Result<(), PluginError> {
        if self.name.is_empty() {
            return Err(PluginError::InvalidManifest {
                name: "(empty)".to_string(),
                reason: "plugin name must not be empty".to_string(),
            });
        }

        // Check that the plugin name contains only valid characters
        if !self
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(PluginError::InvalidManifest {
                name: self.name.clone(),
                reason:
                    "plugin name must contain only alphanumeric characters, hyphens, or underscores"
                        .to_string(),
            });
        }

        // Check API version compatibility
        let current_api =
            semver::Version::parse(CURRENT_API_VERSION).expect("CURRENT_API_VERSION is valid");

        if self.api_version.major > current_api.major {
            return Err(PluginError::InvalidManifest {
                name: self.name.clone(),
                reason: format!(
                    "plugin requires API version {} but platform provides {}",
                    self.api_version, current_api
                ),
            });
        }

        Ok(())
    }

    /// Check if this manifest is compatible with the current platform API version.
    pub fn is_api_compatible(&self) -> bool {
        let current_api =
            semver::Version::parse(CURRENT_API_VERSION).expect("CURRENT_API_VERSION is valid");
        self.api_version.major <= current_api.major
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_manifest_file(dir: &Path, content: &str) -> std::path::PathBuf {
        let manifest_path = dir.join("plugin.toml");
        fs::write(&manifest_path, content).unwrap();
        manifest_path
    }

    #[test]
    fn test_valid_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("my-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = r#"
name = "my-plugin"
version = "1.0.0"
api_version = "0.1.0"
description = "A test plugin"
entry_point = "plugin.wasm"
permissions = ["filesystem", "network"]
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let manifest = PluginManifest::load(&path).unwrap();

        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, semver::Version::new(1, 0, 0));
        assert_eq!(manifest.api_version, semver::Version::new(0, 1, 0));
        assert_eq!(manifest.description, "A test plugin");
        assert_eq!(manifest.permissions.len(), 2);
        assert!(manifest.is_api_compatible());
    }

    #[test]
    fn test_minimal_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("minimal");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = r#"
name = "minimal"
version = "0.1.0"
api_version = "0.1.0"
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let manifest = PluginManifest::load(&path).unwrap();

        assert_eq!(manifest.name, "minimal");
        assert_eq!(manifest.entry_point, "plugin.wasm");
        assert!(manifest.permissions.is_empty());
    }

    #[test]
    fn test_empty_name_rejected() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("bad");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = r#"
name = ""
version = "1.0.0"
api_version = "0.1.0"
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let result = PluginManifest::load(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::InvalidManifest { .. }));
    }

    #[test]
    fn test_invalid_name_characters() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("bad-chars");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = r#"
name = "my plugin/bad"
version = "1.0.0"
api_version = "0.1.0"
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let result = PluginManifest::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_incompatible_api_version() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("future");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = r#"
name = "future-plugin"
version = "1.0.0"
api_version = "99.0.0"
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let result = PluginManifest::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("broken");
        fs::create_dir_all(&plugin_dir).unwrap();

        let content = "this is not valid toml {{{";
        let path = create_manifest_file(&plugin_dir, content);
        let result = PluginManifest::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_required_fields() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("incomplete");
        fs::create_dir_all(&plugin_dir).unwrap();

        // Missing version and api_version
        let content = r#"
name = "incomplete"
"#;

        let path = create_manifest_file(&plugin_dir, content);
        let result = PluginManifest::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_nonexistent_file() {
        let path = Path::new("/nonexistent/plugin.toml");
        let result = PluginManifest::load(path);
        assert!(result.is_err());
    }
}
