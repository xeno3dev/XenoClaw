# Hello World Plugin

A minimal example plugin demonstrating the XenoClaw WASM plugin API.

## What It Does

This plugin registers a single tool (`hello_greet`) that returns a greeting
message. It also subscribes to `message_received` events to demonstrate the
event handler API.

## Plugin Manifest

The `plugin.toml` declares the plugin's metadata and permissions:

```toml
[plugin]
name = "hello-world"
version = "0.1.0"
api_version = "0.1.0"
description = "Example plugin that registers a simple greeting tool"
author = "XenoClaw"

[permissions]
tools = ["hello_greet"]
events = ["message_received"]
```

## Building (once Wasmtime is integrated)

```sh
# Install the WASM target
rustup target add wasm32-wasi

# Build the plugin as a WASM module
cargo build --target wasm32-wasi -p hello-world-plugin --release

# Copy the compiled module into the plugin directory
cp target/wasm32-wasi/release/hello_world_plugin.wasm plugins/example-plugin/plugin.wasm
```

## Plugin API Available to WASM Modules

WASM plugins interact with XenoClaw through host functions linked at instantiation:

| Host Function | Signature | Description |
|---------------|-----------|-------------|
| `register_tool` | `(name, description, schema) -> result` | Register a tool with the ToolRegistry |
| `register_event_handler` | `(event_type, handler_id) -> result` | Subscribe to platform events |
| `log` | `(level, message) -> ()` | Write to the agent's structured log |
| `memory_read` | `(key) -> value` | Read from the plugin's isolated KV store |
| `memory_write` | `(key, value) -> result` | Write to the plugin's isolated KV store |

### Lifecycle

1. The platform loads `plugin.wasm` and links host functions.
2. The platform calls the `_start` or `init` export to initialize the plugin.
3. The plugin calls `register_tool` / `register_event_handler` during init.
4. When a registered tool is invoked or event fires, the platform calls the
   corresponding exported function.
5. On unload (or hot-reload), the platform calls `shutdown` if exported, then
   frees all resources.

### Security

- All operations are subject to the Security Layer's resource limits.
- Tool names follow first-registration-wins conflict resolution.
- Memory access is namespaced — plugins cannot read other plugins' data.
- File and network access require explicit permissions in `plugin.toml`.
