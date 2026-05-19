//! WASM Plugin Runtime — execution environment for WebAssembly plugins.
//!
//! This module defines the trait-based abstraction for running WASM plugins.
//! The architecture separates the runtime interface from the implementation,
//! allowing different WASM engines (Wasmtime, Wasmer, etc.) to be plugged in.
//!
//! # Current State
//!
//! The module ships with a [`StubWasmRuntime`] that returns [`WasmError::NotAvailable`]
//! for all operations. Once the `wasmtime` feature is enabled and the dependency added,
//! a real implementation can be provided by implementing [`WasmRuntime`].
//!
//! # Host Functions
//!
//! WASM plugins interact with the platform through [`HostFunctions`], which expose:
//! - Tool registration
//! - Event handler registration
//! - Structured logging
//! - Plugin-scoped key-value storage
//!
//! # Example
//!
//! ```rust,no_run
//! use plugin_system::wasm_runtime::{StubWasmRuntime, WasmRuntime, WasmValue};
//!
//! # async fn example() {
//! let runtime = StubWasmRuntime;
//! let result = runtime.load_module("my-plugin", &[]).await;
//! assert!(result.is_err()); // Stub always returns NotAvailable
//! # }
//! ```

use async_trait::async_trait;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Handle to a loaded WASM module.
///
/// Returned by [`WasmRuntime::load_module`] and used to reference the module
/// in subsequent calls (function invocation, unloading).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WasmModuleHandle {
    /// Unique identifier for this module instance.
    pub id: String,
    /// Human-readable name (typically the plugin name).
    pub name: String,
}

/// Values that can be passed to and returned from WASM functions.
///
/// This covers the core WASM value types plus higher-level types that the
/// host functions use (strings and byte buffers are passed through linear memory).
#[derive(Debug, Clone, PartialEq)]
pub enum WasmValue {
    /// 32-bit signed integer.
    I32(i32),
    /// 64-bit signed integer.
    I64(i64),
    /// 32-bit floating point.
    F32(f32),
    /// 64-bit floating point.
    F64(f64),
    /// UTF-8 string (serialized through linear memory in actual WASM).
    String(String),
    /// Raw byte buffer (serialized through linear memory in actual WASM).
    Bytes(Vec<u8>),
}

