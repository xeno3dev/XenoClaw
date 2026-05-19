# Plugin System

XenoClaw's plugin system lets you extend the agent with custom tools and event handlers. Plugins are self-contained directories discovered at startup and optionally hot-reloaded on file changes.

Plugins can be written as Rust crates (native, compiled into the binary) or as WASM modules (compiled to `wasm32-wasi`, loaded at runtime via the `WasmRuntime` trait).

## Architecture

```
PluginManager
├── PluginLoader        — scans directory, reads manifests, loads plugins
├── PluginWatcher       — filesystem watcher for hot-reload (notify crate)
├── PluginApi           — sandboxed interface per plugin instance
│   ├── Tool Registration   → ToolRegistry
│   ├── Event Handlers      → EventBus
│   └── Resource Enforcement → SecurityLayer
└── WasmRuntime         — trait for WASM execution (StubWasmRuntime shipped)
    └── HostFunctions   — functions exposed to WASM modules (register_tool, log, etc.)
```

## Plugin Structure

```
plugins/
├── my-plugin/
│   ├── plugin.toml        # Manifest (required)
│   ├── plugin.wasm         # WASM entry point (optional — generated from src/)
│   └── src/
│       └── lib.rs          # Rust source
```

Each plugin lives in its own subdirectory under the configured plugins directory (`./plugins` by default).

## Manifest (`plugin.toml`)

```toml
[plugin]
name = "my-plugin"
version = "1.0.0"
api_version = "0.1.0"
description = "Does something useful"
author = "Your Name"

[permissions]
tools = ["my_tool", "my_other_tool"]
events = ["message_received", "task_completed"]
```

### `[plugin]` section

| Field         | Required | Description                                      |
|---------------|----------|--------------------------------------------------|
| `name`        | yes      | Unique identifier (alphanumeric, `-`, `_`)       |
| `version`     | yes      | Semantic version                                 |
| `api_version` | yes      | Platform API version this plugin targets         |
| `description` | no       | Human-readable summary                           |
| `author`      | no       | Plugin author                                    |

### `[permissions]` section

| Field    | Description                                      |
|----------|--------------------------------------------------|
| `tools`  | List of tool names the plugin will register      |
| `events` | List of event types the plugin will subscribe to |

## WASM Runtime

The `WasmRuntime` trait in `crates/plugin-system/src/wasm_runtime.rs` defines the interface for executing WASM plugins:

```rust
#[async_trait]
pub trait WasmRuntime: Send + Sync {
    async fn load_module(&self, name: &str, wasm_bytes: &[u8]) -> Result<WasmModuleHandle, WasmError>;
    async fn call_function(&self, module: &WasmModuleHandle, function: &str, args: &[WasmValue]) -> Result<Vec<WasmValue>, WasmError>;
    async fn unload_module(&self, module: WasmModuleHandle) -> Result<(), WasmError>;
    fn is_available(&self) -> bool { false }
}
```

| Method           | Description                                          |
|------------------|------------------------------------------------------|
| `load_module`    | Compile and instantiate a WASM module from bytes     |
| `call_function`  | Call an exported function on a loaded module         |
| `unload_module`  | Free all resources associated with a module          |
| `is_available`   | Whether the runtime is functional (not a stub)       |

### Current State: Stub Runtime

`StubWasmRuntime` is the shipped implementation. It returns `WasmError::NotAvailable` for all operations. To enable real WASM execution:

1. Add `wasmtime` (or another engine) as a dependency behind a feature flag.
2. Implement the `WasmRuntime` trait.
3. Wire it into `PluginManager`.

### Host Functions

WASM plugins interact with XenoClaw through host functions linked at instantiation:

| Function                 | Signature                                      | Description                              |
|--------------------------|------------------------------------------------|------------------------------------------|
| `register_tool`          | `(name, description, schema) -> result`        | Register a tool with the ToolRegistry    |
| `register_event_handler` | `(event_type, handler_id) -> result`           | Subscribe to platform events             |
| `log`                    | `(level, message) -> ()`                       | Write to the agent's structured log      |
| `memory_read`            | `(key) -> value`                               | Read from the plugin's isolated KV store |
| `memory_write`           | `(key, value) -> result`                       | Write to the plugin's isolated KV store  |

Each plugin gets its own `HostFunctions` instance scoped to its name. All operations are subject to the Security Layer's resource limits and conflict resolution.

## Plugin Lifecycle

```
Discovery ──→ Load ──→ Active ──→ Unload
                 │                    │
                 └──→ Reload ←────────┘
```

