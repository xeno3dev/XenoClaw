//! Property-based tests for the Plugin System.
//!
//! **Validates: Requirements 13.5, 13.7**
//!
//! Property 24: Plugin Fault Isolation
//! Property 25: Plugin Name Conflict Resolution

use std::path::Path;
use std::sync::Arc;

use proptest::prelude::*;
use tempfile::TempDir;
use tokio::sync::RwLock;

use agent_core::event_bus::EventBus;
use agent_core::tool_registry::{Tool, ToolRegistry};
use async_trait::async_trait;
use common::config::ResourceLimits;
use common::errors::PluginError;
use plugin_system::{PluginApi, PluginLoader};
use serde_json::Value;

// ============================================================================
// Strategies
// ============================================================================

/// Generate a valid plugin name (alphanumeric + hyphens, 1-20 chars).
fn valid_plugin_name_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z][a-z0-9\\-]{0,19}")
        .unwrap()
        .prop_filter("name must not be empty", |s| !s.is_empty())
}

/// Generate a valid semver version string.
fn valid_version_strategy() -> impl Strategy<Value = String> {
    (0u8..10, 0u8..20, 0u8..50)
        .prop_map(|(major, minor, patch)| format!("{}.{}.{}", major, minor, patch))
}

/// Kinds of invalid manifests that should cause load failures.
#[derive(Debug, Clone)]
enum InvalidManifestKind {
    /// Completely invalid TOML syntax.
    BadToml,
    /// Missing required fields.
    MissingFields,
    /// Incompatible API version (major version too high).
    IncompatibleApiVersion,
    /// Empty plugin name.
    EmptyName,
    /// Invalid characters in name.
    InvalidNameChars,
}

/// Generate an invalid manifest kind.
fn invalid_manifest_kind_strategy() -> impl Strategy<Value = InvalidManifestKind> {
    prop_oneof![
        Just(InvalidManifestKind::BadToml),
        Just(InvalidManifestKind::MissingFields),
        Just(InvalidManifestKind::IncompatibleApiVersion),
        Just(InvalidManifestKind::EmptyName),
        Just(InvalidManifestKind::InvalidNameChars),
    ]
}

/// Generate an invalid manifest TOML string based on the kind.
fn invalid_manifest_content(kind: &InvalidManifestKind, dir_name: &str) -> String {
    match kind {
        InvalidManifestKind::BadToml => "this is {{{{ not valid toml [[[[".to_string(),
        InvalidManifestKind::MissingFields => {
            // Missing version and api_version
            format!("name = \"{}\"", dir_name)
        }
        InvalidManifestKind::IncompatibleApiVersion => {
            format!(
                r#"name = "{}"
version = "1.0.0"
api_version = "99.0.0"
"#,
                dir_name
            )
        }
        InvalidManifestKind::EmptyName => r#"name = ""
version = "1.0.0"
api_version = "0.1.0"
"#
        .to_string(),
        InvalidManifestKind::InvalidNameChars => r#"name = "bad name/with spaces!"
version = "1.0.0"
api_version = "0.1.0"
"#
        .to_string(),
    }
}

/// A combination of valid and invalid plugins to place in a directory.
/// Each entry is (dir_name, is_valid, manifest_content).
#[derive(Debug, Clone)]
struct PluginSetup {
    /// Valid plugins: (dir_name, manifest_content)
    valid_plugins: Vec<(String, String)>,
    /// Invalid plugins: (dir_name, invalid_kind)
    invalid_plugins: Vec<(String, InvalidManifestKind)>,
}

/// Generate a mix of valid and invalid plugins (at least 1 valid, at least 1 invalid).
fn plugin_mix_strategy() -> impl Strategy<Value = PluginSetup> {
    let valid_count = 1..5usize;
    let invalid_count = 1..4usize;

    (valid_count, invalid_count).prop_flat_map(|(vc, ic)| {
        let valid_plugins = prop::collection::vec(
            (valid_plugin_name_strategy(), valid_version_strategy()),
            vc..=vc,
        )
        .prop_map(|entries| {
            entries
                .into_iter()
                .enumerate()
                .map(|(i, (name, version))| {
                    let dir_name = format!("valid-{}-{}", name, i);
                    let manifest = format!(
                        r#"name = "{}"
version = "{}"
api_version = "0.1.0"
description = "Valid test plugin"
"#,
                        dir_name, version
                    );
                    (dir_name, manifest)
                })
                .collect::<Vec<_>>()
        });

        let invalid_plugins = prop::collection::vec(invalid_manifest_kind_strategy(), ic..=ic)
            .prop_map(|kinds| {
                kinds
                    .into_iter()
                    .enumerate()
                    .map(|(i, kind)| {
                        let dir_name = format!("invalid-{}", i);
                        (dir_name, kind)
                    })
                    .collect::<Vec<_>>()
            });

        (valid_plugins, invalid_plugins).prop_map(|(valid, invalid)| PluginSetup {
            valid_plugins: valid,
            invalid_plugins: invalid,
        })
    })
}

