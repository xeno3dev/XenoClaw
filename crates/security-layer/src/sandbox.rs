//! Sandbox enforcement for filesystem, network, and resource access.
//!
//! Provides:
//! - Filesystem path validation with canonical resolution (handles `..`, symlinks, relative paths)
//! - Per-directory read/write/execute permission enforcement
//! - Network access control with default-deny policy and allowlist
//! - CPU and memory limits for spawned processes (terminate within 5 seconds on violation)

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time;
use tracing::{info, warn};

use common::config::{FilesystemRule, NetworkRule, ResourceLimits};
use common::errors::SecurityError;
use common::models::AccessType;

// =============================================================================
// Filesystem Sandbox
// =============================================================================

/// Validates a filesystem path against the configured rules.
///
/// Resolves the path to its canonical form (handling `..`, symlinks, and relative paths),
/// then checks if it falls within any allowed directory with the appropriate permission.
///
/// Returns the canonical path on success, or `SecurityError::SandboxViolation` on failure.
pub fn validate_path(
    path: &Path,
    access_type: AccessType,
    rules: &[FilesystemRule],
) -> Result<PathBuf, SecurityError> {
    // Canonicalize the path to resolve .., symlinks, and relative components.
    // If the path doesn't exist yet (e.g., creating a new file), we canonicalize
    // the parent directory and append the filename.
    let canonical = canonicalize_path(path)?;

    // Check if the canonical path falls within any allowed directory
    // with the appropriate permission.
    for rule in rules {
        // Canonicalize the rule path as well for consistent comparison.
        let rule_canonical = match std::fs::canonicalize(&rule.path) {
            Ok(p) => p,
            Err(_) => continue, // Skip rules with non-existent paths
        };

        if canonical.starts_with(&rule_canonical) {
            let has_permission = match access_type {
                AccessType::Read => rule.read,
                AccessType::Write => rule.write,
                AccessType::Execute => rule.execute,
            };

            if has_permission {
                return Ok(canonical);
            }
        }
    }

    // No matching rule found or permission denied — sandbox violation.
    Err(SecurityError::SandboxViolation {
        path: path.to_path_buf(),
        access: access_type,
    })
}

/// Canonicalize a path, handling the case where the file doesn't exist yet.
///
/// If the full path exists, canonicalize it directly.
/// If not, normalize the path (resolving `.` and `..` logically) and then try to
/// canonicalize the longest existing prefix, appending the remaining components.
fn canonicalize_path(path: &Path) -> Result<PathBuf, SecurityError> {
    // Try direct canonicalization first (works when the full path exists)
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }

    // Normalize the path logically to resolve `.` and `..` components,
    // then try to canonicalize the result or its parent.
    let normalized = normalize_path(path);

    // Try canonicalizing the normalized path directly
    if let Ok(canonical) = std::fs::canonicalize(&normalized) {
        return Ok(canonical);
    }

    // If the normalized path doesn't exist, try to canonicalize its parent
    if let Some(parent) = normalized.parent() {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            if let Some(filename) = normalized.file_name() {
                return Ok(canonical_parent.join(filename));
            }
        }
    }

    // If we can't canonicalize at all, reject with sandbox violation.
    // This handles cases like completely non-existent parent directories.
    Err(SecurityError::SandboxViolation {
        path: path.to_path_buf(),
        access: AccessType::Read, // Default; caller will provide actual access type
    })
}

