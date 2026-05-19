//! Plugin API — the interface available to plugins for registering tools and event handlers.
//!
//! Provides `PluginApi` which plugins use to:
//! - Register tools with the Tool Registry
//! - Register event handlers with the Event Bus
//! - Log messages
//!
//! All operations are scoped to the plugin and enforce Security Layer resource limits.
//!
//! ## Conflict Resolution (Requirement 13.7)
//!
//! Both tool and event handler registrations enforce first-registration-wins semantics:
//! - If a tool name is already registered (by any plugin or built-in), the new registration
//!   is rejected with a `PluginError::NameConflict` identifying both plugins.
//! - If an event handler name is already registered (by any plugin), the new registration
//!   is rejected with a `PluginError::NameConflict` identifying both plugins.
//! - The system continues operating with the original registration intact.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use agent_core::event_bus::{EventBus, EventType};
use agent_core::tool_registry::{Tool, ToolRegistry};
use common::errors::PluginError;
use security_layer::ResourceEnforcer;

/// A callback function for handling events.
pub type EventHandlerFn = Arc<dyn Fn(Value) + Send + Sync>;

/// An event handler registered by a plugin.
#[derive(Clone)]
pub struct EventHandler {
    /// The plugin that registered this handler.
    pub plugin_name: String,
    /// The event type this handler listens for.
    pub event_type: EventType,
    /// A unique name for this handler.
    pub handler_name: String,
    /// The handler callback.
    pub callback: EventHandlerFn,
}

impl std::fmt::Debug for EventHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventHandler")
            .field("plugin_name", &self.plugin_name)
            .field("event_type", &self.event_type)
            .field("handler_name", &self.handler_name)
            .finish()
    }
}

/// A shared registry that tracks event handler name ownership across all plugins.
///
/// This ensures that handler names are globally unique — if plugin A registers
/// a handler named "on_task_complete", plugin B cannot register a handler with
/// the same name (first registration wins, per Requirement 13.7).
#[derive(Debug, Clone, Default)]
pub struct EventHandlerRegistry {
    /// Maps handler_name -> plugin_name that owns it.
    handlers: Arc<RwLock<HashMap<String, String>>>,
}

impl EventHandlerRegistry {
    /// Create a new empty event handler registry.
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attempt to register a handler name for a plugin.
    ///
    /// Returns `Ok(())` if the name is available, or `Err(existing_plugin)` if
    /// the name is already taken by another plugin.
    pub async fn register(&self, handler_name: &str, plugin_name: &str) -> Result<(), String> {
        let mut handlers = self.handlers.write().await;
        if let Some(existing) = handlers.get(handler_name) {
            Err(existing.clone())
        } else {
            handlers.insert(handler_name.to_string(), plugin_name.to_string());
            Ok(())
        }
    }

    /// Unregister a handler name.
    pub async fn unregister(&self, handler_name: &str) {
        let mut handlers = self.handlers.write().await;
        handlers.remove(handler_name);
    }

    /// Unregister all handler names owned by a specific plugin.
    pub async fn unregister_plugin(&self, plugin_name: &str) {
        let mut handlers = self.handlers.write().await;
        handlers.retain(|_, owner| owner != plugin_name);
    }

    /// Get the plugin that owns a handler name, if any.
    pub async fn owner(&self, handler_name: &str) -> Option<String> {
        let handlers = self.handlers.read().await;
        handlers.get(handler_name).cloned()
    }
}

/// The API surface available to plugins for registering capabilities.
///
/// Each plugin gets its own `PluginApi` instance scoped to its name.
/// All registrations are tracked so they can be cleaned up on plugin unload.
pub struct PluginApi {
    /// The name of the plugin this API instance belongs to.
    plugin_name: String,
    /// Shared reference to the tool registry.
    tool_registry: Arc<RwLock<ToolRegistry>>,
    /// Shared reference to the event bus.
    event_bus: EventBus,
    /// Local event handlers for this plugin (handler_name -> handler).
    event_handlers: Arc<RwLock<HashMap<String, EventHandler>>>,
    /// Shared event handler registry for cross-plugin conflict detection.
    handler_registry: EventHandlerRegistry,
    /// Resource enforcer for sandboxing plugin operations.
    resource_enforcer: Arc<ResourceEnforcer>,
}

