//! Plugin directory watcher for hot-reload support.
//!
//! Monitors the plugins directory for file changes and triggers plugin
//! reload within 10 seconds of detecting a change. Allows in-flight
//! operations to complete or timeout within 30 seconds.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::loader::PluginLoader;

/// Configuration for the plugin watcher.
#[derive(Debug, Clone)]
pub struct WatcherConfig {
    /// How long to debounce file change events before triggering reload.
    /// Must be less than 10 seconds to meet the requirement.
    pub debounce_duration: Duration,

    /// Maximum time to wait for in-flight operations to complete during reload.
    pub graceful_timeout: Duration,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            // Debounce for 2 seconds to batch rapid changes
            debounce_duration: Duration::from_secs(2),
            // Allow 30 seconds for in-flight operations
            graceful_timeout: Duration::from_secs(30),
        }
    }
}

/// The plugin watcher monitors the plugins directory and triggers hot-reloads.
///
/// Uses the `notify` crate for filesystem event detection. Changes are debounced
/// to avoid excessive reloads during rapid file modifications (e.g., during a
/// plugin build process).
pub struct PluginWatcher {
    /// Configuration for the watcher.
    config: WatcherConfig,
    /// The plugin loader to trigger reloads on.
    loader: Arc<PluginLoader>,
    /// Whether the watcher is currently running.
    running: Arc<RwLock<bool>>,
    /// Handle to stop the watcher.
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl PluginWatcher {
    /// Create a new plugin watcher.
    pub fn new(loader: Arc<PluginLoader>, config: WatcherConfig) -> Self {
        Self {
            config,
            loader,
            running: Arc::new(RwLock::new(false)),
            stop_tx: None,
        }
    }

    /// Start watching the plugins directory for changes.
    ///
    /// Spawns a background task that monitors filesystem events and triggers
    /// plugin reloads as needed. Returns immediately.
    pub async fn start(&mut self) -> Result<(), String> {
        let plugins_dir = self.loader.plugins_dir().to_path_buf();

        if !plugins_dir.exists() {
            std::fs::create_dir_all(&plugins_dir)
                .map_err(|e| format!("Failed to create plugins directory: {e}"))?;
        }

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        self.stop_tx = Some(stop_tx);

        let loader = Arc::clone(&self.loader);
        let config = self.config.clone();
        let running = Arc::clone(&self.running);

        // Mark as running
        {
            let mut r = running.write().await;
            *r = true;
        }

        // Spawn the watcher task
        tokio::spawn(async move {
            if let Err(e) = run_watcher(plugins_dir, loader, config, running.clone(), stop_rx).await
            {
                error!(error = %e, "Plugin watcher failed");
            }
            let mut r = running.write().await;
            *r = false;
        });

        info!("Plugin watcher started");
        Ok(())
    }

    /// Stop the plugin watcher.
    pub async fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }

        // Wait for the watcher to stop
        let mut attempts = 0;
        while *self.running.read().await && attempts < 10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            attempts += 1;
        }

        info!("Plugin watcher stopped");
    }

    /// Check if the watcher is currently running.
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}

/// Internal function that runs the filesystem watcher loop.
async fn run_watcher(
    plugins_dir: PathBuf,
    loader: Arc<PluginLoader>,
    config: WatcherConfig,
    running: Arc<RwLock<bool>>,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), String> {
    let (tx, mut rx) = mpsc::channel::<NotifyEvent>(100);

    // Create the filesystem watcher
    let mut watcher = RecommendedWatcher::new(
        move |result: Result<NotifyEvent, notify::Error>| {
            if let Ok(event) = result {
                let _ = tx.blocking_send(event);
            }
        },
        notify::Config::default(),
    )
    .map_err(|e| format!("Failed to create filesystem watcher: {e}"))?;

    // Start watching the plugins directory
    watcher
        .watch(&plugins_dir, RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch plugins directory: {e}"))?;

    info!(dir = %plugins_dir.display(), "Watching plugins directory for changes");

    // Track which plugins need reloading (debounce)
    let mut pending_reloads: HashSet<String> = HashSet::new();
    let mut debounce_timer: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            // Check for stop signal
            _ = &mut stop_rx => {
                debug!("Plugin watcher received stop signal");
                break;
            }

            // Process filesystem events
            event = rx.recv() => {
                match event {
                    Some(notify_event) => {
                        if let Some(plugin_name) = extract_plugin_name(&notify_event, &plugins_dir) {
                            if is_relevant_change(&notify_event.kind) {
                                debug!(
                                    plugin = %plugin_name,
                                    kind = ?notify_event.kind,
                                    "File change detected in plugin directory"
                                );
                                pending_reloads.insert(plugin_name);
                                debounce_timer = Some(tokio::time::Instant::now() + config.debounce_duration);
                            }
                        }
                    }
                    None => {
                        warn!("Filesystem event channel closed");
                        break;
                    }
                }
            }

            // Check debounce timer
            _ = async {
                if let Some(deadline) = debounce_timer {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    // No timer set, sleep forever (will be interrupted by other branches)
                    std::future::pending::<()>().await;
                }
            } => {
                // Debounce period elapsed, process pending reloads
                if !pending_reloads.is_empty() {
                    let plugins_to_reload: Vec<String> = pending_reloads.drain().collect();
                    process_reloads(&loader, &plugins_to_reload, &config, &plugins_dir).await;
                }
                debounce_timer = None;
            }
        }
    }

    let mut r = running.write().await;
    *r = false;
    Ok(())
}