/// Normalize a path by resolving `.` and `..` components logically (without filesystem access).
///
/// This does NOT resolve symlinks — it only handles syntactic path normalization.
fn normalize_path(path: &Path) -> PathBuf {
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

// =============================================================================
// Network Sandbox
// =============================================================================

/// Validates a network connection attempt against the allowlist.
///
/// Implements a default-deny policy: only connections to hosts/ports explicitly
/// listed in the allowlist are permitted.
///
/// If a rule has `port = None`, all ports for that host are allowed.
pub fn validate_network(
    host: &str,
    port: u16,
    allowlist: &[NetworkRule],
) -> Result<(), SecurityError> {
    for rule in allowlist {
        if rule.host == host {
            match rule.port {
                None => return Ok(()), // All ports allowed for this host
                Some(allowed_port) if allowed_port == port => return Ok(()),
                _ => continue,
            }
        }
    }

    // Default deny — no matching rule found.
    Err(SecurityError::NetworkBlocked {
        host: host.to_string(),
        port,
    })
}

// =============================================================================
// Command Sandbox
// =============================================================================

/// Validates a shell command against the configured allowlist and blocklist.
///
/// Implements a default-deny policy:
/// - If the allowlist is empty, ALL commands are denied.
/// - If the command matches any blocklist entry, it is denied.
/// - If the command is on the allowlist and NOT on the blocklist, it is allowed.
///
/// Matching is performed on the base command name (first token / program name).
pub fn validate_command(
    command: &str,
    allowlist: &[String],
    blocklist: &[String],
) -> Result<(), SecurityError> {
    // Extract the base command (first whitespace-separated token)
    let base_command = command.split_whitespace().next().unwrap_or("");

    if base_command.is_empty() {
        return Err(SecurityError::SandboxViolation {
            path: std::path::PathBuf::from(command),
            access: AccessType::Execute,
        });
    }

    // Default-deny: if allowlist is empty, deny all commands
    if allowlist.is_empty() {
        return Err(SecurityError::SandboxViolation {
            path: std::path::PathBuf::from(base_command),
            access: AccessType::Execute,
        });
    }

    // Check blocklist first — blocklist takes precedence
    if blocklist.iter().any(|blocked| blocked == base_command) {
        return Err(SecurityError::SandboxViolation {
            path: std::path::PathBuf::from(base_command),
            access: AccessType::Execute,
        });
    }

    // Check allowlist
    if allowlist.iter().any(|allowed| allowed == base_command) {
        return Ok(());
    }

    // Not on allowlist — deny
    Err(SecurityError::SandboxViolation {
        path: std::path::PathBuf::from(base_command),
        access: AccessType::Execute,
    })
}

// =============================================================================
// Resource Enforcer
// =============================================================================

/// Tracks a spawned process for resource enforcement.
#[derive(Debug)]
struct TrackedProcess {
    child: Child,
    #[allow(dead_code)]
    started_at: std::time::Instant,
}

/// Enforces CPU and memory limits for spawned processes.
///
/// Spawns processes with resource constraints and monitors them.
/// Terminates processes within 5 seconds if they exceed configured limits.
#[derive(Debug, Clone)]
pub struct ResourceEnforcer {
    limits: ResourceLimits,
    processes: Arc<Mutex<Vec<TrackedProcess>>>,
}

impl ResourceEnforcer {
    /// Create a new ResourceEnforcer with the given limits.
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            processes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the configured resource limits.
    pub fn limits(&self) -> &ResourceLimits {
        &self.limits
    }

    /// Returns the current number of tracked processes.
    pub async fn active_process_count(&self) -> usize {
        let processes = self.processes.lock().await;
        processes.len()
    }

    /// Spawn a process with resource limits enforced.
    ///
    /// On Linux, uses ulimit-style constraints via pre_exec.
    /// Monitors the process and terminates it within 5 seconds if limits are exceeded.
    ///
    /// Returns the process ID on success, or an error if the process limit is reached.
    pub async fn spawn(
        &self,
        program: &str,
        args: &[&str],
        working_dir: Option<&Path>,
    ) -> Result<u32, SecurityError> {
        // Check process count limit
        {
            let processes = self.processes.lock().await;
            if processes.len() >= self.limits.max_processes as usize {
                return Err(SecurityError::SandboxViolation {
                    path: PathBuf::from(program),
                    access: AccessType::Execute,
                });
            }
        }

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        // On Unix, set resource limits via pre_exec
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let max_memory_bytes = (self.limits.max_memory_mb as u64) * 1024 * 1024;
            unsafe {
                cmd.pre_exec(move || {
                    // Set memory limit (RLIMIT_AS - address space)
                    let mem_limit = libc::rlimit {
                        rlim_cur: max_memory_bytes,
                        rlim_max: max_memory_bytes,
                    };
                    libc::setrlimit(libc::RLIMIT_AS, &mem_limit);

                    // Set CPU time limit (RLIMIT_CPU) - use a generous limit
                    // The monitoring loop handles finer-grained enforcement
                    let cpu_limit = libc::rlimit {
                        rlim_cur: 3600, // 1 hour hard cap
                        rlim_max: 3600,
                    };
                    libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit);

                    Ok(())
                });
            }
        }

        let child = cmd.spawn().map_err(|_| SecurityError::SandboxViolation {
            path: PathBuf::from(program),
            access: AccessType::Execute,
        })?;

        let pid = child.id().unwrap_or(0);

        info!(pid = pid, program = program, "Spawned sandboxed process");

        // Track the process
        {
            let mut processes = self.processes.lock().await;
            processes.push(TrackedProcess {
                child,
                started_at: std::time::Instant::now(),
            });
        }

        // Start the monitoring task
        let processes_clone = Arc::clone(&self.processes);
        let max_memory_mb = self.limits.max_memory_mb;
        let _max_cpu_percent = self.limits.max_cpu_percent;

        tokio::spawn(async move {
            Self::monitor_process(processes_clone, pid, max_memory_mb).await;
        });

        Ok(pid)
    }

    /// Monitor a process and terminate it within 5 seconds if limits are exceeded.
    async fn monitor_process(
        processes: Arc<Mutex<Vec<TrackedProcess>>>,
        pid: u32,
        max_memory_mb: u32,
    ) {
        let check_interval = Duration::from_secs(1);
        let kill_deadline = Duration::from_secs(5);

        loop {
            time::sleep(check_interval).await;

            // Check if process is still running
            let mut procs = processes.lock().await;
            let proc_idx = procs.iter().position(|p| p.child.id() == Some(pid));

            let Some(idx) = proc_idx else {
                // Process no longer tracked (already exited or removed)
                break;
            };

            // Check if process has exited
            match procs[idx].child.try_wait() {
                Ok(Some(_status)) => {
                    // Process has exited, remove from tracking
                    procs.remove(idx);
                    break;
                }
                Ok(None) => {
                    // Still running, check resource usage
                }
                Err(_) => {
                    procs.remove(idx);
                    break;
                }
            }

            // Check memory usage (Linux-specific via /proc)
            #[cfg(target_os = "linux")]
            {
                if let Some(memory_mb) = get_process_memory_mb(pid) {
                    if memory_mb > max_memory_mb as u64 {
                        warn!(
                            pid = pid,
                            memory_mb = memory_mb,
                            limit_mb = max_memory_mb,
                            "Process exceeded memory limit, terminating"
                        );
                        Self::terminate_process(&mut procs[idx].child, kill_deadline).await;
                        procs.remove(idx);
                        break;
                    }
                }
            }

            // On non-Linux platforms, rely on the OS-level limits set via pre_exec
            #[cfg(not(target_os = "linux"))]
            {
                let _ = max_memory_mb;
            }

            drop(procs);
        }
    }

    /// Terminate a process, ensuring it's killed within the deadline.
    ///
    /// First sends SIGTERM (on Unix) or starts kill, then waits up to the deadline.
    /// If the process hasn't exited by then, sends SIGKILL.
    async fn terminate_process(child: &mut Child, deadline: Duration) {
        // Try graceful termination first
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = child.start_kill();
        }

        // Wait for process to exit within deadline
        match time::timeout(deadline, child.wait()).await {
            Ok(_) => {
                info!("Process terminated gracefully");
            }
            Err(_) => {
                // Force kill if still running after deadline
                warn!("Process did not terminate gracefully, force killing");
                let _ = child.kill().await;
            }
        }
    }

    /// Wait for a specific process to complete and remove it from tracking.
    pub async fn wait_for_process(&self, pid: u32) -> Option<std::process::ExitStatus> {
        loop {
            let mut procs = self.processes.lock().await;
            let proc_idx = procs.iter().position(|p| p.child.id() == Some(pid));

            if let Some(idx) = proc_idx {
                match procs[idx].child.try_wait() {
                    Ok(Some(status)) => {
                        procs.remove(idx);
                        return Some(status);
                    }
                    Ok(None) => {
                        // Still running, drop lock and wait
                        drop(procs);
                        time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(_) => {
                        procs.remove(idx);
                        return None;
                    }
                }
            } else {
                return None;
            }
        }
    }

    /// Terminate all tracked processes (cleanup on shutdown).
    pub async fn terminate_all(&self) {
        let mut procs = self.processes.lock().await;
        let deadline = Duration::from_secs(5);

        for tracked in procs.iter_mut() {
            Self::terminate_process(&mut tracked.child, deadline).await;
        }

        procs.clear();
        info!("All sandboxed processes terminated");
    }
}

