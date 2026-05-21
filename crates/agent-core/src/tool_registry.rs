//! Tool Registry — manages available tools and their execution.
//!
//! The Tool Registry is responsible for:
//! - Registering tools (both built-in and plugin-provided)
//! - Looking up tools by name
//! - Executing tool calls and returning results
//! - Supporting mode-based tool availability (General vs Coding)
//! - Tracking which plugin registered each tool (for conflict resolution and unloading)
//! - Emitting ToolRegistered/ToolUnregistered events on the event bus

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use common::models::ToolResult;
use llm_router::ToolDefinition;

use crate::event_bus::{Event, EventBus, EventType};

/// A trait that all tool implementations must satisfy.
///
/// Tools are async and receive their arguments as a JSON value.
/// They return a string output (success) or an error string.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The unique name of this tool.
    fn name(&self) -> &str;

    /// A human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// The JSON Schema describing the tool's parameters.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given arguments.
    ///
    /// Returns `Ok(output)` on success or `Err(error_message)` on failure.
    async fn execute(&self, arguments: Value) -> Result<String, String>;

    /// Whether this tool is only available in Coding mode.
    fn coding_only(&self) -> bool {
        false
    }
}

/// Metadata about a registered tool, including its source (built-in or plugin).
#[derive(Clone)]
struct ToolEntry {
    /// The tool implementation.
    tool: Arc<dyn Tool>,
    /// The plugin that registered this tool, or None for built-in tools.
    plugin_name: Option<String>,
}

impl std::fmt::Debug for ToolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolEntry")
            .field("tool_name", &self.tool.name())
            .field("plugin_name", &self.plugin_name)
            .finish()
    }
}