impl PluginApi {
    /// Create a new PluginApi instance for a specific plugin.
    pub fn new(
        plugin_name: String,
        tool_registry: Arc<RwLock<ToolRegistry>>,
        event_bus: EventBus,
        resource_enforcer: Arc<ResourceEnforcer>,
    ) -> Self {
        Self {
            plugin_name,
            tool_registry,
            event_bus,
            event_handlers: Arc::new(RwLock::new(HashMap::new())),
            handler_registry: EventHandlerRegistry::new(),
            resource_enforcer,
        }
    }

    /// Create a new PluginApi instance with a shared event handler registry.
    ///
    /// This constructor should be used when multiple plugins share the same
    /// handler registry for cross-plugin conflict detection.
    pub fn with_shared_handler_registry(
        plugin_name: String,
        tool_registry: Arc<RwLock<ToolRegistry>>,
        event_bus: EventBus,
        resource_enforcer: Arc<ResourceEnforcer>,
        handler_registry: EventHandlerRegistry,
    ) -> Self {
        Self {
            plugin_name,
            tool_registry,
            event_bus,
            event_handlers: Arc::new(RwLock::new(HashMap::new())),
            handler_registry,
            resource_enforcer,
        }
    }

    /// Get the plugin name this API is scoped to.
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    /// Register a new tool with the Tool Registry.
    ///
    /// The tool is tracked as belonging to this plugin. If a tool with the same
    /// name already exists (registered by another plugin or built-in), the
    /// registration is rejected (first registration wins, per Req 13.7).
    pub async fn register_tool(&self, tool: Arc<dyn Tool>) -> Result<(), PluginError> {
        let tool_name = tool.name().to_string();
        let mut registry = self.tool_registry.write().await;

        if registry.register_plugin_tool(&self.plugin_name, tool) {
            info!(
                plugin = %self.plugin_name,
                tool = %tool_name,
                "Plugin tool registered successfully"
            );
            Ok(())
        } else {
            // Determine who owns the conflicting registration
            let existing_owner = registry
                .tool_plugin(&tool_name)
                .unwrap_or("built-in")
                .to_string();

            warn!(
                plugin = %self.plugin_name,
                tool = %tool_name,
                existing_owner = %existing_owner,
                "Plugin tool registration rejected: name conflict"
            );

            Err(PluginError::NameConflict {
                name: tool_name,
                existing_plugin: existing_owner,
            })
        }
    }

    /// Register an event handler for a specific event type.
    ///
    /// The handler is identified by a unique name. If a handler with the same
    /// name already exists (registered by this plugin or any other plugin),
    /// the registration is rejected (first registration wins, per Req 13.7).
    pub async fn register_event_handler(
        &self,
        handler_name: &str,
        event_type: EventType,
        callback: EventHandlerFn,
    ) -> Result<(), PluginError> {
        // Check the shared handler registry for cross-plugin conflicts
        if let Err(existing_plugin) = self.handler_registry.register(handler_name, &self.plugin_name).await {
            error!(
                plugin = %self.plugin_name,
                handler = %handler_name,
                existing_plugin = %existing_plugin,
                "Event handler registration rejected: name conflict with another plugin"
            );
            return Err(PluginError::NameConflict {
                name: handler_name.to_string(),
                existing_plugin,
            });
        }

        let mut handlers = self.event_handlers.write().await;

        let handler = EventHandler {
            plugin_name: self.plugin_name.clone(),
            event_type,
            handler_name: handler_name.to_string(),
            callback,
        };

        handlers.insert(handler_name.to_string(), handler);

        info!(
            plugin = %self.plugin_name,
            handler = %handler_name,
            event_type = %event_type,
            "Plugin event handler registered"
        );

        Ok(())
    }

    /// Get the event bus for subscribing to events.
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// Get the resource enforcer for spawning sandboxed processes.
    pub fn resource_enforcer(&self) -> &ResourceEnforcer {
        &self.resource_enforcer
    }

    /// Get the shared event handler registry.
    pub fn handler_registry(&self) -> &EventHandlerRegistry {
        &self.handler_registry
    }

    /// Get all registered event handlers for this plugin.
    pub async fn event_handlers(&self) -> Vec<EventHandler> {
        let handlers = self.event_handlers.read().await;
        handlers.values().cloned().collect()
    }

