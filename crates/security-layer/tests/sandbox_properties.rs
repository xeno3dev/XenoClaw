//! Property-based tests for sandbox enforcement.
//!
//! **Validates: Requirements 4.3, 5.4, 5.5, 11.1, 11.3, 11.6, 11.7**
//!
//! - Property 8: Filesystem Path Sandbox Validation
//! - Property 11: Shell Command Allowlist/Blocklist Enforcement
//! - Property 13: Network Access Control Default-Deny

use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;

use common::config::{FilesystemRule, NetworkRule};
use common::models::AccessType;
use security_layer::sandbox::{validate_command, validate_network, validate_path};

// ============================================================================
// Strategies
// ============================================================================

/// Generate an arbitrary AccessType.
fn access_type_strategy() -> impl Strategy<Value = AccessType> {
    prop_oneof![
        Just(AccessType::Read),
        Just(AccessType::Write),
        Just(AccessType::Execute),
    ]
}

/// Generate a relative path segment that may include `..` components.
fn path_segment_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // Normal directory/file names
        "[a-z][a-z0-9_]{0,7}".prop_map(|s| s),
        // Parent directory traversal
        Just("..".to_string()),
        // Current directory
        Just(".".to_string()),
    ]
}

/// Generate a relative path with 1-4 segments, potentially including `..` components.
fn relative_path_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(path_segment_strategy(), 1..=4)
}

/// Generate a valid hostname for network tests.
fn hostname_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("localhost".to_string()),
        Just("api.example.com".to_string()),
        Just("internal.service".to_string()),
        Just("db.cluster.local".to_string()),
        Just("cdn.provider.net".to_string()),
        "[a-z]{3,8}\\.[a-z]{2,5}".prop_map(|s| s),
    ]
}

/// Generate a port number.
fn port_strategy() -> impl Strategy<Value = u16> {
    prop_oneof![
        // Common ports
        Just(80u16),
        Just(443u16),
        Just(8080u16),
        Just(5432u16),
        Just(3306u16),
        Just(6379u16),
        // Random ports
        1..=65535u16,
    ]
}

/// Generate a command name for shell command tests.
fn command_name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("ls".to_string()),
        Just("cat".to_string()),
        Just("grep".to_string()),
        Just("rm".to_string()),
        Just("git".to_string()),
        Just("cargo".to_string()),
        Just("npm".to_string()),
        Just("python".to_string()),
        Just("node".to_string()),
        Just("curl".to_string()),
        Just("wget".to_string()),
        Just("ssh".to_string()),
        "[a-z]{2,8}".prop_map(|s| s),
    ]
}