1. **Discovery** — `PluginLoader.load_all()` scans the plugins directory for subdirectories containing `plugin.toml`.
2. **Load** — Manifest is validated, `PluginApi` is created (and WASM module loaded if using `WasmRuntime`), `PluginLoaded` event is emitted.
3. **Active** — Plugin's tools and event handlers are available to the agent.
4. **Hot-Reload** — `PluginWatcher` detects file changes (debounced at 2s), unloads old version (30s graceful timeout), loads new version.
5. **Unload** — All tools, event handlers, and WASM modules are cleaned up, `PluginUnloaded` event emitted.

## PluginApi

Each loaded plugin receives its own `PluginApi` instance scoped to its name.

### Registering Tools

```rust
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};
use agent_core::tool_registry::Tool;
use plugin_system::PluginApi;

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "What it does" }
    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": { ... }, "required": [...] })
    }
    async fn execute(&self, arguments: Value) -> Result<String, String> {
        // tool logic
        Ok("done".into())
    }
}

// In your init function:
api.register_tool(Arc::new(MyTool)).await?;
```

### Registering Event Handlers

```rust
use agent_core::event_bus::EventType;
use plugin_system::EventHandlerFn;

let handler: EventHandlerFn = Arc::new(|payload| {
    tracing::info!("Got event: {:?}", payload);
});

api.register_event_handler("my_handler_name", EventType::TaskCompleted, handler).await?;
```

Available event types:
- `ToolRegistered` / `ToolUnregistered`
- `TaskCompleted` / `TaskFailed`
- `MessageReceived`
- `AgentModeChanged`
- `PluginLoaded` / `PluginUnloaded`

### Logging

```rust
api.log(tracing::Level::INFO, "Something happened");
```

## Conflict Resolution

First-registration-wins semantics apply to both tools and event handlers:

- If a tool name is already registered (by any plugin or built-in), the new registration is rejected with `PluginError::NameConflict`.
- If an event handler name is already registered (by any plugin), the new registration is rejected with `PluginError::NameConflict`.
- The original registration remains intact. The system continues operating normally.

## Hot-Reload

The `PluginWatcher` monitors the plugins directory using the `notify` crate:

- File changes are debounced for 2 seconds to batch rapid writes.
- In-flight operations get up to 30 seconds to complete before the old version is replaced.
- New plugins appearing in the directory are loaded automatically.
- Plugin directory removal triggers unload.

## Configuration

In `config.toml`:

```toml
[plugins]
enabled = true
directory = "./plugins"
reload_interval_seconds = 10
```

## Creating a Plugin (Quick Start)

### Native (Rust crate)

1. **Create the directory**:
   ```sh
   mkdir -p plugins/my-plugin/src
   ```

2. **Write `plugins/my-plugin/plugin.toml`**:
   ```toml
   [plugin]
   name = "my-plugin"
   version = "0.1.0"
   api_version = "0.1.0"
   description = "My first plugin"

   [permissions]
   tools = []
   events = []
   ```

3. **Write `plugins/my-plugin/src/lib.rs`** with an `init` function that registers tools via `PluginApi` (see `plugins/example-plugin/src/lib.rs`).

4. **Add your crate to the workspace** in `Cargo.toml` and wire `init` into the loader (see `crates/plugin-system/src/loader.rs`).

5. **Run XenoClaw** — the plugin is discovered and loaded at startup.

### WASM (future — once Wasmtime is integrated)

1. Follow steps 1–2 above.
2. Write your WASM plugin in Rust targeting `wasm32-wasi`:
   ```sh
   rustup target add wasm32-wasi
   cargo build --target wasm32-wasi --release
   cp target/wasm32-wasi/release/my_plugin.wasm plugins/my-plugin/plugin.wasm
   ```
3. The plugin calls host functions (`register_tool`, `log`, etc.) at runtime instead of using `PluginApi` directly.

## Reference

| Source File | Description |
|---|---|---|
| `crates/plugin-system/src/manifest.rs` | Manifest parsing & validation |
| `crates/plugin-system/src/loader.rs` | Plugin discovery and loading |
| `crates/plugin-system/src/api.rs` | PluginApi — tool & event handler registration |
| `crates/plugin-system/src/manager.rs` | PluginManager — top-level coordinator |
| `crates/plugin-system/src/watcher.rs` | Filesystem watcher & hot-reload |
| `crates/plugin-system/src/wasm_runtime.rs` | WasmRuntime trait, WasmValue, host functions, StubWasmRuntime |
| `crates/agent-core/src/tool_registry.rs` | Central tool registry |
| `crates/agent-core/src/event_bus.rs` | Pub/sub event bus |
| `crates/security-layer/src/lib.rs` | Resource enforcement & sandboxing |
| `plugins/example-plugin/` | Working template plugin |