    /// Unregister all tools and handlers for this plugin.
    ///
    /// Called during plugin unload to clean up registrations.
    pub async fn cleanup(&self) {
        // Unregister all tools
        {
            let mut registry = self.tool_registry.write().await;
            let removed = registry.unregister_plugin_tools(&self.plugin_name);
            if !removed.is_empty() {
                debug!(
                    plugin = %self.plugin_name,
                    tools = ?removed,
                    "Cleaned up plugin tools"
                );
            }
        }

        // Clear event handlers from both local and shared registries
        {
            let mut handlers = self.event_handlers.write().await;
            let count = handlers.len();
            handlers.clear();
            if count > 0 {
                debug!(
                    plugin = %self.plugin_name,
                    handler_count = count,
                    "Cleaned up plugin event handlers"
                );
            }
        }

        // Remove all handler names owned by this plugin from the shared registry
        self.handler_registry.unregister_plugin(&self.plugin_name).await;

        info!(plugin = %self.plugin_name, "Plugin API cleanup complete");
    }

    /// Log a message on behalf of the plugin.
    pub fn log(&self, level: tracing::Level, message: &str) {
        match level {
            tracing::Level::ERROR => error!(plugin = %self.plugin_name, "{}", message),
            tracing::Level::WARN => warn!(plugin = %self.plugin_name, "{}", message),
            tracing::Level::INFO => info!(plugin = %self.plugin_name, "{}", message),
            tracing::Level::DEBUG => debug!(plugin = %self.plugin_name, "{}", message),
            _ => debug!(plugin = %self.plugin_name, "{}", message),
        }
    }
}

/// A wrapper tool that enforces resource limits on plugin tool execution.
///
/// Wraps a plugin-provided tool and ensures that the Security Layer's
/// resource limits are respected during execution.
pub struct SandboxedTool {
    inner: Arc<dyn Tool>,
    plugin_name: String,
    resource_enforcer: Arc<ResourceEnforcer>,
}

impl SandboxedTool {
    /// Create a new sandboxed tool wrapper.
    pub fn new(
        inner: Arc<dyn Tool>,
        plugin_name: String,
        resource_enforcer: Arc<ResourceEnforcer>,
    ) -> Self {
        Self {
            inner,
            plugin_name,
            resource_enforcer,
        }
    }
}