// ============================================================================
// Property 8: Filesystem Path Sandbox Validation
//
// For any filesystem path (including paths with `..` components, symbolic links,
// and relative segments), the Security_Layer SHALL accept the path if and only
// if its canonical resolution falls within a configured allowed directory with
// the appropriate access permission (read, write, or execute).
//
// **Validates: Requirements 4.3, 11.1, 11.6**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// **Validates: Requirements 4.3, 11.1, 11.6**
    ///
    /// Property 8: Paths with `..` components that resolve inside an allowed
    /// directory SHALL be accepted; paths that resolve outside SHALL be rejected.
    #[test]
    fn prop_path_with_traversal_accepted_only_if_resolves_inside_allowed_dir(
        segments in relative_path_strategy(),
        access_type in access_type_strategy(),
    ) {
        // Set up a temp directory structure:
        //   tmp/
        //     allowed/
        //       subdir/
        //         file.txt
        //     outside/
        //       secret.txt
        let tmp = TempDir::new().unwrap();
        let allowed_dir = tmp.path().join("allowed");
        let subdir = allowed_dir.join("subdir");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("file.txt"), "content").unwrap();

        let outside_dir = tmp.path().join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("secret.txt"), "secret").unwrap();

        // Build a rule that allows all access types for the allowed directory
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: true,
            execute: true,
        }];

        // Construct a path starting from subdir with the generated segments
        let mut test_path = subdir.clone();
        for segment in &segments {
            test_path = test_path.join(segment);
        }

        let result = validate_path(&test_path, access_type, &rules);

        // Determine if the canonical path should be inside the allowed directory
        // by resolving it ourselves. We first try direct canonicalization, then
        // logical normalization (resolving `.` and `..` syntactically) followed
        // by canonicalization — matching the implementation's behavior.
        let canonical = if test_path.exists() {
            std::fs::canonicalize(&test_path).ok()
        } else {
            // Try logical normalization first (resolve .. syntactically)
            let normalized = normalize_path_for_test(&test_path);
            if let Ok(c) = std::fs::canonicalize(&normalized) {
                Some(c)
            } else if let Some(parent) = normalized.parent() {
                std::fs::canonicalize(parent)
                    .ok()
                    .map(|p| p.join(normalized.file_name().unwrap_or_default()))
            } else {
                None
            }
        };

        let allowed_canonical = std::fs::canonicalize(&allowed_dir).unwrap();

        match canonical {
            Some(ref resolved) if resolved.starts_with(&allowed_canonical) => {
                prop_assert!(
                    result.is_ok(),
                    "Path {:?} resolves to {:?} which is inside allowed dir {:?}, but was rejected",
                    test_path, resolved, allowed_canonical
                );
            }
            Some(ref resolved) => {
                prop_assert!(
                    result.is_err(),
                    "Path {:?} resolves to {:?} which is OUTSIDE allowed dir {:?}, but was accepted",
                    test_path, resolved, allowed_canonical
                );
            }
            None => {
                // Path cannot be resolved at all — should be rejected
                prop_assert!(
                    result.is_err(),
                    "Path {:?} cannot be canonicalized, should be rejected",
                    test_path
                );
            }
        }
    }

    /// **Validates: Requirements 4.3, 11.1, 11.6**
    ///
    /// Property 8: Access type enforcement — a path inside an allowed directory
    /// SHALL be rejected if the specific access type is not granted.
    #[test]
    fn prop_path_inside_allowed_dir_rejected_without_matching_permission(
        access_type in access_type_strategy(),
        grant_read in any::<bool>(),
        grant_write in any::<bool>(),
        grant_execute in any::<bool>(),
    ) {
        let tmp = TempDir::new().unwrap();
        let allowed_dir = tmp.path().join("workspace");
        fs::create_dir_all(&allowed_dir).unwrap();
        fs::write(allowed_dir.join("file.txt"), "data").unwrap();

        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: grant_read,
            write: grant_write,
            execute: grant_execute,
        }];

        let test_path = allowed_dir.join("file.txt");
        let result = validate_path(&test_path, access_type, &rules);

        let has_permission = match access_type {
            AccessType::Read => grant_read,
            AccessType::Write => grant_write,
            AccessType::Execute => grant_execute,
        };

        if has_permission {
            prop_assert!(
                result.is_ok(),
                "Path {:?} with {:?} access should be allowed (permission granted), but got {:?}",
                test_path, access_type, result
            );
        } else {
            prop_assert!(
                result.is_err(),
                "Path {:?} with {:?} access should be denied (permission not granted), but was allowed",
                test_path, access_type
            );
        }
    }

    /// **Validates: Requirements 4.3, 11.1, 11.6**
    ///
    /// Property 8: Traversal escape — any path that uses `..` to escape the
    /// allowed directory SHALL always be rejected regardless of access type.
    #[test]
    fn prop_traversal_escape_always_rejected(
        access_type in access_type_strategy(),
        extra_depth in 1..=3usize,
    ) {
        let tmp = TempDir::new().unwrap();
        let allowed_dir = tmp.path().join("sandbox");
        fs::create_dir_all(&allowed_dir).unwrap();

        // Create a file outside the sandbox
        fs::write(tmp.path().join("escape_target.txt"), "escaped").unwrap();

        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: true,
            execute: true,
        }];

        // Build a path that escapes: sandbox/../escape_target.txt
        let mut escape_path = allowed_dir.clone();
        for _ in 0..=extra_depth {
            escape_path = escape_path.join("..");
        }
        escape_path = escape_path.join("escape_target.txt");

        let result = validate_path(&escape_path, access_type, &rules);
        prop_assert!(
            result.is_err(),
            "Path {:?} escapes the sandbox via '..' traversal but was accepted with {:?} access",
            escape_path, access_type
        );
    }
}