/// The Tool Registry manages all registered tools and provides lookup/execution.
///
/// Supports both built-in tools and plugin-provided tools. Plugin tools are tracked
/// by their source plugin name, enabling bulk unregistration when a plugin is unloaded.
pub struct ToolRegistry {
    /// All registered tools, keyed by name.
    tools: HashMap<String, ToolEntry>,
    /// Index of tools registered by each plugin (plugin_name -> set of tool names).
    plugin_tools: HashMap<String, HashSet<String>>,
    /// Optional event bus for emitting tool lifecycle events.
    event_bus: Option<EventBus>,
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            plugin_tools: HashMap::new(),
            event_bus: None,
        }
    }

    /// Create a new tool registry with an event bus for lifecycle notifications.
    pub fn with_event_bus(event_bus: EventBus) -> Self {
        Self {
            tools: HashMap::new(),
            plugin_tools: HashMap::new(),
            event_bus: Some(event_bus),
        }
    }

    /// Set or replace the event bus.
    pub fn set_event_bus(&mut self, event_bus: EventBus) {
        self.event_bus = Some(event_bus);
    }

    /// Get a reference to the event bus, if one is configured.
    pub fn event_bus(&self) -> Option<&EventBus> {
        self.event_bus.as_ref()
    }

    /// Register a built-in tool. Returns `false` if a tool with the same name already exists.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> bool {
        let name = tool.name().to_string();
        if self.tools.contains_key(&name) {
            warn!(
                tool_name = %name,
                "Tool registration rejected: name already registered"
            );
            return false;
        }
        info!(tool_name = %name, "Tool registered");
        self.tools.insert(
            name.clone(),
            ToolEntry {
                tool,
                plugin_name: None,
            },
        );

        self.emit_tool_registered(&name, None);
        true
    }

    /// Register a tool provided by a plugin.
    ///
    /// Tracks the plugin name for later bulk unregistration. Returns `false` if
    /// a tool with the same name already exists (first registration wins per Req 13.7).
    pub fn register_plugin_tool(&mut self, plugin_name: &str, tool: Arc<dyn Tool>) -> bool {
        let tool_name = tool.name().to_string();

        if let Some(existing) = self.tools.get(&tool_name) {
            let existing_source = existing.plugin_name.as_deref().unwrap_or("built-in");
            warn!(
                tool_name = %tool_name,
                new_plugin = %plugin_name,
                existing_source = %existing_source,
                "Plugin tool registration rejected: name already registered"
            );
            return false;
        }

        info!(
            tool_name = %tool_name,
            plugin = %plugin_name,
            "Plugin tool registered"
        );

        self.tools.insert(
            tool_name.clone(),
            ToolEntry {
                tool,
                plugin_name: Some(plugin_name.to_string()),
            },
        );

        // Track in the plugin index
        self.plugin_tools
            .entry(plugin_name.to_string())
            .or_default()
            .insert(tool_name.clone());

        self.emit_tool_registered(&tool_name, Some(plugin_name));
        true
    }

    /// Unregister a tool by name. Returns `true` if the tool was found and removed.
    pub fn unregister(&mut self, name: &str) -> bool {
        if let Some(entry) = self.tools.remove(name) {
            // Remove from plugin index if it was a plugin tool
            if let Some(ref plugin_name) = entry.plugin_name {
                if let Some(tools) = self.plugin_tools.get_mut(plugin_name) {
                    tools.remove(name);
                    if tools.is_empty() {
                        self.plugin_tools.remove(plugin_name);
                    }
                }
            }

            info!(
                tool_name = %name,
                plugin = ?entry.plugin_name,
                "Tool unregistered"
            );

            self.emit_tool_unregistered(name, entry.plugin_name.as_deref());
            true
        } else {
            false
        }
    }

    /// Unregister all tools provided by a specific plugin.
    ///
    /// Returns the names of all tools that were removed.
    pub fn unregister_plugin_tools(&mut self, plugin_name: &str) -> Vec<String> {
        let tool_names = match self.plugin_tools.remove(plugin_name) {
            Some(names) => names,
            None => {
                debug!(
                    plugin = %plugin_name,
                    "No tools found for plugin"
                );
                return Vec::new();
            }
        };

        let mut removed = Vec::new();
        for name in &tool_names {
            if self.tools.remove(name).is_some() {
                info!(
                    tool_name = %name,
                    plugin = %plugin_name,
                    "Plugin tool unregistered (plugin unloading)"
                );
                self.emit_tool_unregistered(name, Some(plugin_name));
                removed.push(name.clone());
            }
        }

        info!(
            plugin = %plugin_name,
            tool_count = removed.len(),
            "All plugin tools unregistered"
        );

        removed
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name).map(|entry| &entry.tool)
    }

    /// Get the plugin name that registered a tool, if any.
    pub fn tool_plugin(&self, tool_name: &str) -> Option<&str> {
        self.tools
            .get(tool_name)
            .and_then(|entry| entry.plugin_name.as_deref())
    }

    /// Get all tool names registered by a specific plugin.
    pub fn tools_for_plugin(&self, plugin_name: &str) -> Vec<String> {
        self.plugin_tools
            .get(plugin_name)
            .map(|names| names.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all plugin names that have registered tools.
    pub fn registered_plugins(&self) -> Vec<String> {
        self.plugin_tools.keys().cloned().collect()
    }

    /// Get all tool definitions suitable for sending to the LLM.
    ///
    /// If `include_coding` is false, coding-only tools are excluded.
    pub fn tool_definitions(&self, include_coding: bool) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .filter(|entry| include_coding || !entry.tool.coding_only())
            .map(|entry| ToolDefinition {
                name: entry.tool.name().to_string(),
                description: entry.tool.description().to_string(),
                parameters: entry.tool.parameters_schema(),
            })
            .collect()
    }

    /// Execute a tool call by name with the given arguments.
    ///
    /// Returns a `ToolResult` with the output or error.
    pub async fn execute_tool_call(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> ToolResult {
        debug!(
            tool_name = %tool_name,
            tool_call_id = %tool_call_id,
            "Executing tool call"
        );

        match self.tools.get(tool_name) {
            Some(entry) => match entry.tool.execute(arguments).await {
                Ok(output) => {
                    debug!(
                        tool_name = %tool_name,
                        tool_call_id = %tool_call_id,
                        output_len = output.len(),
                        "Tool execution succeeded"
                    );
                    ToolResult {
                        tool_call_id: tool_call_id.to_string(),
                        output,
                        is_error: false,
                    }
                }
                Err(err) => {
                    error!(
                        tool_name = %tool_name,
                        tool_call_id = %tool_call_id,
                        error = %err,
                        "Tool execution failed"
                    );
                    ToolResult {
                        tool_call_id: tool_call_id.to_string(),
                        output: err,
                        is_error: true,
                    }
                }
            },
            None => {
                warn!(
                    tool_name = %tool_name,
                    tool_call_id = %tool_call_id,
                    "Tool not found in registry"
                );
                ToolResult {
                    tool_call_id: tool_call_id.to_string(),
                    output: format!("Tool '{}' not found", tool_name),
                    is_error: true,
                }
            }
        }
    }

    /// Returns the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Returns the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Emit a ToolRegistered event on the event bus.
    fn emit_tool_registered(&self, tool_name: &str, plugin_name: Option<&str>) {
        if let Some(ref bus) = self.event_bus {
            let payload = serde_json::json!({
                "tool_name": tool_name,
                "plugin": plugin_name,
            });
            bus.publish(Event::new(EventType::ToolRegistered, payload));
        }
    }

    /// Emit a ToolUnregistered event on the event bus.
    fn emit_tool_unregistered(&self, tool_name: &str, plugin_name: Option<&str>) {
        if let Some(ref bus) = self.event_bus {
            let payload = serde_json::json!({
                "tool_name": tool_name,
                "plugin": plugin_name,
            });
            bus.publish(Event::new(EventType::ToolUnregistered, payload));
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A simple test tool that echoes its input.
    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echoes the input back"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }

        async fn execute(&self, arguments: Value) -> Result<String, String> {
            let msg = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("no message");
            Ok(msg.to_string())
        }
    }

    /// A tool that always fails.
    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }

        fn description(&self) -> &str {
            "Always fails"
        }

        fn parameters_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn execute(&self, _arguments: Value) -> Result<String, String> {
            Err("intentional failure".to_string())
        }
    }

    /// A coding-only tool.
    struct CodingTool;

    #[async_trait]
    impl Tool for CodingTool {
        fn name(&self) -> &str {
            "file_read"
        }

        fn description(&self) -> &str {
            "Read a file from the workspace"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            })
        }

        async fn execute(&self, arguments: Value) -> Result<String, String> {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(format!("Contents of {}", path))
        }

        fn coding_only(&self) -> bool {
            true
        }
    }

    /// A named tool for plugin testing.
    struct NamedTool {
        tool_name: String,
    }

    impl NamedTool {
        fn new(name: &str) -> Self {
            Self {
                tool_name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "A named test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({ "type": "object" })
        }

        async fn execute(&self, _arguments: Value) -> Result<String, String> {
            Ok(format!("executed {}", self.tool_name))
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(EchoTool);

        assert!(registry.register(tool));
        assert_eq!(registry.tool_count(), 1);
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let mut registry = ToolRegistry::new();
        let tool1 = Arc::new(EchoTool);
        let tool2 = Arc::new(EchoTool);

        assert!(registry.register(tool1));
        assert!(!registry.register(tool2));
        assert_eq!(registry.tool_count(), 1);
    }

    #[test]
    fn test_unregister() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        assert!(registry.unregister("echo"));
        assert_eq!(registry.tool_count(), 0);
        assert!(!registry.unregister("echo")); // already removed
    }

    #[test]
    fn test_tool_definitions_excludes_coding_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(CodingTool));

        let general_defs = registry.tool_definitions(false);
        assert_eq!(general_defs.len(), 1);
        assert_eq!(general_defs[0].name, "echo");

        let all_defs = registry.tool_definitions(true);
        assert_eq!(all_defs.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_tool_success() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry
            .execute_tool_call("call_1", "echo", json!({"message": "hello"}))
            .await;

        assert_eq!(result.tool_call_id, "call_1");
        assert_eq!(result.output, "hello");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_tool_failure() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FailTool));

        let result = registry
            .execute_tool_call("call_2", "fail", json!({}))
            .await;

        assert_eq!(result.tool_call_id, "call_2");
        assert_eq!(result.output, "intentional failure");
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_execute_nonexistent_tool() {
        let registry = ToolRegistry::new();

        let result = registry
            .execute_tool_call("call_3", "nonexistent", json!({}))
            .await;

        assert_eq!(result.tool_call_id, "call_3");
        assert!(result.output.contains("not found"));
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_names() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailTool));

        let mut names = registry.tool_names();
        names.sort();
        assert_eq!(names, vec!["echo", "fail"]);
    }

    // --- Plugin tool registration tests ---

    #[test]
    fn test_register_plugin_tool() {
        let mut registry = ToolRegistry::new();
        let tool = Arc::new(NamedTool::new("plugin_tool_1"));

        assert!(registry.register_plugin_tool("my-plugin", tool));
        assert_eq!(registry.tool_count(), 1);
        assert!(registry.get("plugin_tool_1").is_some());
        assert_eq!(registry.tool_plugin("plugin_tool_1"), Some("my-plugin"));
    }

    #[test]
    fn test_plugin_tool_conflict_with_builtin() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        // Plugin tries to register a tool with the same name as a built-in
        let plugin_echo = Arc::new(NamedTool::new("echo"));
        assert!(!registry.register_plugin_tool("my-plugin", plugin_echo));
        assert_eq!(registry.tool_count(), 1);
        // Original built-in is still there
        assert_eq!(registry.tool_plugin("echo"), None);
    }

    #[test]
    fn test_plugin_tool_conflict_between_plugins() {
        let mut registry = ToolRegistry::new();

        let tool1 = Arc::new(NamedTool::new("shared_tool"));
        let tool2 = Arc::new(NamedTool::new("shared_tool"));

        assert!(registry.register_plugin_tool("plugin-a", tool1));
        assert!(!registry.register_plugin_tool("plugin-b", tool2));

        // First registration wins
        assert_eq!(registry.tool_plugin("shared_tool"), Some("plugin-a"));
    }

    #[test]
    fn test_unregister_plugin_tools() {
        let mut registry = ToolRegistry::new();

        // Register multiple tools from one plugin
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("tool_a")));
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("tool_b")));
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("tool_c")));

        // Register a tool from another plugin
        registry.register_plugin_tool("other-plugin", Arc::new(NamedTool::new("tool_d")));

        // Register a built-in tool
        registry.register(Arc::new(EchoTool));

        assert_eq!(registry.tool_count(), 5);

        // Unregister all tools from my-plugin
        let removed = registry.unregister_plugin_tools("my-plugin");
        assert_eq!(removed.len(), 3);
        assert!(removed.contains(&"tool_a".to_string()));
        assert!(removed.contains(&"tool_b".to_string()));
        assert!(removed.contains(&"tool_c".to_string()));

        // Other tools remain
        assert_eq!(registry.tool_count(), 2);
        assert!(registry.get("tool_d").is_some());
        assert!(registry.get("echo").is_some());
    }

    #[test]
    fn test_unregister_plugin_tools_nonexistent_plugin() {
        let mut registry = ToolRegistry::new();
        let removed = registry.unregister_plugin_tools("nonexistent");
        assert!(removed.is_empty());
    }

    #[test]
    fn test_tools_for_plugin() {
        let mut registry = ToolRegistry::new();
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("tool_x")));
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("tool_y")));

        let mut tools = registry.tools_for_plugin("my-plugin");
        tools.sort();
        assert_eq!(tools, vec!["tool_x", "tool_y"]);

        assert!(registry.tools_for_plugin("other").is_empty());
    }

    #[test]
    fn test_registered_plugins() {
        let mut registry = ToolRegistry::new();
        registry.register_plugin_tool("alpha", Arc::new(NamedTool::new("t1")));
        registry.register_plugin_tool("beta", Arc::new(NamedTool::new("t2")));

        let mut plugins = registry.registered_plugins();
        plugins.sort();
        assert_eq!(plugins, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_builtin_tool_has_no_plugin() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert_eq!(registry.tool_plugin("echo"), None);
    }

    #[tokio::test]
    async fn test_event_bus_emits_on_register() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolRegistered);

        let mut registry = ToolRegistry::with_event_bus(bus);
        registry.register(Arc::new(EchoTool));

        let event = rx.recv().await.unwrap();
        assert_eq!(event.event_type, EventType::ToolRegistered);
        assert_eq!(event.payload["tool_name"], "echo");
        assert_eq!(event.payload["plugin"], Value::Null);
    }

    #[tokio::test]
    async fn test_event_bus_emits_on_plugin_register() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolRegistered);

        let mut registry = ToolRegistry::with_event_bus(bus);
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("custom_tool")));

        let event = rx.recv().await.unwrap();
        assert_eq!(event.event_type, EventType::ToolRegistered);
        assert_eq!(event.payload["tool_name"], "custom_tool");
        assert_eq!(event.payload["plugin"], "my-plugin");
    }

    #[tokio::test]
    async fn test_event_bus_emits_on_unregister() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolUnregistered);

        let mut registry = ToolRegistry::with_event_bus(bus);
        registry.register(Arc::new(EchoTool));
        registry.unregister("echo");

        let event = rx.recv().await.unwrap();
        assert_eq!(event.event_type, EventType::ToolUnregistered);
        assert_eq!(event.payload["tool_name"], "echo");
    }

    #[tokio::test]
    async fn test_event_bus_emits_on_plugin_unload() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe(EventType::ToolUnregistered);

        let mut registry = ToolRegistry::with_event_bus(bus);
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("t1")));
        registry.register_plugin_tool("my-plugin", Arc::new(NamedTool::new("t2")));

        registry.unregister_plugin_tools("my-plugin");

        // Should receive two unregister events
        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();

        let mut names: Vec<String> = vec![
            e1.payload["tool_name"].as_str().unwrap().to_string(),
            e2.payload["tool_name"].as_str().unwrap().to_string(),
        ];
        names.sort();
        assert_eq!(names, vec!["t1", "t2"]);
    }

    #[test]
    fn test_no_event_bus_does_not_panic() {
        // Registry without event bus should work fine
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.unregister("echo");
        // No panic — events are simply not emitted
    }
}