impl WasmValue {
    /// Returns `true` if this value is a numeric WASM type (i32, i64, f32, f64).
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            WasmValue::I32(_) | WasmValue::I64(_) | WasmValue::F32(_) | WasmValue::F64(_)
        )
    }

    /// Try to extract an i32 value.
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            WasmValue::I32(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract an i64 value.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            WasmValue::I64(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to extract a string reference.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            WasmValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Try to extract a byte slice reference.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            WasmValue::Bytes(b) => Some(b.as_slice()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors from the WASM runtime.
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    /// The WASM runtime is not available (feature not enabled or engine missing).
    #[error("WASM runtime not available: {0}")]
    NotAvailable(String),

    /// Failed to compile or instantiate a WASM module.
    #[error("Failed to load module '{name}': {reason}")]
    LoadFailed {
        /// Name of the module that failed to load.
        name: String,
        /// Reason for the failure.
        reason: String,
    },

    /// The requested function was not found in the module's exports.
    #[error("Function '{function}' not found in module '{module}'")]
    FunctionNotFound {
        /// Module that was searched.
        module: String,
        /// Function name that was not found.
        function: String,
    },

    /// An error occurred during WASM function execution (trap, OOM, etc.).
    #[error("Execution error in module '{module}': {reason}")]
    ExecutionError {
        /// Module where the error occurred.
        module: String,
        /// Description of the error.
        reason: String,
    },

    /// The referenced module is not currently loaded.
    #[error("Module '{name}' not loaded")]
    ModuleNotLoaded {
        /// Name of the module that was expected to be loaded.
        name: String,
    },

    /// A type mismatch between expected and actual WASM values.
    #[error("Type mismatch in module '{module}': expected {expected}, got {actual}")]
    TypeMismatch {
        /// Module where the mismatch occurred.
        module: String,
        /// Expected type description.
        expected: String,
        /// Actual type description.
        actual: String,
    },
}

// ---------------------------------------------------------------------------
// Runtime trait
// ---------------------------------------------------------------------------

/// Trait abstracting the WASM plugin execution environment.
///
/// Implementations provide the actual WASM engine (e.g., Wasmtime, Wasmer).
/// The default implementation ([`StubWasmRuntime`]) is a no-op stub that returns
/// [`WasmError::NotAvailable`] for all operations.
///
/// # Implementors
///
/// To add a real WASM engine:
/// 1. Add the engine crate (e.g., `wasmtime`) as a dependency behind a feature flag.
/// 2. Create a struct implementing this trait.
/// 3. Wire it into [`crate::PluginManager`] based on the feature flag.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow concurrent plugin execution
/// across the async runtime.
#[async_trait]
pub trait WasmRuntime: Send + Sync {
    /// Load a WASM module from raw bytes.
    ///
    /// The runtime should compile and instantiate the module, linking any
    /// host functions defined in [`HostFunctions`].
    ///
    /// # Arguments
    /// * `name` — Human-readable name for the module (used in error messages).
    /// * `wasm_bytes` — The raw `.wasm` binary content.
    ///
    /// # Returns
    /// A [`WasmModuleHandle`] that can be used to call functions or unload the module.
    async fn load_module(
        &self,
        name: &str,
        wasm_bytes: &[u8],
    ) -> Result<WasmModuleHandle, WasmError>;

    /// Call a function exported by a loaded WASM module.
    ///
    /// # Arguments
    /// * `module` — Handle to the loaded module.
    /// * `function` — Name of the exported function to call.
    /// * `args` — Arguments to pass to the function.
    ///
    /// # Returns
    /// A vector of return values from the function.
    async fn call_function(
        &self,
        module: &WasmModuleHandle,
        function: &str,
        args: &[WasmValue],
    ) -> Result<Vec<WasmValue>, WasmError>;

    /// Unload a module, freeing all associated resources.
    ///
    /// After this call, the handle is invalid and must not be reused.
    async fn unload_module(&self, module: WasmModuleHandle) -> Result<(), WasmError>;

    /// Check whether this runtime is functional (i.e., not a stub).
    ///
    /// Returns `true` if the runtime can actually execute WASM modules.
    /// The default implementation returns `false`.
    fn is_available(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Stub implementation
// ---------------------------------------------------------------------------

/// Stub WASM runtime that returns [`WasmError::NotAvailable`] for all operations.
///
/// Used when the `wasmtime` feature is not enabled. This allows the rest of the
/// plugin system to compile and function (loading manifests, watching for changes)
/// without pulling in the heavy WASM engine dependency.
///
/// When a plugin attempts to execute, the stub logs a warning and returns an error,
/// making it clear that the WASM runtime needs to be enabled for full functionality.
pub struct StubWasmRuntime;

impl StubWasmRuntime {
    /// Create a new stub runtime instance.
    pub fn new() -> Self {
        Self
    }

    fn not_available_error() -> WasmError {
        WasmError::NotAvailable(
            "wasmtime feature not enabled. Add wasmtime to Cargo.toml to enable WASM plugin execution.".to_string(),
        )
    }
}

impl Default for StubWasmRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WasmRuntime for StubWasmRuntime {
    async fn load_module(
        &self,
        name: &str,
        _wasm_bytes: &[u8],
    ) -> Result<WasmModuleHandle, WasmError> {
        tracing::warn!(
            plugin = %name,
            "WASM runtime not available — wasmtime feature not enabled"
        );
        Err(Self::not_available_error())
    }

    async fn call_function(
        &self,
        module: &WasmModuleHandle,
        function: &str,
        _args: &[WasmValue],
    ) -> Result<Vec<WasmValue>, WasmError> {
        tracing::warn!(
            module = %module.name,
            function = %function,
            "Cannot call WASM function — runtime not available"
        );
        Err(Self::not_available_error())
    }

    async fn unload_module(&self, module: WasmModuleHandle) -> Result<(), WasmError> {
        tracing::warn!(
            module = %module.name,
            "Cannot unload WASM module — runtime not available"
        );
        Err(Self::not_available_error())
    }

    fn is_available(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Host functions interface
// ---------------------------------------------------------------------------

/// Host functions exposed to WASM plugins.
///
/// These are the functions that plugins can call from within their WASM code
/// to interact with the XenoClaw platform. In a full implementation, each
/// function would be linked into the WASM module's import table.
///
/// ## Available Host Functions
///
/// | Function | Signature | Description |
/// |----------|-----------|-------------|
/// | `register_tool` | `(name, description, schema) -> result` | Register a tool with the ToolRegistry |
/// | `register_event_handler` | `(event_type, handler_id) -> result` | Register an event handler |
/// | `log` | `(level, message) -> ()` | Write to the agent's structured log |
/// | `memory_read` | `(key) -> value` | Read from the plugin's isolated memory store |
/// | `memory_write` | `(key, value) -> result` | Write to the plugin's isolated memory store |
///
/// ## Security Model
///
/// Each plugin gets its own `HostFunctions` instance scoped to its name.
/// All operations are subject to the Security Layer's resource limits:
/// - Tool registrations go through conflict resolution (first-registration-wins)
/// - Memory operations are namespaced to prevent cross-plugin access
/// - Logging is attributed to the plugin for audit purposes
pub struct HostFunctions {
    /// Name of the plugin these host functions are scoped to.
    pub plugin_name: String,
    // In a full implementation, these would hold Arc references to:
    // - ToolRegistry (for register_tool)
    // - EventBus (for register_event_handler)
    // - StructuredLogger (for log)
    // - Plugin-scoped KV store (for memory_read / memory_write)
    // - ResourceEnforcer (for enforcing limits on all operations)
}

impl HostFunctions {
    /// Create a new set of host functions scoped to a plugin.
    pub fn new(plugin_name: String) -> Self {
        Self { plugin_name }
    }

    /// Get the plugin name these host functions are scoped to.
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_value_is_numeric() {
        assert!(WasmValue::I32(42).is_numeric());
        assert!(WasmValue::I64(100).is_numeric());
        assert!(WasmValue::F32(3.14).is_numeric());
        assert!(WasmValue::F64(2.718).is_numeric());
        assert!(!WasmValue::String("hello".into()).is_numeric());
        assert!(!WasmValue::Bytes(vec![1, 2, 3]).is_numeric());
    }

    #[test]
    fn test_wasm_value_accessors() {
        assert_eq!(WasmValue::I32(42).as_i32(), Some(42));
        assert_eq!(WasmValue::I64(100).as_i64(), Some(100));
        assert_eq!(WasmValue::String("hi".into()).as_str(), Some("hi"));
        assert_eq!(
            WasmValue::Bytes(vec![1, 2]).as_bytes(),
            Some([1u8, 2].as_slice())
        );

        // Wrong type returns None
        assert_eq!(WasmValue::I32(42).as_str(), None);
        assert_eq!(WasmValue::String("hi".into()).as_i32(), None);
    }

    #[test]
    fn test_wasm_module_handle_equality() {
        let h1 = WasmModuleHandle {
            id: "abc-123".to_string(),
            name: "my-plugin".to_string(),
        };
        let h2 = WasmModuleHandle {
            id: "abc-123".to_string(),
            name: "my-plugin".to_string(),
        };
        let h3 = WasmModuleHandle {
            id: "def-456".to_string(),
            name: "other-plugin".to_string(),
        };

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_wasm_module_handle_hash() {
        use std::collections::HashSet;

        let h1 = WasmModuleHandle {
            id: "abc".to_string(),
            name: "plugin".to_string(),
        };
        let h2 = h1.clone();

        let mut set = HashSet::new();
        set.insert(h1);
        set.insert(h2); // duplicate, should not increase size
        assert_eq!(set.len(), 1);
    }

    #[tokio::test]
    async fn test_stub_runtime_load_returns_not_available() {
        let runtime = StubWasmRuntime::new();
        let result = runtime.load_module("test-plugin", &[0x00, 0x61]).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, WasmError::NotAvailable(_)));
        assert!(err.to_string().contains("wasmtime feature not enabled"));
    }

    #[tokio::test]
    async fn test_stub_runtime_call_returns_not_available() {
        let runtime = StubWasmRuntime::new();
        let handle = WasmModuleHandle {
            id: "fake".to_string(),
            name: "fake-module".to_string(),
        };

        let result = runtime
            .call_function(&handle, "init", &[WasmValue::I32(1)])
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WasmError::NotAvailable(_)));
    }

    #[tokio::test]
    async fn test_stub_runtime_unload_returns_not_available() {
        let runtime = StubWasmRuntime::new();
        let handle = WasmModuleHandle {
            id: "fake".to_string(),
            name: "fake-module".to_string(),
        };

        let result = runtime.unload_module(handle).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WasmError::NotAvailable(_)));
    }

    #[test]
    fn test_stub_runtime_is_not_available() {
        let runtime = StubWasmRuntime::new();
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_stub_runtime_default() {
        let runtime = StubWasmRuntime::default();
        assert!(!runtime.is_available());
    }

    #[test]
    fn test_host_functions_creation() {
        let hf = HostFunctions::new("my-plugin".to_string());
        assert_eq!(hf.plugin_name(), "my-plugin");
    }

    #[test]
    fn test_wasm_error_display() {
        let err = WasmError::LoadFailed {
            name: "bad-plugin".to_string(),
            reason: "invalid magic bytes".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to load module 'bad-plugin': invalid magic bytes"
        );

        let err = WasmError::FunctionNotFound {
            module: "my-mod".to_string(),
            function: "init".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Function 'init' not found in module 'my-mod'"
        );

        let err = WasmError::ExecutionError {
            module: "crashy".to_string(),
            reason: "out of memory".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Execution error in module 'crashy': out of memory"
        );

        let err = WasmError::ModuleNotLoaded {
            name: "ghost".to_string(),
        };
        assert_eq!(err.to_string(), "Module 'ghost' not loaded");

        let err = WasmError::TypeMismatch {
            module: "typed".to_string(),
            expected: "i32".to_string(),
            actual: "f64".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Type mismatch in module 'typed': expected i32, got f64"
        );
    }
}