// ============================================================================
// Property 11: Shell Command Allowlist/Blocklist Enforcement
//
// For any command, if it matches a blocklist entry or is not on the allowlist,
// it SHALL be rejected. If it is on the allowlist and not on the blocklist,
// it SHALL be accepted. With no allowlist configured, all commands SHALL be denied.
//
// **Validates: Requirements 5.4, 5.5**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// **Validates: Requirements 5.4, 5.5**
    ///
    /// Property 11: A command on the allowlist and NOT on the blocklist SHALL be accepted.
    #[test]
    fn prop_command_on_allowlist_not_on_blocklist_is_accepted(
        command in command_name_strategy(),
        extra_allowed in prop::collection::vec(command_name_strategy(), 0..5),
        blocklist in prop::collection::vec(command_name_strategy(), 0..5),
    ) {
        // Ensure the command is NOT in the blocklist
        let blocklist: Vec<String> = blocklist.into_iter()
            .filter(|b| b != &command)
            .collect();

        // Ensure the command IS in the allowlist
        let mut allowlist = extra_allowed;
        if !allowlist.contains(&command) {
            allowlist.push(command.clone());
        }

        let result = validate_command(&command, &allowlist, &blocklist);
        prop_assert!(
            result.is_ok(),
            "Command '{}' is on allowlist {:?} and NOT on blocklist {:?}, should be accepted but got {:?}",
            command, allowlist, blocklist, result
        );
    }

    /// **Validates: Requirements 5.4, 5.5**
    ///
    /// Property 11: A command on the blocklist SHALL always be rejected,
    /// even if it is also on the allowlist.
    #[test]
    fn prop_command_on_blocklist_is_always_rejected(
        command in command_name_strategy(),
        extra_allowed in prop::collection::vec(command_name_strategy(), 0..5),
    ) {
        // Put the command on both the allowlist and blocklist
        let mut allowlist = extra_allowed;
        if !allowlist.contains(&command) {
            allowlist.push(command.clone());
        }
        let blocklist = vec![command.clone()];

        let result = validate_command(&command, &allowlist, &blocklist);
        prop_assert!(
            result.is_err(),
            "Command '{}' is on blocklist {:?}, should be rejected even though it's on allowlist {:?}",
            command, blocklist, allowlist
        );
    }

    /// **Validates: Requirements 5.4, 5.5**
    ///
    /// Property 11: A command NOT on the allowlist SHALL be rejected.
    #[test]
    fn prop_command_not_on_allowlist_is_rejected(
        command in command_name_strategy(),
        allowlist in prop::collection::vec(command_name_strategy(), 1..5),
    ) {
        // Ensure the command is NOT in the allowlist
        let allowlist: Vec<String> = allowlist.into_iter()
            .filter(|a| a != &command)
            .collect();

        // Skip if filtering removed all entries (command happened to match all)
        prop_assume!(!allowlist.is_empty());

        let blocklist: Vec<String> = vec![];

        let result = validate_command(&command, &allowlist, &blocklist);
        prop_assert!(
            result.is_err(),
            "Command '{}' is NOT on allowlist {:?}, should be rejected but was accepted",
            command, allowlist
        );
    }

    /// **Validates: Requirements 5.4, 5.5**
    ///
    /// Property 11: With an empty allowlist, ALL commands SHALL be denied (default-deny).
    #[test]
    fn prop_empty_allowlist_denies_all_commands(
        command in command_name_strategy(),
    ) {
        let allowlist: Vec<String> = vec![];
        let blocklist: Vec<String> = vec![];

        let result = validate_command(&command, &allowlist, &blocklist);
        prop_assert!(
            result.is_err(),
            "Command '{}' should be denied when allowlist is empty (default-deny), but was accepted",
            command
        );
    }
}

