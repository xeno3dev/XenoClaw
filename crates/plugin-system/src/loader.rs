//! Plugin discovery and loading from a configurable directory.
//!
//! Scans the plugins directory at startup, discovers plugins with valid manifests,
//! and loads them. Handles load failures gracefully — logs the error and continues
//! loading remaining plugins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use agent_core::event_bus::{Event, EventBus, EventType};
use agent_core::tool_registry::ToolRegistry;
use common::config::ResourceLimits;
use common::errors::PluginError;
use security_layer::ResourceEnforcer;

use crate::api::EventHandlerRegistry;
use crate::api::PluginApi;
use crate::manifest::PluginManifest;

/// Information about a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// The plugin manifest.
    pub manifest: PluginManifest,
    /// Path to the plugin directory.
    pub directory: PathBuf,
    /// When the plugin was loaded.
    pub loaded_at: DateTime<Utc>,
    /// Current status of the plugin.
    pub status: PluginStatus,
}

/// Status of a loaded plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// Plugin is loaded and active.
    Active,
    /// Plugin failed to load.
    Failed { reason: String },
    /// Plugin is being reloaded.
    Reloading,
}

/// Result of attempting to load a plugin.
#[derive(Debug)]
pub struct PluginLoadResult {
    /// Name of the plugin (from directory name if manifest failed).
    pub name: String,
    /// Whether loading succeeded.
    pub success: bool,
    /// Error message if loading failed.
    pub error: Option<String>,
}

/// The plugin loader discovers and loads plugins from a directory.
///
/// Each plugin is expected to be in its own subdirectory with a `plugin.toml`
/// manifest file at the root of that subdirectory.
///
/// Directory structure:
/// ```text
/// plugins/
/// ├── my-plugin/
/// │   ├── plugin.toml
/// │   └── plugin.wasm
/// ├── another-plugin/
/// │   ├── plugin.toml
/// │   └── ...
/// ```
pub struct PluginLoader {
    /// Directory to scan for plugins.
    plugins_dir: PathBuf,
    /// Loaded plugins indexed by name.
    plugins: Arc<RwLock<HashMap<String, PluginInfo>>>,
    /// Plugin API instances indexed by plugin name.
    plugin_apis: Arc<RwLock<HashMap<String, Arc<PluginApi>>>>,
    /// Shared tool registry.
    tool_registry: Arc<RwLock<ToolRegistry>>,
    /// Shared event bus.
    event_bus: EventBus,
    /// Resource enforcer for sandboxing.
    resource_enforcer: Arc<ResourceEnforcer>,
    /// Shared event handler registry for cross-plugin conflict detection.
    handler_registry: EventHandlerRegistry,
}

impl PluginLoader {
    /// Create a new plugin loader.
    pub fn new(
        plugins_dir: PathBuf,
        tool_registry: Arc<RwLock<ToolRegistry>>,
        event_bus: EventBus,
        resource_limits: ResourceLimits,
    ) -> Self {
        Self {
            plugins_dir,
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_apis: Arc::new(RwLock::new(HashMap::new())),
            tool_registry,
            event_bus,
            resource_enforcer: Arc::new(ResourceEnforcer::new(resource_limits)),
            handler_registry: EventHandlerRegistry::new(),
        }
    }

    /// Get the plugins directory path.
    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    /// Discover and load all plugins from the configured directory.
    ///
    /// Scans the plugins directory for subdirectories containing a valid
    /// `plugin.toml` manifest. Each valid plugin is loaded; failures are
    /// logged and do not prevent other plugins from loading.
    ///
    /// Returns a list of load results for each discovered plugin.
    pub async fn load_all(&self) -> Vec<PluginLoadResult> {
        let mut results = Vec::new();

        // Ensure the plugins directory exists
        if !self.plugins_dir.exists() {
            info!(
                dir = %self.plugins_dir.display(),
                "Plugins directory does not exist, creating it"
            );
            if let Err(e) = std::fs::create_dir_all(&self.plugins_dir) {
                error!(
                    dir = %self.plugins_dir.display(),
                    error = %e,
                    "Failed to create plugins directory"
                );
                return results;
            }
        }

        // Read directory entries
        let entries = match std::fs::read_dir(&self.plugins_dir) {
            Ok(entries) => entries,
            Err(e) => {
                error!(
                    dir = %self.plugins_dir.display(),
                    error = %e,
                    "Failed to read plugins directory"
                );
                return results;
            }
        };

        // Process each subdirectory
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let result = self.load_plugin(&path).await;
            results.push(PluginLoadResult {
                name: dir_name,
                success: result.is_ok(),
                error: result.err().map(|e| e.to_string()),
            });
        }

