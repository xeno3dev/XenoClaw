//! Plugin System — extensibility framework for the VPS AI Agent Platform.
//!
//! The plugin system provides:
//! - **Discovery**: Scan a configurable directory for plugins with valid manifests
//! - **Loading**: Load plugins and provide them with a sandboxed API
//! - **Hot-reload**: Detect file changes and reload plugins within 10 seconds
//! - **Fault isolation**: Plugin failures don't affect the agent or other plugins
//! - **Security**: Enforce Security Layer resource limits on plugin operations
//!
//! # Architecture
//!
//! ```text
//! PluginManager
//! ├── PluginLoader (discovery + loading)
//! ├── PluginWatcher (hot-reload via filesystem events)
//! └── PluginApi (per-plugin sandboxed interface)
//!     ├── Tool Registration → ToolRegistry
//!     ├── Event Handlers → EventBus
//!     └── Resource Enforcement → SecurityLayer
//! ```
//!
//! # Plugin Structure
//!
//! Each plugin lives in its own subdirectory under the configured plugins directory:
//!
//! ```text
//! plugins/
//! ├── my-plugin/
//! │   ├── plugin.toml    (manifest: name, version, api_version)
//! │   └── plugin.wasm    (entry point)
//! └── another-plugin/
//!     ├── plugin.toml
//!     └── ...
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//! use plugin_system::PluginManager;
//! use agent_core::{EventBus, ToolRegistry};
//! use common::config::{PluginConfig, ResourceLimits};
//!
//! # async fn example() {
//! let config = PluginConfig::default();
//! let registry = Arc::new(RwLock::new(ToolRegistry::new()));
//! let event_bus = EventBus::new(256);
//! let limits = ResourceLimits::default();
//!
//! let mut manager = PluginManager::new(config, registry, event_bus, limits);
//! let results = manager.initialize().await;
//!
//! for result in &results {
//!     if result.success {
//!         println!("Loaded plugin: {}", result.name);
//!     } else {
//!         println!("Failed to load: {} - {:?}", result.name, result.error);
//!     }
//! }
//! # }
//! ```

pub mod api;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod watcher;

// Re-export primary types for convenience
pub use api::{EventHandler, EventHandlerFn, EventHandlerRegistry, PluginApi, SandboxedTool};
pub use loader::{PluginInfo, PluginLoadResult, PluginLoader, PluginStatus};
pub use manager::PluginManager;
pub use manifest::{Permission, PluginManifest, CURRENT_API_VERSION};
pub use watcher::{PluginWatcher, WatcherConfig};