// ============================================================================
// Property 13: Network Access Control Default-Deny
//
// For any (host, port) pair, the connection SHALL be allowed if and only if
// it matches an entry in the configured allowlist.
//
// **Validates: Requirements 11.3, 11.7**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// **Validates: Requirements 11.3, 11.7**
    ///
    /// Property 13: A (host, port) pair that matches an allowlist entry SHALL be accepted.
    #[test]
    fn prop_matching_host_port_is_accepted(
        host in hostname_strategy(),
        port in port_strategy(),
        extra_rules in prop::collection::vec(
            (hostname_strategy(), prop::option::of(port_strategy())),
            0..5
        ),
    ) {
        let mut allowlist: Vec<NetworkRule> = extra_rules
            .into_iter()
            .map(|(h, p)| NetworkRule { host: h, port: p })
            .collect();

        // Add the exact (host, port) to the allowlist
        allowlist.push(NetworkRule {
            host: host.clone(),
            port: Some(port),
        });

        let result = validate_network(&host, port, &allowlist);
        prop_assert!(
            result.is_ok(),
            "({}, {}) matches allowlist entry, should be accepted but got {:?}",
            host, port, result
        );
    }

    /// **Validates: Requirements 11.3, 11.7**
    ///
    /// Property 13: A (host, port) pair that does NOT match any allowlist entry
    /// SHALL be rejected (default-deny).
    #[test]
    fn prop_non_matching_host_port_is_rejected(
        host in hostname_strategy(),
        port in port_strategy(),
        allowlist_entries in prop::collection::vec(
            (hostname_strategy(), prop::option::of(port_strategy())),
            0..5
        ),
    ) {
        // Build an allowlist that does NOT contain any entry matching the target (host, port)
        let allowlist: Vec<NetworkRule> = allowlist_entries_filtered(&host, port, &allowlist_entries);

        let result = validate_network(&host, port, &allowlist);
        prop_assert!(
            result.is_err(),
            "({}, {}) does NOT match any allowlist entry {:?}, should be rejected but was accepted",
            host, port, allowlist
        );
    }

    /// **Validates: Requirements 11.3, 11.7**
    ///
    /// Property 13: An empty allowlist SHALL deny all connections (default-deny).
    #[test]
    fn prop_empty_allowlist_denies_all_connections(
        host in hostname_strategy(),
        port in port_strategy(),
    ) {
        let allowlist: Vec<NetworkRule> = vec![];

        let result = validate_network(&host, port, &allowlist);
        prop_assert!(
            result.is_err(),
            "({}, {}) should be denied with empty allowlist (default-deny), but was accepted",
            host, port
        );
    }

    /// **Validates: Requirements 11.3, 11.7**
    ///
    /// Property 13: A wildcard port rule (port=None) SHALL allow any port for that host.
    #[test]
    fn prop_wildcard_port_allows_any_port_for_host(
        host in hostname_strategy(),
        port in port_strategy(),
    ) {
        let allowlist = vec![NetworkRule {
            host: host.clone(),
            port: None, // All ports allowed
        }];

        let result = validate_network(&host, port, &allowlist);
        prop_assert!(
            result.is_ok(),
            "({}, {}) should be accepted because host '{}' has a wildcard port rule, but got {:?}",
            host, port, host, result
        );
    }

    /// **Validates: Requirements 11.3, 11.7**
    ///
    /// Property 13: A specific port rule SHALL only allow that exact port.
    #[test]
    fn prop_specific_port_rule_rejects_other_ports(
        host in hostname_strategy(),
        allowed_port in port_strategy(),
        request_port in port_strategy(),
    ) {
        prop_assume!(allowed_port != request_port);

        let allowlist = vec![NetworkRule {
            host: host.clone(),
            port: Some(allowed_port),
        }];

        let result = validate_network(&host, request_port, &allowlist);
        prop_assert!(
            result.is_err(),
            "({}, {}) should be rejected because only port {} is allowed for host '{}', but was accepted",
            host, request_port, allowed_port, host
        );
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Normalize a path by resolving `.` and `..` components logically (without filesystem access).
/// This mirrors the implementation's `normalize_path` function for test verification.
fn normalize_path_for_test(path: &std::path::Path) -> PathBuf {
    use std::path::Component;

    let mut components = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {
                // `.` — skip
            }
            Component::ParentDir => {
                // `..` — pop the last normal component if possible
                if let Some(last) = components.last() {
                    match last {
                        Component::Normal(_) => {
                            components.pop();
                        }
                        _ => {
                            components.push(component);
                        }
                    }
                } else {
                    components.push(component);
                }
            }
            _ => {
                components.push(component);
            }
        }
    }

    components.iter().collect()
}

/// Filter allowlist entries to exclude any that would match the given (host, port).
fn allowlist_entries_filtered(
    target_host: &str,
    target_port: u16,
    entries: &[(String, Option<u16>)],
) -> Vec<NetworkRule> {
    entries
        .iter()
        .filter(|(h, p)| {
            if h == target_host {
                match p {
                    None => false,           // Wildcard port would match — exclude
                    Some(port) => *port != target_port, // Only keep if port doesn't match
                }
            } else {
                true // Different host — keep it, won't match
            }
        })
        .map(|(h, p)| NetworkRule {
            host: h.clone(),
            port: *p,
        })
        .collect()
}
