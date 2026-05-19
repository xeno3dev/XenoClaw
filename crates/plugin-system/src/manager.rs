//! Plugin Manager — top-level coordinator for the plugin system.
//!
//! The PluginManager orchestrates plugin discovery, loading, hot-reload watching,
//! and lifecycle management. It is the primary entry point for the plugin system.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info};

use agent_core::event_bus::EventBus;
use agent_core::tool_registry::ToolRegistry;
use common::config::{PluginConfig, ResourceLimits};
use common::errors::PluginError;

use crate::api::PluginApi;
use crate::loader::{PluginInfo, PluginLoadResult, PluginLoader};
use crate::manifest::PluginManifest;
use crate::watcher::{PluginWatcher, WatcherConfig};

/// The Plugin Manager coordinates all plugin system operations.
///
/// Provides the top-level interface described in the design document's
/// `PluginSystem` trait:
/// - `load_all()` — discover and load all plugins from the configured directory
/// - `reload_plugin(name)` — reload a specific plugin
/// - `list_plugins()` — list all loaded plugins
/// - `plugin_api()` — get the PluginApi for a specific plugin
pub struct PluginManager {
    /// The plugin loader handles discovery and loading.
    loader: Arc<PluginLoader>,
    /// The file watcher for hot-reload support.
    watcher: Option<PluginWatcher>,
    /// Plugin system configuration.
    config: PluginConfig,
}

impl PluginManager {
    /// Create a new PluginManager with the given configuration.
    ///
    /// # Arguments
    /// - `config` — Plugin system configuration (directory, reload interval, etc.)
    /// - `tool_registry` — Shared tool registry for plugin tool registration
    /// - `event_bus` — Shared event bus for plugin event handler registration
    /// - `resource_limits` — Security layer resource limits to enforce on plugins
    pub fn new(
        config: PluginConfig,
        tool_registry: Arc<RwLock<ToolRegistry>>,
        event_bus: EventBus,
        resource_limits: ResourceLimits,
    ) -> Self {
        let loader = Arc::new(PluginLoader::new(
            config.directory.clone(),
            tool_registry,
            event_bus,
            resource_limits,
        ));

        Self {
            loader,
            watcher: None,
            config,
        }
    }

    /// Initialize the plugin system: load all plugins and start the file watcher.
    ///
    /// This is the main startup method. It:
    /// 1. Discovers and loads all plugins from the configured directory
    /// 2. Starts the filesystem watcher for hot-reload (if enabled)
    ///
    /// Returns the results of the initial plugin load.
    pub async fn initialize(&mut self) -> Vec<PluginLoadResult> {
        if !self.config.enabled {
            info!("Plugin system is disabled by configuration");
            return Vec::new();
        }

        info!(
            dir = %self.config.directory.display(),
            "Initializing plugin system"
        );

        // Load all plugins from the directory
        let results = self.loader.load_all().await;

        // Start the file watcher for hot-reload
        if let Err(e) = self.start_watcher().await {
            error!(error = %e, "Failed to start plugin file watcher");
        }

        results
    }

    /// Start the filesystem watcher for hot-reload support.
    async fn start_watcher(&mut self) -> Result<(), String> {
        let watcher_config = WatcherConfig {
            debounce_duration: std::time::Duration::from_secs(
                self.config.reload_interval_seconds.min(10) as u64 / 2,
            ),
            graceful_timeout: std::time::Duration::from_secs(30),
        };

        let mut watcher = PluginWatcher::new(Arc::clone(&self.loader), watcher_config);
        watcher.start().await?;
        self.watcher = Some(watcher);

        Ok(())
    }

    /// Load all plugins from the configured directory.
    ///
    /// Can be called to re-scan the directory and load any new plugins
    /// that weren't present during initialization.
    pub async fn load_all(&self) -> Vec<PluginLoadResult> {
        self.loader.load_all().await
    }

    /// Reload a specific plugin by name.
    ///
    /// Unloads the current version (allowing in-flight operations to complete
    /// within 30 seconds) and loads the new version from disk.
    pub async fn reload_plugin(&self, name: &str) -> Result<PluginManifest, PluginError> {
        self.loader.reload_plugin(name).await
    }

    /// Unload a specific plugin by name.
    pub async fn unload_plugin(&self, name: &str) -> Result<(), PluginError> {
        self.loader.unload_plugin(name).await
    }

