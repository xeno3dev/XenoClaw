//! Hello World Plugin — template for XenoClaw plugin authors.
//!
//! This demonstrates how to register tools and event handlers
//! using the PluginApi. For WASM targets the same init logic is
//! called by the WASM runtime (see plugin_system::wasm_runtime).
//!
//! ## Two approaches
//!
//! - **Native (lib)** — add this crate to the workspace, wire `init`
//!   into PluginLoader for direct Rust integration.
//! - **WASM (cdylib)** — compile to `wasm32-wasi`, place the `.wasm`
//!   in the plugin directory as `plugin.wasm`. The WasmRuntime trait
//!   (stub currently) handles instantiation and host function linking.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use agent_core::event_bus::EventType;
use agent_core::tool_registry::Tool;
use plugin_system::PluginApi;

// ── Tool Definitions ──────────────────────────────────────────────────

struct GreetTool;

#[async_trait]
impl Tool for GreetTool {
    fn name(&self) -> &str {
        "hello_greet"
    }

    fn description(&self) -> &str {
        "Returns a friendly greeting for the given name"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name to greet"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<String, String> {
        let name = arguments
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("world");
        Ok(format!("Hello, {}!", name))
    }
}

// ── Plugin Initialisation ─────────────────────────────────────────────

/// Initialise the plugin — called by PluginLoader (native) or WASM runtime.
///
/// Registers tools and event handlers through the sandboxed PluginApi.
pub async fn init(api: &PluginApi) -> Result<(), Box<dyn std::error::Error>> {
    info!(plugin = "hello-world", "Initialising plugin");

    api.register_tool(Arc::new(GreetTool)).await?;

    let on_message: plugin_system::EventHandlerFn = Arc::new(|payload| {
        info!(plugin = "hello-world", event = ?payload, "Received event");
    });

    api.register_event_handler("hello_on_message", EventType::MessageReceived, on_message)
        .await?;

    info!(plugin = "hello-world", "Plugin initialised successfully");
    Ok(())
}