#[async_trait]
impl Tool for SandboxedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        // Check resource limits before execution
        let active_count = self.resource_enforcer.active_process_count().await;
        let max_processes = self.resource_enforcer.limits().max_processes;

        if active_count >= max_processes as usize {
            return Err(format!(
                "Plugin '{}' tool execution denied: resource limit reached ({}/{} processes)",
                self.plugin_name, active_count, max_processes
            ));
        }

        // Execute the inner tool with a timeout to prevent runaway operations
        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.inner.execute(arguments),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "Plugin '{}' tool '{}' timed out after 30 seconds",
                self.plugin_name,
                self.inner.name()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::event_bus::EventBus;
    use common::config::ResourceLimits;
    use serde_json::json;

    /// A simple test tool for plugin API tests.
    struct TestTool {
        tool_name: String,
    }

    impl TestTool {
        fn new(name: &str) -> Self {
            Self {
                tool_name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "A test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn execute(&self, _arguments: Value) -> Result<String, String> {
            Ok(format!("executed {}", self.tool_name))
        }
    }

    fn create_test_api(plugin_name: &str) -> PluginApi {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        }));

        PluginApi::new(
            plugin_name.to_string(),
            registry,
            event_bus,
            enforcer,
        )
    }

    /// Create two PluginApi instances sharing the same tool registry and handler registry.
    fn create_shared_apis(name_a: &str, name_b: &str) -> (PluginApi, PluginApi) {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits::default()));
        let handler_registry = EventHandlerRegistry::new();

        let api_a = PluginApi::with_shared_handler_registry(
            name_a.to_string(),
            Arc::clone(&registry),
            event_bus.clone(),
            Arc::clone(&enforcer),
            handler_registry.clone(),
        );
        let api_b = PluginApi::with_shared_handler_registry(
            name_b.to_string(),
            Arc::clone(&registry),
            event_bus,
            enforcer,
            handler_registry,
        );

        (api_a, api_b)
    }

    #[tokio::test]
    async fn test_register_tool() {
        let api = create_test_api("test-plugin");
        let tool = Arc::new(TestTool::new("my_tool"));

        let result = api.register_tool(tool).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_register_duplicate_tool_rejected() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits::default()));

        let api1 = PluginApi::new(
            "plugin-a".to_string(),
            Arc::clone(&registry),
            event_bus.clone(),
            Arc::clone(&enforcer),
        );
        let api2 = PluginApi::new(
            "plugin-b".to_string(),
            Arc::clone(&registry),
            event_bus,
            enforcer,
        );

        // First registration succeeds
        let tool1 = Arc::new(TestTool::new("shared_tool"));
        assert!(api1.register_tool(tool1).await.is_ok());

        // Second registration with same name fails
        let tool2 = Arc::new(TestTool::new("shared_tool"));
        let result = api2.register_tool(tool2).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PluginError::NameConflict { .. }));
    }

    #[tokio::test]
    async fn test_register_event_handler() {
        let api = create_test_api("test-plugin");

        let callback: EventHandlerFn = Arc::new(|_payload| {
            // Handler logic
        });

        let result = api
            .register_event_handler("on_task_complete", EventType::TaskCompleted, callback)
            .await;
        assert!(result.is_ok());

        let handlers = api.event_handlers().await;
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].handler_name, "on_task_complete");
    }

    #[tokio::test]
    async fn test_duplicate_handler_rejected() {
        let api = create_test_api("test-plugin");

        let callback1: EventHandlerFn = Arc::new(|_| {});
        let callback2: EventHandlerFn = Arc::new(|_| {});

        assert!(api
            .register_event_handler("handler1", EventType::TaskCompleted, callback1)
            .await
            .is_ok());

        let result = api
            .register_event_handler("handler1", EventType::TaskFailed, callback2)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cleanup_removes_tools_and_handlers() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits::default()));

        let api = PluginApi::new(
            "cleanup-test".to_string(),
            Arc::clone(&registry),
            event_bus,
            enforcer,
        );

        // Register a tool and handler
        let tool = Arc::new(TestTool::new("cleanup_tool"));
        api.register_tool(tool).await.unwrap();

        let callback: EventHandlerFn = Arc::new(|_| {});
        api.register_event_handler("h1", EventType::PluginLoaded, callback)
            .await
            .unwrap();

        // Verify they exist
        {
            let reg = registry.read().await;
            assert!(reg.get("cleanup_tool").is_some());
        }
        assert_eq!(api.event_handlers().await.len(), 1);

        // Cleanup
        api.cleanup().await;

        // Verify they're gone
        {
            let reg = registry.read().await;
            assert!(reg.get("cleanup_tool").is_none());
        }
        assert_eq!(api.event_handlers().await.len(), 0);
    }

    #[tokio::test]
    async fn test_sandboxed_tool_execution() {
        let inner = Arc::new(TestTool::new("sandboxed"));
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        }));

        let sandboxed = SandboxedTool::new(inner, "test-plugin".to_string(), enforcer);

        let result = sandboxed.execute(json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "executed sandboxed");
    }

    #[test]
    fn test_plugin_name() {
        let api = create_test_api("my-plugin");
        assert_eq!(api.plugin_name(), "my-plugin");
    }

    // =========================================================================
    // Conflict Resolution Tests (Requirement 13.7)
    // =========================================================================

    #[tokio::test]
    async fn test_tool_conflict_first_registration_wins() {
        // Two plugins share the same tool registry
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Plugin A registers a tool
        let tool_a = Arc::new(TestTool::new("search"));
        assert!(api_a.register_tool(tool_a).await.is_ok());

        // Plugin B tries to register a tool with the same name — should be rejected
        let tool_b = Arc::new(TestTool::new("search"));
        let result = api_b.register_tool(tool_b).await;
        assert!(result.is_err());

        // Verify the error identifies both plugins
        match result.unwrap_err() {
            PluginError::NameConflict { name, existing_plugin } => {
                assert_eq!(name, "search");
                assert_eq!(existing_plugin, "plugin-alpha");
            }
            other => panic!("Expected NameConflict, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_tool_conflict_original_registration_intact() {
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Plugin A registers a tool
        let tool_a = Arc::new(TestTool::new("my_tool"));
        api_a.register_tool(tool_a).await.unwrap();

        // Plugin B's conflicting registration fails
        let tool_b = Arc::new(TestTool::new("my_tool"));
        let _ = api_b.register_tool(tool_b).await;

        // Original tool from plugin-alpha is still functional
        let registry = api_a.tool_registry.read().await;
        let tool = registry.get("my_tool").unwrap();
        let result = tool.execute(json!({})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "executed my_tool");

        // Confirm ownership is still plugin-alpha
        assert_eq!(registry.tool_plugin("my_tool"), Some("plugin-alpha"));
    }

    #[tokio::test]
    async fn test_handler_conflict_cross_plugin_rejected() {
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Plugin A registers a handler
        let callback_a: EventHandlerFn = Arc::new(|_| {});
        assert!(api_a
            .register_event_handler("on_file_change", EventType::TaskCompleted, callback_a)
            .await
            .is_ok());

        // Plugin B tries to register a handler with the same name — should be rejected
        let callback_b: EventHandlerFn = Arc::new(|_| {});
        let result = api_b
            .register_event_handler("on_file_change", EventType::TaskFailed, callback_b)
            .await;
        assert!(result.is_err());

        // Verify the error identifies both plugins
        match result.unwrap_err() {
            PluginError::NameConflict { name, existing_plugin } => {
                assert_eq!(name, "on_file_change");
                assert_eq!(existing_plugin, "plugin-alpha");
            }
            other => panic!("Expected NameConflict, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_handler_conflict_original_handler_intact() {
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Plugin A registers a handler
        let callback_a: EventHandlerFn = Arc::new(|_| {});
        api_a
            .register_event_handler("shared_handler", EventType::TaskCompleted, callback_a)
            .await
            .unwrap();

        // Plugin B's conflicting registration fails
        let callback_b: EventHandlerFn = Arc::new(|_| {});
        let _ = api_b
            .register_event_handler("shared_handler", EventType::TaskFailed, callback_b)
            .await;

        // Plugin A's handler is still registered
        let handlers = api_a.event_handlers().await;
        assert_eq!(handlers.len(), 1);
        assert_eq!(handlers[0].handler_name, "shared_handler");
        assert_eq!(handlers[0].plugin_name, "plugin-alpha");

        // Plugin B has no handlers
        let handlers_b = api_b.event_handlers().await;
        assert_eq!(handlers_b.len(), 0);
    }

    #[tokio::test]
    async fn test_different_names_no_conflict() {
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Both plugins register tools with different names — no conflict
        let tool_a = Arc::new(TestTool::new("tool_alpha"));
        let tool_b = Arc::new(TestTool::new("tool_beta"));
        assert!(api_a.register_tool(tool_a).await.is_ok());
        assert!(api_b.register_tool(tool_b).await.is_ok());

        // Both plugins register handlers with different names — no conflict
        let cb_a: EventHandlerFn = Arc::new(|_| {});
        let cb_b: EventHandlerFn = Arc::new(|_| {});
        assert!(api_a
            .register_event_handler("handler_alpha", EventType::TaskCompleted, cb_a)
            .await
            .is_ok());
        assert!(api_b
            .register_event_handler("handler_beta", EventType::TaskFailed, cb_b)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_tool_conflict_with_builtin() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits::default()));

        // Register a built-in tool directly in the registry
        {
            let mut reg = registry.write().await;
            reg.register(Arc::new(TestTool::new("builtin_search")));
        }

        let api = PluginApi::new(
            "my-plugin".to_string(),
            Arc::clone(&registry),
            event_bus,
            enforcer,
        );

        // Plugin tries to register a tool with the same name as a built-in
        let tool = Arc::new(TestTool::new("builtin_search"));
        let result = api.register_tool(tool).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            PluginError::NameConflict { name, existing_plugin } => {
                assert_eq!(name, "builtin_search");
                assert_eq!(existing_plugin, "built-in");
            }
            other => panic!("Expected NameConflict, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cleanup_frees_handler_names_for_reuse() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        let event_bus = EventBus::new(16);
        let enforcer = Arc::new(ResourceEnforcer::new(ResourceLimits::default()));
        let handler_registry = EventHandlerRegistry::new();

        let api_a = PluginApi::with_shared_handler_registry(
            "plugin-a".to_string(),
            Arc::clone(&registry),
            event_bus.clone(),
            Arc::clone(&enforcer),
            handler_registry.clone(),
        );
        let api_b = PluginApi::with_shared_handler_registry(
            "plugin-b".to_string(),
            Arc::clone(&registry),
            event_bus,
            enforcer,
            handler_registry,
        );

        // Plugin A registers a handler
        let cb: EventHandlerFn = Arc::new(|_| {});
        api_a
            .register_event_handler("reusable_handler", EventType::TaskCompleted, cb)
            .await
            .unwrap();

        // Plugin B can't use the same name
        let cb2: EventHandlerFn = Arc::new(|_| {});
        assert!(api_b
            .register_event_handler("reusable_handler", EventType::TaskFailed, cb2)
            .await
            .is_err());

        // Plugin A is unloaded (cleanup)
        api_a.cleanup().await;

        // Now plugin B can register the same handler name
        let cb3: EventHandlerFn = Arc::new(|_| {});
        assert!(api_b
            .register_event_handler("reusable_handler", EventType::TaskFailed, cb3)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_system_continues_after_conflict() {
        let (api_a, api_b) = create_shared_apis("plugin-alpha", "plugin-beta");

        // Plugin A registers tools and handlers
        let tool_a = Arc::new(TestTool::new("tool_1"));
        api_a.register_tool(tool_a).await.unwrap();
        let cb_a: EventHandlerFn = Arc::new(|_| {});
        api_a
            .register_event_handler("handler_1", EventType::TaskCompleted, cb_a)
            .await
            .unwrap();

        // Plugin B has conflicts on both
        let tool_b = Arc::new(TestTool::new("tool_1"));
        let _ = api_b.register_tool(tool_b).await; // fails
        let cb_b: EventHandlerFn = Arc::new(|_| {});
        let _ = api_b
            .register_event_handler("handler_1", EventType::TaskFailed, cb_b)
            .await; // fails

        // But plugin B can still register other tools and handlers
        let tool_b2 = Arc::new(TestTool::new("tool_2"));
        assert!(api_b.register_tool(tool_b2).await.is_ok());
        let cb_b2: EventHandlerFn = Arc::new(|_| {});
        assert!(api_b
            .register_event_handler("handler_2", EventType::TaskFailed, cb_b2)
            .await
            .is_ok());

        // Both plugins are operational
        let registry = api_a.tool_registry.read().await;
        assert_eq!(registry.tool_count(), 2); // tool_1 (alpha) + tool_2 (beta)
    }

    // =========================================================================
    // EventHandlerRegistry unit tests
    // =========================================================================

    #[tokio::test]
    async fn test_handler_registry_register_and_owner() {
        let registry = EventHandlerRegistry::new();

        assert!(registry.register("handler_a", "plugin-1").await.is_ok());
        assert_eq!(registry.owner("handler_a").await, Some("plugin-1".to_string()));
        assert_eq!(registry.owner("nonexistent").await, None);
    }

    #[tokio::test]
    async fn test_handler_registry_duplicate_rejected() {
        let registry = EventHandlerRegistry::new();

        assert!(registry.register("handler_x", "plugin-1").await.is_ok());
        let result = registry.register("handler_x", "plugin-2").await;
        assert_eq!(result, Err("plugin-1".to_string()));
    }

    #[tokio::test]
    async fn test_handler_registry_unregister() {
        let registry = EventHandlerRegistry::new();

        registry.register("handler_y", "plugin-1").await.unwrap();
        registry.unregister("handler_y").await;
        assert_eq!(registry.owner("handler_y").await, None);

        // Can re-register after unregister
        assert!(registry.register("handler_y", "plugin-2").await.is_ok());
        assert_eq!(registry.owner("handler_y").await, Some("plugin-2".to_string()));
    }

    #[tokio::test]
    async fn test_handler_registry_unregister_plugin() {
        let registry = EventHandlerRegistry::new();

        registry.register("h1", "plugin-1").await.unwrap();
        registry.register("h2", "plugin-1").await.unwrap();
        registry.register("h3", "plugin-2").await.unwrap();

        registry.unregister_plugin("plugin-1").await;

        assert_eq!(registry.owner("h1").await, None);
        assert_eq!(registry.owner("h2").await, None);
        assert_eq!(registry.owner("h3").await, Some("plugin-2".to_string()));
    }
}