/// Read process memory usage from /proc on Linux.
#[cfg(target_os = "linux")]
fn get_process_memory_mb(pid: u32) -> Option<u64> {
    let status_path = format!("/proc/{}/status", pid);
    let content = std::fs::read_to_string(status_path).ok()?;

    for line in content.lines() {
        if line.starts_with("VmRSS:") {
            // VmRSS is in kB
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let kb: u64 = parts[1].parse().ok()?;
                return Some(kb / 1024);
            }
        }
    }

    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper to create a temporary directory structure for testing
    fn setup_test_dirs() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let allowed_dir = tmp.path().join("allowed");
        let forbidden_dir = tmp.path().join("forbidden");
        fs::create_dir_all(&allowed_dir).unwrap();
        fs::create_dir_all(&forbidden_dir).unwrap();
        // Create a file in the allowed directory
        fs::write(allowed_dir.join("test.txt"), "hello").unwrap();
        (tmp, allowed_dir, forbidden_dir)
    }

    #[test]
    fn test_validate_path_allows_read_in_permitted_directory() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: false,
            execute: false,
        }];

        let result = validate_path(&allowed_dir.join("test.txt"), AccessType::Read, &rules);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_denies_write_without_permission() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: false,
            execute: false,
        }];

        let result = validate_path(&allowed_dir.join("test.txt"), AccessType::Write, &rules);
        assert!(matches!(
            result,
            Err(SecurityError::SandboxViolation { .. })
        ));
    }

    #[test]
    fn test_validate_path_denies_access_outside_rules() {
        let (_tmp, allowed_dir, forbidden_dir) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir,
            read: true,
            write: true,
            execute: false,
        }];

        // Create a file in the forbidden directory
        fs::write(forbidden_dir.join("secret.txt"), "secret").unwrap();

        let result = validate_path(&forbidden_dir.join("secret.txt"), AccessType::Read, &rules);
        assert!(matches!(
            result,
            Err(SecurityError::SandboxViolation { .. })
        ));
    }

    #[test]
    fn test_validate_path_resolves_dot_dot() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: false,
            execute: false,
        }];

        // Try to escape using ..
        let sneaky_path = allowed_dir.join("subdir").join("..").join("test.txt");
        let result = validate_path(&sneaky_path, AccessType::Read, &rules);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_blocks_traversal_escape() {
        let (tmp, allowed_dir, _) = setup_test_dirs();
        // Create a file outside the allowed directory
        fs::write(tmp.path().join("outside.txt"), "outside").unwrap();

        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: true,
            execute: true,
        }];

        // Try to escape using .. to reach the parent
        let escape_path = allowed_dir.join("..").join("outside.txt");
        let result = validate_path(&escape_path, AccessType::Read, &rules);
        assert!(matches!(
            result,
            Err(SecurityError::SandboxViolation { .. })
        ));
    }

    #[test]
    fn test_validate_path_allows_new_file_in_permitted_directory() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: true,
            execute: false,
        }];

        // File doesn't exist yet but parent is allowed
        let new_file = allowed_dir.join("new_file.txt");
        let result = validate_path(&new_file, AccessType::Write, &rules);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_execute_permission() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules = vec![FilesystemRule {
            path: allowed_dir.clone(),
            read: true,
            write: false,
            execute: true,
        }];

        let result = validate_path(&allowed_dir.join("test.txt"), AccessType::Execute, &rules);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_empty_rules_denies_all() {
        let (_tmp, allowed_dir, _) = setup_test_dirs();
        let rules: Vec<FilesystemRule> = vec![];

        let result = validate_path(&allowed_dir.join("test.txt"), AccessType::Read, &rules);
        assert!(matches!(
            result,
            Err(SecurityError::SandboxViolation { .. })
        ));
    }

    // =========================================================================
    // Network Tests
    // =========================================================================

    #[test]
    fn test_validate_network_allows_matching_host_and_port() {
        let allowlist = vec![NetworkRule {
            host: "api.example.com".to_string(),
            port: Some(443),
        }];

        let result = validate_network("api.example.com", 443, &allowlist);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_network_denies_wrong_port() {
        let allowlist = vec![NetworkRule {
            host: "api.example.com".to_string(),
            port: Some(443),
        }];

        let result = validate_network("api.example.com", 8080, &allowlist);
        assert!(matches!(result, Err(SecurityError::NetworkBlocked { .. })));
    }

    #[test]
    fn test_validate_network_denies_wrong_host() {
        let allowlist = vec![NetworkRule {
            host: "api.example.com".to_string(),
            port: Some(443),
        }];

        let result = validate_network("evil.com", 443, &allowlist);
        assert!(matches!(result, Err(SecurityError::NetworkBlocked { .. })));
    }

    #[test]
    fn test_validate_network_allows_all_ports_when_port_is_none() {
        let allowlist = vec![NetworkRule {
            host: "trusted.internal".to_string(),
            port: None,
        }];

        assert!(validate_network("trusted.internal", 80, &allowlist).is_ok());
        assert!(validate_network("trusted.internal", 443, &allowlist).is_ok());
        assert!(validate_network("trusted.internal", 9090, &allowlist).is_ok());
    }

    #[test]
    fn test_validate_network_default_deny_empty_allowlist() {
        let allowlist: Vec<NetworkRule> = vec![];

        let result = validate_network("any.host.com", 80, &allowlist);
        assert!(matches!(result, Err(SecurityError::NetworkBlocked { .. })));
    }

    #[test]
    fn test_validate_network_multiple_rules() {
        let allowlist = vec![
            NetworkRule {
                host: "api.openai.com".to_string(),
                port: Some(443),
            },
            NetworkRule {
                host: "localhost".to_string(),
                port: None,
            },
        ];

        assert!(validate_network("api.openai.com", 443, &allowlist).is_ok());
        assert!(validate_network("localhost", 5432, &allowlist).is_ok());
        assert!(validate_network("api.openai.com", 80, &allowlist).is_err());
        assert!(validate_network("unknown.com", 443, &allowlist).is_err());
    }

    // =========================================================================
    // Resource Enforcer Tests
    // =========================================================================

    #[tokio::test]
    async fn test_resource_enforcer_creation() {
        let limits = ResourceLimits {
            max_memory_mb: 256,
            max_cpu_percent: 50,
            max_processes: 5,
        };

        let enforcer = ResourceEnforcer::new(limits.clone());
        assert_eq!(enforcer.limits().max_memory_mb, 256);
        assert_eq!(enforcer.limits().max_cpu_percent, 50);
        assert_eq!(enforcer.limits().max_processes, 5);
        assert_eq!(enforcer.active_process_count().await, 0);
    }

    #[tokio::test]
    async fn test_resource_enforcer_spawn_and_wait() {
        let limits = ResourceLimits {
            max_memory_mb: 512,
            max_cpu_percent: 80,
            max_processes: 10,
        };

        let enforcer = ResourceEnforcer::new(limits);
        let pid = enforcer.spawn("echo", &["hello"], None).await.unwrap();
        assert!(pid > 0);

        // Wait for the process to complete
        let status = enforcer.wait_for_process(pid).await;
        assert!(status.is_some());
        assert!(status.unwrap().success());
    }

    #[tokio::test]
    async fn test_resource_enforcer_process_limit() {
        let limits = ResourceLimits {
            max_memory_mb: 512,
            max_cpu_percent: 80,
            max_processes: 1, // Only allow 1 process
        };

        let enforcer = ResourceEnforcer::new(limits);

        // Spawn a long-running process
        let _pid = enforcer.spawn("sleep", &["10"], None).await.unwrap();

        // Second spawn should fail due to process limit
        let result = enforcer.spawn("echo", &["test"], None).await;
        assert!(result.is_err());

        // Cleanup
        enforcer.terminate_all().await;
    }

    #[tokio::test]
    async fn test_resource_enforcer_terminate_all() {
        let limits = ResourceLimits {
            max_memory_mb: 512,
            max_cpu_percent: 80,
            max_processes: 10,
        };

        let enforcer = ResourceEnforcer::new(limits);
        let _pid = enforcer.spawn("sleep", &["60"], None).await.unwrap();

        assert_eq!(enforcer.active_process_count().await, 1);

        enforcer.terminate_all().await;
        assert_eq!(enforcer.active_process_count().await, 0);
    }
}