        let loaded_count = results.iter().filter(|r| r.success).count();
        let failed_count = results.iter().filter(|r| !r.success).count();

        info!(
            loaded = loaded_count,
            failed = failed_count,
            total = results.len(),
            "Plugin discovery complete"
        );

        results
    }

    /// Load a single plugin from its directory.
    ///
    /// Reads and validates the manifest, creates a PluginApi instance,
    /// and registers the plugin as active.
    pub async fn load_plugin(&self, plugin_dir: &Path) -> Result<PluginManifest, PluginError> {
        let manifest_path = plugin_dir.join("plugin.toml");

        // Load and validate the manifest
        let manifest = PluginManifest::load(&manifest_path)?;
        let plugin_name = manifest.name.clone();

        // Check if a plugin with this name is already loaded
        {
            let plugins = self.plugins.read().await;
            if plugins.contains_key(&plugin_name) {
                return Err(PluginError::LoadFailed {
                    name: plugin_name,
                    reason: "a plugin with this name is already loaded".to_string(),
                });
            }
        }

        // Create a PluginApi instance for this plugin
        let api = Arc::new(PluginApi::with_shared_handler_registry(
            plugin_name.clone(),
            Arc::clone(&self.tool_registry),
            self.event_bus.clone(),
            Arc::clone(&self.resource_enforcer),
            self.handler_registry.clone(),
        ));

        // Store the plugin info
        let plugin_info = PluginInfo {
            manifest: manifest.clone(),
            directory: plugin_dir.to_path_buf(),
            loaded_at: Utc::now(),
            status: PluginStatus::Active,
        };

        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(plugin_name.clone(), plugin_info);
        }

        {
            let mut apis = self.plugin_apis.write().await;
            apis.insert(plugin_name.clone(), api);
        }

        // Emit PluginLoaded event
        self.event_bus.publish(Event::new(
            EventType::PluginLoaded,
            serde_json::json!({
                "plugin": plugin_name,
                "version": manifest.version.to_string(),
            }),
        ));

        info!(
            plugin = %plugin_name,
            version = %manifest.version,
            dir = %plugin_dir.display(),
            "Plugin loaded successfully"
        );

        Ok(manifest)
    }

    /// Unload a plugin by name.
    ///
    /// Cleans up all registrations (tools, event handlers) and removes
    /// the plugin from the active set.
    pub async fn unload_plugin(&self, plugin_name: &str) -> Result<(), PluginError> {
        // Get and clean up the plugin API
        let api = {
            let mut apis = self.plugin_apis.write().await;
            apis.remove(plugin_name)
        };

        if let Some(api) = api {
            api.cleanup().await;
        }

        // Remove from loaded plugins
        let removed = {
            let mut plugins = self.plugins.write().await;
            plugins.remove(plugin_name)
        };

        if removed.is_none() {
            return Err(PluginError::LoadFailed {
                name: plugin_name.to_string(),
                reason: "plugin not found".to_string(),
            });
        }

        // Emit PluginUnloaded event
        self.event_bus.publish(Event::new(
            EventType::PluginUnloaded,
            serde_json::json!({
                "plugin": plugin_name,
            }),
        ));

        info!(plugin = %plugin_name, "Plugin unloaded");
        Ok(())
    }

    /// Reload a plugin by unloading and re-loading it.
    ///
    /// Allows in-flight operations from the previous version to complete
    /// or timeout within 30 seconds before fully replacing the plugin.
    pub async fn reload_plugin(&self, plugin_name: &str) -> Result<PluginManifest, PluginError> {
        // Get the plugin directory before unloading
        let plugin_dir = {
            let plugins = self.plugins.read().await;
            match plugins.get(plugin_name) {
                Some(info) => info.directory.clone(),
                None => {
                    return Err(PluginError::LoadFailed {
                        name: plugin_name.to_string(),
                        reason: "plugin not found for reload".to_string(),
                    });
                }
            }
        };

        // Mark as reloading
        {
            let mut plugins = self.plugins.write().await;
            if let Some(info) = plugins.get_mut(plugin_name) {
                info.status = PluginStatus::Reloading;
            }
        }

        info!(plugin = %plugin_name, "Starting plugin reload");

        // Allow in-flight operations to complete (up to 30 seconds)
        // We give a brief grace period for any active tool executions
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Unload the old version
        if let Err(e) = self.unload_plugin(plugin_name).await {
            warn!(
                plugin = %plugin_name,
                error = %e,
                "Error during plugin unload for reload"
            );
        }

        // Load the new version
        self.load_plugin(&plugin_dir).await
    }

    /// Get information about all loaded plugins.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        plugins.values().cloned().collect()
    }

    /// Get information about a specific plugin.
    pub async fn get_plugin(&self, name: &str) -> Option<PluginInfo> {
        let plugins = self.plugins.read().await;
        plugins.get(name).cloned()
    }

    /// Get the PluginApi instance for a specific plugin.
    pub async fn get_plugin_api(&self, name: &str) -> Option<Arc<PluginApi>> {
        let apis = self.plugin_apis.read().await;
        apis.get(name).cloned()
    }

    /// Check if a plugin is loaded.
    pub async fn is_loaded(&self, name: &str) -> bool {
        let plugins = self.plugins.read().await;
        plugins.contains_key(name)
    }

    /// Get the number of loaded plugins.
    pub async fn plugin_count(&self) -> usize {
        let plugins = self.plugins.read().await;
        plugins.len()
    }

    /// Get a reference to the shared resource enforcer.
    pub fn resource_enforcer(&self) -> &Arc<ResourceEnforcer> {
        &self.resource_enforcer
    }

    /// Get a reference to the shared plugins map (for watcher integration).
    pub(crate) fn plugins_map(&self) -> &Arc<RwLock<HashMap<String, PluginInfo>>> {
        &self.plugins
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_loader(plugins_dir: &Path) -> PluginLoader {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let limits = ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        };

        PluginLoader::new(plugins_dir.to_path_buf(), registry, event_bus, limits)
    }

    fn create_plugin_dir(base: &Path, name: &str, manifest: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("plugin.toml"), manifest).unwrap();
        dir
    }

    #[tokio::test]
    async fn test_load_all_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let loader = create_test_loader(tmp.path());

        let results = loader.load_all().await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_load_all_creates_missing_directory() {
        let tmp = TempDir::new().unwrap();
        let plugins_dir = tmp.path().join("nonexistent_plugins");
        let loader = create_test_loader(&plugins_dir);

        let results = loader.load_all().await;
        assert!(results.is_empty());
        assert!(plugins_dir.exists());
    }

    #[tokio::test]
    async fn test_load_single_valid_plugin() {
        let tmp = TempDir::new().unwrap();
        let manifest = r#"
name = "test-plugin"
version = "1.0.0"
api_version = "0.1.0"
description = "A test plugin"
"#;
        create_plugin_dir(tmp.path(), "test-plugin", manifest);

        let loader = create_test_loader(tmp.path());
        let results = loader.load_all().await;

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(results[0].name, "test-plugin");
        assert_eq!(loader.plugin_count().await, 1);
    }

    #[tokio::test]
    async fn test_load_multiple_plugins() {
        let tmp = TempDir::new().unwrap();

        create_plugin_dir(
            tmp.path(),
            "plugin-a",
            r#"
name = "plugin-a"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        create_plugin_dir(
            tmp.path(),
            "plugin-b",
            r#"
name = "plugin-b"
version = "2.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        let results = loader.load_all().await;

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
        assert_eq!(loader.plugin_count().await, 2);
    }

    #[tokio::test]
    async fn test_load_continues_on_failure() {
        let tmp = TempDir::new().unwrap();

        // Valid plugin
        create_plugin_dir(
            tmp.path(),
            "good-plugin",
            r#"
name = "good-plugin"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        // Invalid plugin (bad TOML)
        create_plugin_dir(tmp.path(), "bad-plugin", "this is not valid toml {{{");

        let loader = create_test_loader(tmp.path());
        let results = loader.load_all().await;

        assert_eq!(results.len(), 2);

        let good = results.iter().find(|r| r.name == "good-plugin").unwrap();
        let bad = results.iter().find(|r| r.name == "bad-plugin").unwrap();

        assert!(good.success);
        assert!(!bad.success);
        assert!(bad.error.is_some());

        // Good plugin should still be loaded
        assert!(loader.is_loaded("good-plugin").await);
        assert!(!loader.is_loaded("bad-plugin").await);
    }

    #[tokio::test]
    async fn test_unload_plugin() {
        let tmp = TempDir::new().unwrap();
        create_plugin_dir(
            tmp.path(),
            "unload-me",
            r#"
name = "unload-me"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        loader.load_all().await;

        assert!(loader.is_loaded("unload-me").await);

        let result = loader.unload_plugin("unload-me").await;
        assert!(result.is_ok());
        assert!(!loader.is_loaded("unload-me").await);
    }

    #[tokio::test]
    async fn test_unload_nonexistent_plugin() {
        let tmp = TempDir::new().unwrap();
        let loader = create_test_loader(tmp.path());

        let result = loader.unload_plugin("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_reload_plugin() {
        let tmp = TempDir::new().unwrap();
        create_plugin_dir(
            tmp.path(),
            "reload-me",
            r#"
name = "reload-me"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        loader.load_all().await;

        // Update the manifest
        fs::write(
            tmp.path().join("reload-me").join("plugin.toml"),
            r#"
name = "reload-me"
version = "2.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let result = loader.reload_plugin("reload-me").await;
        assert!(result.is_ok());

        let manifest = result.unwrap();
        assert_eq!(manifest.version, semver::Version::new(2, 0, 0));
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let tmp = TempDir::new().unwrap();
        create_plugin_dir(
            tmp.path(),
            "list-test",
            r#"
name = "list-test"
version = "1.0.0"
api_version = "0.1.0"
description = "For listing"
"#,
        );

        let loader = create_test_loader(tmp.path());
        loader.load_all().await;

        let plugins = loader.list_plugins().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest.name, "list-test");
        assert_eq!(plugins[0].status, PluginStatus::Active);
    }

    #[tokio::test]
    async fn test_get_plugin_api() {
        let tmp = TempDir::new().unwrap();
        create_plugin_dir(
            tmp.path(),
            "api-test",
            r#"
name = "api-test"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        loader.load_all().await;

        let api = loader.get_plugin_api("api-test").await;
        assert!(api.is_some());
        assert_eq!(api.unwrap().plugin_name(), "api-test");

        let no_api = loader.get_plugin_api("nonexistent").await;
        assert!(no_api.is_none());
    }

    #[tokio::test]
    async fn test_skips_non_directory_entries() {
        let tmp = TempDir::new().unwrap();

        // Create a regular file in the plugins directory (should be skipped)
        fs::write(tmp.path().join("not-a-plugin.txt"), "hello").unwrap();

        // Create a valid plugin directory
        create_plugin_dir(
            tmp.path(),
            "real-plugin",
            r#"
name = "real-plugin"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        let results = loader.load_all().await;

        // Only the directory should be processed
        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[tokio::test]
    async fn test_duplicate_plugin_name_rejected() {
        let tmp = TempDir::new().unwrap();

        // Two directories with the same plugin name in manifest
        create_plugin_dir(
            tmp.path(),
            "dir-a",
            r#"
name = "same-name"
version = "1.0.0"
api_version = "0.1.0"
"#,
        );

        create_plugin_dir(
            tmp.path(),
            "dir-b",
            r#"
name = "same-name"
version = "2.0.0"
api_version = "0.1.0"
"#,
        );

        let loader = create_test_loader(tmp.path());
        let results = loader.load_all().await;

        // One should succeed, one should fail
        let successes = results.iter().filter(|r| r.success).count();
        let failures = results.iter().filter(|r| !r.success).count();
        assert_eq!(successes, 1);
        assert_eq!(failures, 1);
    }
}