/// Extract the plugin name from a filesystem event.
///
/// Determines which plugin directory a changed file belongs to by
/// examining the path relative to the plugins directory.
fn extract_plugin_name(event: &NotifyEvent, plugins_dir: &PathBuf) -> Option<String> {
    for path in &event.paths {
        if let Ok(relative) = path.strip_prefix(plugins_dir) {
            // The first component of the relative path is the plugin directory name
            if let Some(first_component) = relative.components().next() {
                if let Some(name) = first_component.as_os_str().to_str() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// Check if a filesystem event kind is relevant for plugin reload.
fn is_relevant_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Process pending plugin reloads.
async fn process_reloads(
    loader: &Arc<PluginLoader>,
    plugin_names: &[String],
    config: &WatcherConfig,
    plugins_dir: &PathBuf,
) {
    for plugin_name in plugin_names {
        info!(plugin = %plugin_name, "Processing hot-reload for plugin");

        // Check if the plugin is currently loaded
        if loader.is_loaded(plugin_name).await {
            // Reload existing plugin (with graceful timeout for in-flight ops)
            match tokio::time::timeout(config.graceful_timeout, loader.reload_plugin(plugin_name))
                .await
            {
                Ok(Ok(manifest)) => {
                    info!(
                        plugin = %plugin_name,
                        version = %manifest.version,
                        "Plugin hot-reloaded successfully"
                    );
                }
                Ok(Err(e)) => {
                    error!(
                        plugin = %plugin_name,
                        error = %e,
                        "Plugin hot-reload failed"
                    );
                }
                Err(_) => {
                    error!(
                        plugin = %plugin_name,
                        timeout_secs = config.graceful_timeout.as_secs(),
                        "Plugin hot-reload timed out waiting for in-flight operations"
                    );
                }
            }
        } else {
            // New plugin detected — try to load it
            let plugin_dir = plugins_dir.join(plugin_name);
            if plugin_dir.is_dir() && plugin_dir.join("plugin.toml").exists() {
                match loader.load_plugin(&plugin_dir).await {
                    Ok(manifest) => {
                        info!(
                            plugin = %plugin_name,
                            version = %manifest.version,
                            "New plugin discovered and loaded via hot-reload"
                        );
                    }
                    Err(e) => {
                        error!(
                            plugin = %plugin_name,
                            error = %e,
                            "Failed to load newly discovered plugin"
                        );
                    }
                }
            } else if !plugin_dir.exists() {
                // Plugin directory was removed — it's already unloaded
                debug!(plugin = %plugin_name, "Plugin directory removed (already unloaded)");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::PluginLoader;
    use agent_core::event_bus::EventBus;
    use agent_core::tool_registry::ToolRegistry;
    use common::config::ResourceLimits;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_loader(plugins_dir: &std::path::Path) -> Arc<PluginLoader> {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let limits = ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        };

        Arc::new(PluginLoader::new(
            plugins_dir.to_path_buf(),
            registry,
            event_bus,
            limits,
        ))
    }

    #[test]
    fn test_extract_plugin_name() {
        let plugins_dir = PathBuf::from("/home/user/plugins");
        let event = NotifyEvent {
            kind: EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            paths: vec![PathBuf::from("/home/user/plugins/my-plugin/plugin.toml")],
            attrs: Default::default(),
        };

        let name = extract_plugin_name(&event, &plugins_dir);
        assert_eq!(name, Some("my-plugin".to_string()));
    }

    #[test]
    fn test_extract_plugin_name_nested_file() {
        let plugins_dir = PathBuf::from("/plugins");
        let event = NotifyEvent {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/plugins/test-plugin/src/main.rs")],
            attrs: Default::default(),
        };

        let name = extract_plugin_name(&event, &plugins_dir);
        assert_eq!(name, Some("test-plugin".to_string()));
    }

    #[test]
    fn test_extract_plugin_name_no_match() {
        let plugins_dir = PathBuf::from("/plugins");
        let event = NotifyEvent {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![PathBuf::from("/other/path/file.txt")],
            attrs: Default::default(),
        };

        let name = extract_plugin_name(&event, &plugins_dir);
        assert_eq!(name, None);
    }

    #[test]
    fn test_is_relevant_change() {
        assert!(is_relevant_change(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_relevant_change(&EventKind::Modify(
            notify::event::ModifyKind::Any
        )));
        assert!(is_relevant_change(&EventKind::Remove(
            notify::event::RemoveKind::File
        )));
        assert!(!is_relevant_change(&EventKind::Access(
            notify::event::AccessKind::Read
        )));
    }

    #[test]
    fn test_watcher_config_default() {
        let config = WatcherConfig::default();
        assert_eq!(config.debounce_duration, Duration::from_secs(2));
        assert_eq!(config.graceful_timeout, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_watcher_start_and_stop() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path()).unwrap();

        let loader = create_test_loader(tmp.path());
        let mut watcher = PluginWatcher::new(loader, WatcherConfig::default());

        // Start the watcher
        let result = watcher.start().await;
        assert!(result.is_ok());

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(watcher.is_running().await);

        // Stop the watcher
        watcher.stop().await;

        // Give it a moment to stop
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!watcher.is_running().await);
    }
}