/// Generate a valid tool name (alphanumeric + underscores, 1-30 chars).
fn tool_name_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z][a-z0-9_]{0,29}")
        .unwrap()
        .prop_filter("name must not be empty", |s| !s.is_empty())
}

// ============================================================================
// Helpers
// ============================================================================

/// Create a plugin directory with the given manifest content.
fn create_plugin_dir(base: &Path, dir_name: &str, manifest_content: &str) {
    let dir = base.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.toml"), manifest_content).unwrap();
}

/// Create a PluginLoader for testing.
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

/// A simple test tool implementation for property tests.
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
        "A test tool for property testing"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }

    async fn execute(&self, _arguments: Value) -> Result<String, String> {
        Ok(format!("executed {}", self.tool_name))
    }
}

/// Create a shared PluginApi setup with two plugin APIs sharing the same ToolRegistry.
fn create_shared_plugin_apis(
    plugin_a_name: &str,
    plugin_b_name: &str,
) -> (Arc<PluginApi>, Arc<PluginApi>, Arc<RwLock<ToolRegistry>>) {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let event_bus = EventBus::new(16);
    let enforcer = Arc::new(security_layer::ResourceEnforcer::new(ResourceLimits {
        max_memory_mb: 256,
        max_cpu_percent: 50,
        max_processes: 5,
    }));

    let api_a = Arc::new(PluginApi::new(
        plugin_a_name.to_string(),
        Arc::clone(&registry),
        event_bus.clone(),
        Arc::clone(&enforcer),
    ));

    let api_b = Arc::new(PluginApi::new(
        plugin_b_name.to_string(),
        Arc::clone(&registry),
        event_bus,
        enforcer,
    ));

    (api_a, api_b, registry)
}

// ============================================================================
// Property 24: Plugin Fault Isolation
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// **Validates: Requirements 13.5**
    ///
    /// Property 24: When one plugin fails to load (invalid manifest, missing files,
    /// incompatible API version), all other valid plugins in the same directory still
    /// load successfully. The failed plugin does not affect the system.
    #[test]
    fn prop_plugin_fault_isolation(setup in plugin_mix_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let plugins_dir = tmp.path();

            // Create valid plugin directories
            for (dir_name, manifest) in &setup.valid_plugins {
                create_plugin_dir(plugins_dir, dir_name, manifest);
            }

            // Create invalid plugin directories
            for (dir_name, kind) in &setup.invalid_plugins {
                let content = invalid_manifest_content(kind, dir_name);
                create_plugin_dir(plugins_dir, dir_name, &content);
            }

            let loader = create_test_loader(plugins_dir);
            let results = loader.load_all().await;

            // Total results should equal total plugins
            let total_plugins = setup.valid_plugins.len() + setup.invalid_plugins.len();
            prop_assert_eq!(
                results.len(),
                total_plugins,
                "Should have a result for each plugin directory. Expected {}, got {}",
                total_plugins,
                results.len()
            );

            // All valid plugins should have loaded successfully
            for (dir_name, _) in &setup.valid_plugins {
                let result = results.iter().find(|r| r.name == *dir_name);
                prop_assert!(
                    result.is_some(),
                    "Should have a result for valid plugin '{}'",
                    dir_name
                );
                let result = result.unwrap();
                prop_assert!(
                    result.success,
                    "Valid plugin '{}' should load successfully, but got error: {:?}",
                    dir_name,
                    result.error
                );
            }

            // All invalid plugins should have failed
            for (dir_name, _kind) in &setup.invalid_plugins {
                let result = results.iter().find(|r| r.name == *dir_name);
                prop_assert!(
                    result.is_some(),
                    "Should have a result for invalid plugin '{}'",
                    dir_name
                );
                let result = result.unwrap();
                prop_assert!(
                    !result.success,
                    "Invalid plugin '{}' should fail to load",
                    dir_name
                );
                prop_assert!(
                    result.error.is_some(),
                    "Failed plugin '{}' should have an error message",
                    dir_name
                );
            }

            // Verify that valid plugins are actually loaded and accessible
            let loaded_count = loader.plugin_count().await;
            prop_assert_eq!(
                loaded_count,
                setup.valid_plugins.len(),
                "Loaded plugin count should equal valid plugin count. Expected {}, got {}",
                setup.valid_plugins.len(),
                loaded_count
            );

            // Verify each valid plugin is individually accessible
            for (dir_name, _) in &setup.valid_plugins {
                prop_assert!(
                    loader.is_loaded(dir_name).await,
                    "Valid plugin '{}' should be accessible after load_all",
                    dir_name
                );
            }

            // Verify no invalid plugin is loaded
            for (dir_name, _) in &setup.invalid_plugins {
                prop_assert!(
                    !loader.is_loaded(dir_name).await,
                    "Invalid plugin '{}' should NOT be loaded",
                    dir_name
                );
            }

            Ok(())
        })?;
    }

    /// **Validates: Requirements 13.5**
    ///
    /// Property 24: When ALL plugins in a directory are invalid, load_all still
    /// completes without error (returns results with all failures) and the system
    /// remains operational.
    #[test]
    fn prop_plugin_fault_isolation_all_invalid(
        kinds in prop::collection::vec(invalid_manifest_kind_strategy(), 1..5)
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let plugins_dir = tmp.path();

            // Create only invalid plugins
            for (i, kind) in kinds.iter().enumerate() {
                let dir_name = format!("bad-plugin-{}", i);
                let content = invalid_manifest_content(kind, &dir_name);
                create_plugin_dir(plugins_dir, &dir_name, &content);
            }

            let loader = create_test_loader(plugins_dir);
            let results = loader.load_all().await;

            // All should fail
            prop_assert_eq!(results.len(), kinds.len());
            for result in &results {
                prop_assert!(
                    !result.success,
                    "All plugins should fail, but '{}' succeeded",
                    result.name
                );
            }

            // System should still be operational (no plugins loaded)
            prop_assert_eq!(loader.plugin_count().await, 0);

            Ok(())
        })?;
    }
}