    /// List all loaded plugins with their information.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        self.loader.list_plugins().await
    }

    /// Get the PluginApi instance for a specific plugin.
    ///
    /// Returns `None` if the plugin is not loaded.
    pub async fn plugin_api(&self, plugin_name: &str) -> Option<Arc<PluginApi>> {
        self.loader.get_plugin_api(plugin_name).await
    }

    /// Get information about a specific plugin.
    pub async fn get_plugin(&self, name: &str) -> Option<PluginInfo> {
        self.loader.get_plugin(name).await
    }

    /// Check if a plugin is currently loaded.
    pub async fn is_loaded(&self, name: &str) -> bool {
        self.loader.is_loaded(name).await
    }

    /// Get the number of loaded plugins.
    pub async fn plugin_count(&self) -> usize {
        self.loader.plugin_count().await
    }

    /// Get the plugins directory path.
    pub fn plugins_dir(&self) -> &PathBuf {
        &self.config.directory
    }

    /// Shutdown the plugin system.
    ///
    /// Stops the file watcher and unloads all plugins.
    pub async fn shutdown(&mut self) {
        info!("Shutting down plugin system");

        // Stop the watcher
        if let Some(ref mut watcher) = self.watcher {
            watcher.stop().await;
        }
        self.watcher = None;

        // Unload all plugins
        let plugins = self.loader.list_plugins().await;
        for plugin in plugins {
            if let Err(e) = self.loader.unload_plugin(&plugin.manifest.name).await {
                error!(
                    plugin = %plugin.manifest.name,
                    error = %e,
                    "Error unloading plugin during shutdown"
                );
            }
        }

        info!("Plugin system shutdown complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_config(dir: &std::path::Path) -> PluginConfig {
        PluginConfig {
            enabled: true,
            directory: dir.to_path_buf(),
            reload_interval_seconds: 10,
        }
    }

    fn create_test_manager(dir: &std::path::Path) -> PluginManager {
        let config = create_test_config(dir);
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let limits = ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        };

        PluginManager::new(config, registry, event_bus, limits)
    }

    #[tokio::test]
    async fn test_initialize_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let mut manager = create_test_manager(tmp.path());

        let results = manager.initialize().await;
        assert!(results.is_empty());
        assert_eq!(manager.plugin_count().await, 0);
    }

    #[tokio::test]
    async fn test_initialize_with_plugins() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("test-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "test-plugin"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let mut manager = create_test_manager(tmp.path());
        let results = manager.initialize().await;

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
        assert_eq!(manager.plugin_count().await, 1);
        assert!(manager.is_loaded("test-plugin").await);

        // Cleanup
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_initialize_disabled() {
        let tmp = TempDir::new().unwrap();
        let config = PluginConfig {
            enabled: false,
            directory: tmp.path().to_path_buf(),
            reload_interval_seconds: 10,
        };
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let limits = ResourceLimits::default();

        let mut manager = PluginManager::new(config, registry, event_bus, limits);
        let results = manager.initialize().await;

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_reload_plugin() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("reload-test");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "reload-test"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let mut manager = create_test_manager(tmp.path());
        manager.initialize().await;

        // Update the manifest
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "reload-test"
version = "2.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let result = manager.reload_plugin("reload-test").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().version, semver::Version::new(2, 0, 0));

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_unload_plugin() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("unload-test");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "unload-test"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let mut manager = create_test_manager(tmp.path());
        manager.initialize().await;

        assert!(manager.is_loaded("unload-test").await);

        let result = manager.unload_plugin("unload-test").await;
        assert!(result.is_ok());
        assert!(!manager.is_loaded("unload-test").await);

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let tmp = TempDir::new().unwrap();

        for name in &["plugin-a", "plugin-b"] {
            let dir = tmp.path().join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("plugin.toml"),
                format!(
                    r#"
name = "{name}"
version = "1.0.0"
api_version = "0.1.0"
"#
                ),
            )
            .unwrap();
        }

        let mut manager = create_test_manager(tmp.path());
        manager.initialize().await;

        let plugins = manager.list_plugins().await;
        assert_eq!(plugins.len(), 2);

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_plugin_api_access() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("api-access");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "api-access"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let mut manager = create_test_manager(tmp.path());
        manager.initialize().await;

        let api = manager.plugin_api("api-access").await;
        assert!(api.is_some());
        assert_eq!(api.unwrap().plugin_name(), "api-access");

        let no_api = manager.plugin_api("nonexistent").await;
        assert!(no_api.is_none());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_graceful_failure_handling() {
        let tmp = TempDir::new().unwrap();

        // Create one valid and one invalid plugin
        let good_dir = tmp.path().join("good");
        fs::create_dir_all(&good_dir).unwrap();
        fs::write(
            good_dir.join("plugin.toml"),
            r#"
name = "good"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let bad_dir = tmp.path().join("bad");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("plugin.toml"), "invalid toml content {{{").unwrap();

        let mut manager = create_test_manager(tmp.path());
        let results = manager.initialize().await;

        // Both should be attempted
        assert_eq!(results.len(), 2);

        // Good plugin should be loaded despite bad plugin failing
        assert!(manager.is_loaded("good").await);
        assert!(!manager.is_loaded("bad").await);

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_shutdown() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join("shutdown-test");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("plugin.toml"),
            r#"
name = "shutdown-test"
version = "1.0.0"
api_version = "0.1.0"
"#,
        )
        .unwrap();

        let mut manager = create_test_manager(tmp.path());
        manager.initialize().await;

        assert_eq!(manager.plugin_count().await, 1);

        manager.shutdown().await;

        assert_eq!(manager.plugin_count().await, 0);
    }
}