// ============================================================================
// Property 25: Plugin Name Conflict Resolution
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 13.7**
    ///
    /// Property 25: When two plugins attempt to register a tool with the same name,
    /// the first registration wins, the second is rejected with a NameConflict error
    /// identifying both plugins, and the system continues operating with the original
    /// registration intact.
    #[test]
    fn prop_plugin_name_conflict_first_wins(
        tool_name in tool_name_strategy(),
        plugin_a_name in valid_plugin_name_strategy(),
        plugin_b_name in valid_plugin_name_strategy(),
    ) {
        // Ensure plugin names are different
        prop_assume!(plugin_a_name != plugin_b_name);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (api_a, api_b, registry) =
                create_shared_plugin_apis(&plugin_a_name, &plugin_b_name);

            // First plugin registers the tool — should succeed
            let tool_a = Arc::new(TestTool::new(&tool_name));
            let result_a = api_a.register_tool(tool_a).await;
            prop_assert!(
                result_a.is_ok(),
                "First registration of '{}' by plugin '{}' should succeed, got: {:?}",
                tool_name,
                plugin_a_name,
                result_a
            );

            // Second plugin tries to register the same tool name — should fail
            let tool_b = Arc::new(TestTool::new(&tool_name));
            let result_b = api_b.register_tool(tool_b).await;
            prop_assert!(
                result_b.is_err(),
                "Second registration of '{}' by plugin '{}' should fail",
                tool_name,
                plugin_b_name
            );

            // Verify the error is NameConflict with correct details
            let err = result_b.unwrap_err();
            match &err {
                PluginError::NameConflict {
                    name,
                    existing_plugin,
                } => {
                    prop_assert_eq!(
                        name, &tool_name,
                        "NameConflict error should identify the conflicting tool name"
                    );
                    prop_assert_eq!(
                        existing_plugin, &plugin_a_name,
                        "NameConflict error should identify the existing plugin owner"
                    );
                }
                other => {
                    return Err(TestCaseError::Fail(
                        format!(
                            "Expected NameConflict error, got: {:?}",
                            other
                        )
                        .into(),
                    ));
                }
            }

            // Verify the original registration is still intact
            let reg = registry.read().await;
            let registered_tool = reg.get(&tool_name);
            prop_assert!(
                registered_tool.is_some(),
                "Tool '{}' should still be registered after conflict",
                tool_name
            );

            // Verify the tool is still owned by the first plugin
            let owner = reg.tool_plugin(&tool_name);
            prop_assert_eq!(
                owner,
                Some(plugin_a_name.as_str()),
                "Tool '{}' should still be owned by first plugin '{}'",
                tool_name,
                plugin_a_name
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 13.7**
    ///
    /// Property 25: After a name conflict, the system continues operating — the
    /// first plugin's tool can still be executed, and both plugins can register
    /// other tools with different names.
    #[test]
    fn prop_plugin_name_conflict_system_continues(
        conflicting_name in tool_name_strategy(),
        extra_name_a in tool_name_strategy(),
        extra_name_b in tool_name_strategy(),
        plugin_a_name in valid_plugin_name_strategy(),
        plugin_b_name in valid_plugin_name_strategy(),
    ) {
        // Ensure all names are distinct
        prop_assume!(plugin_a_name != plugin_b_name);
        prop_assume!(conflicting_name != extra_name_a);
        prop_assume!(conflicting_name != extra_name_b);
        prop_assume!(extra_name_a != extra_name_b);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (api_a, api_b, registry) =
                create_shared_plugin_apis(&plugin_a_name, &plugin_b_name);

            // First plugin registers the conflicting tool
            let tool_conflict = Arc::new(TestTool::new(&conflicting_name));
            api_a.register_tool(tool_conflict).await.unwrap();

            // Second plugin tries and fails
            let tool_conflict_b = Arc::new(TestTool::new(&conflicting_name));
            let conflict_result = api_b.register_tool(tool_conflict_b).await;
            prop_assert!(conflict_result.is_err());

            // Both plugins can still register other tools with different names
            let tool_extra_a = Arc::new(TestTool::new(&extra_name_a));
            let result_extra_a = api_a.register_tool(tool_extra_a).await;
            prop_assert!(
                result_extra_a.is_ok(),
                "Plugin A should still be able to register other tools after conflict"
            );

            let tool_extra_b = Arc::new(TestTool::new(&extra_name_b));
            let result_extra_b = api_b.register_tool(tool_extra_b).await;
            prop_assert!(
                result_extra_b.is_ok(),
                "Plugin B should still be able to register other tools after conflict"
            );

            // Verify all three tools are registered
            let reg = registry.read().await;
            prop_assert_eq!(
                reg.tool_count(),
                3,
                "Should have 3 tools registered (conflict winner + 2 extras)"
            );

            // Verify the original conflicting tool can still be executed
            let result = reg
                .execute_tool_call("test-call", &conflicting_name, serde_json::json!({}))
                .await;
            prop_assert!(
                !result.is_error,
                "Original tool should still be executable after conflict"
            );
            prop_assert_eq!(
                result.output,
                format!("executed {}", conflicting_name),
                "Original tool should produce correct output"
            );

            Ok(())
        })?;
    }

    /// **Validates: Requirements 13.7**
    ///
    /// Property 25: For any sequence of N unique tool names registered by plugin A,
    /// followed by plugin B attempting to register the same N names, all N of B's
    /// registrations are rejected and all N of A's tools remain intact.
    #[test]
    fn prop_plugin_name_conflict_bulk(
        tool_names in prop::collection::hash_set(tool_name_strategy(), 1..8),
        plugin_a_name in valid_plugin_name_strategy(),
        plugin_b_name in valid_plugin_name_strategy(),
    ) {
        prop_assume!(plugin_a_name != plugin_b_name);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (api_a, api_b, registry) =
                create_shared_plugin_apis(&plugin_a_name, &plugin_b_name);

            let tool_names: Vec<String> = tool_names.into_iter().collect();

            // Plugin A registers all tools
            for name in &tool_names {
                let tool = Arc::new(TestTool::new(name));
                let result = api_a.register_tool(tool).await;
                prop_assert!(
                    result.is_ok(),
                    "Plugin A should register '{}' successfully",
                    name
                );
            }

            // Plugin B tries to register all the same tools — all should fail
            for name in &tool_names {
                let tool = Arc::new(TestTool::new(name));
                let result = api_b.register_tool(tool).await;
                prop_assert!(
                    result.is_err(),
                    "Plugin B should fail to register '{}' (already owned by A)",
                    name
                );

                // Verify it's a NameConflict error
                match result.unwrap_err() {
                    PluginError::NameConflict {
                        name: conflict_name,
                        existing_plugin,
                    } => {
                        prop_assert_eq!(&conflict_name, name);
                        prop_assert_eq!(&existing_plugin, &plugin_a_name);
                    }
                    other => {
                        return Err(TestCaseError::Fail(
                            format!("Expected NameConflict, got: {:?}", other).into(),
                        ));
                    }
                }
            }

            // All of A's tools should still be intact
            let reg = registry.read().await;
            prop_assert_eq!(
                reg.tool_count(),
                tool_names.len(),
                "All of plugin A's tools should remain registered"
            );

            for name in &tool_names {
                prop_assert_eq!(
                    reg.tool_plugin(name),
                    Some(plugin_a_name.as_str()),
                    "Tool '{}' should still be owned by plugin A",
                    name
                );
            }

            Ok(())
        })?;
    }
}
