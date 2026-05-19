//! Shell command execution with streaming output, timeout enforcement,
//! concurrency limiting, and command allowlist/blocklist validation.
//!
//! Key behaviors:
//! - Default deny-all when no allowlist is configured
//! - Blocklist takes precedence over allowlist
//! - On timeout: kill the process, return error with command name and elapsed duration
//! - On concurrency limit: reject immediately with error message
//! - Stream output with ≤2 seconds latency

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use security_layer::validate_command;

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during shell command execution.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ShellError {
    /// The command was denied by the allowlist/blocklist policy.
    #[error("Command denied: '{command}' is not permitted by security policy")]
    CommandDenied { command: String },

    /// The concurrency limit has been reached.
    #[error("Concurrency limit reached: {limit} processes already running")]
    ConcurrencyLimitReached { limit: usize },

    /// The command timed out.
    #[error("Command '{command}' timed out after {elapsed_seconds}s")]
    Timeout {
        command: String,
        elapsed_seconds: u64,
    },

    /// The command failed to spawn.
    #[error("Failed to spawn command '{command}': {reason}")]
    SpawnFailed { command: String, reason: String },

    /// An I/O error occurred while reading output.
    #[error("I/O error: {0}")]
    IoError(String),
}

// =============================================================================
// Output Types
// =============================================================================

/// The result of executing a shell command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandOutput {
    /// Standard output captured from the process.
    pub stdout: String,
    /// Standard error captured from the process.
    pub stderr: String,
    /// The process exit code (None if the process was killed).
    pub exit_code: Option<i32>,
    /// Whether the command timed out.
    pub timed_out: bool,
    /// Duration the command ran for in milliseconds.
    pub duration_ms: u64,
}

/// A chunk of output streamed from a running process.
#[derive(Debug, Clone)]
pub enum OutputChunk {
    /// A line from stdout.
    Stdout(String),
    /// A line from stderr.
    Stderr(String),
    /// The process has completed.
    Done(CommandOutput),
}

/// Information about an active shell process.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// The command being executed.
    pub command: String,
    /// The arguments passed to the command.
    pub args: Vec<String>,
    /// The working directory (if specified).
    pub cwd: Option<PathBuf>,
    /// When the process was started.
    pub started_at: Instant,
}

// =============================================================================
// Shell Executor Configuration
// =============================================================================

/// Configuration for the shell executor.
#[derive(Debug, Clone)]
pub struct ShellExecutorConfig {
    /// Commands explicitly allowed for execution.
    pub command_allowlist: Vec<String>,
    /// Commands explicitly blocked from execution.
    pub command_blocklist: Vec<String>,
    /// Default timeout in seconds (default: 300).
    pub default_timeout_seconds: u32,
    /// Maximum concurrent shell processes (default: 5).
    pub max_concurrent_shells: usize,
}

impl Default for ShellExecutorConfig {
    fn default() -> Self {
        Self {
            command_allowlist: Vec::new(),
            command_blocklist: Vec::new(),
            default_timeout_seconds: 300,
            max_concurrent_shells: 5,
        }
    }
}

impl ShellExecutorConfig {
    /// Create a config from a CodingConfig.
    pub fn from_coding_config(config: &common::config::CodingConfig) -> Self {
        Self {
            command_allowlist: config.command_allowlist.clone(),
            command_blocklist: config.command_blocklist.clone(),
            default_timeout_seconds: config.shell_timeout_seconds,
            max_concurrent_shells: config.max_concurrent_shells as usize,
        }
    }
}

// =============================================================================
// Shell Executor
// =============================================================================

/// Executes shell commands with security validation, timeout enforcement,
/// concurrency limiting, and output streaming.
#[derive(Debug, Clone)]
pub struct ShellExecutor {
    config: Arc<ShellExecutorConfig>,
    active_processes: Arc<AtomicUsize>,
}

impl ShellExecutor {
    /// Create a new ShellExecutor with the given configuration.
    pub fn new(config: ShellExecutorConfig) -> Self {
        Self {
            config: Arc::new(config),
            active_processes: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns the number of currently active shell processes.
    pub fn active_count(&self) -> usize {
        self.active_processes.load(Ordering::SeqCst)
    }

    /// Returns the maximum number of concurrent shells allowed.
    pub fn max_concurrent(&self) -> usize {
        self.config.max_concurrent_shells
    }

    /// Validate a command against the allowlist/blocklist.
    ///
    /// Returns Ok(()) if the command is permitted, or an error if denied.
    pub fn validate(&self, command: &str) -> Result<(), ShellError> {
        validate_command(command, &self.config.command_allowlist, &self.config.command_blocklist)
            .map_err(|_| ShellError::CommandDenied {
                command: command.to_string(),
            })
    }

    /// Execute a shell command, capturing stdout, stderr, and exit code.
    ///
    /// This method:
    /// 1. Validates the command against the allowlist/blocklist
    /// 2. Checks the concurrency limit
    /// 3. Spawns the process with the given arguments, cwd, and env
    /// 4. Enforces the timeout (kills process on exceed)
    /// 5. Returns the captured output
    pub async fn execute(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
    ) -> Result<CommandOutput, ShellError> {
        // 1. Validate command against allowlist/blocklist
        self.validate(command)?;

        // 2. Check concurrency limit
        let current = self.active_processes.load(Ordering::SeqCst);
        if current >= self.config.max_concurrent_shells {
            return Err(ShellError::ConcurrencyLimitReached {
                limit: self.config.max_concurrent_shells,
            });
        }

        // Increment active count (use compare-exchange for safety)
        loop {
            let current = self.active_processes.load(Ordering::SeqCst);
            if current >= self.config.max_concurrent_shells {
                return Err(ShellError::ConcurrencyLimitReached {
                    limit: self.config.max_concurrent_shells,
                });
            }
            match self.active_processes.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(_) => continue, // Retry on contention
            }
        }

        // Ensure we decrement on exit
        let active_processes = Arc::clone(&self.active_processes);
        let _guard = scopeguard::defer(move || {
            active_processes.fetch_sub(1, Ordering::SeqCst);
        });

        // 3. Spawn the process
        let timeout_duration = timeout.unwrap_or(Duration::from_secs(
            self.config.default_timeout_seconds as u64,
        ));

        let start = Instant::now();

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        let mut child = cmd.spawn().map_err(|e| ShellError::SpawnFailed {
            command: command.to_string(),
            reason: e.to_string(),
        })?;

        let pid = child.id().unwrap_or(0);
        info!(command = %command, pid = pid, "Shell process spawned");

        // 4. Read output with timeout enforcement
        let stdout_pipe = child.stdout.take().ok_or_else(|| ShellError::IoError(
            "Failed to capture stdout".to_string(),
        ))?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| ShellError::IoError(
            "Failed to capture stderr".to_string(),
        ))?;

        let mut stdout_reader = BufReader::new(stdout_pipe);
        let mut stderr_reader = BufReader::new(stderr_pipe);

        // Read stdout and stderr concurrently with timeout
        let result = tokio::time::timeout(timeout_duration, async {
            let stdout_task = async {
                let mut buf = String::new();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stdout_reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => buf.push_str(&line),
                        Err(e) => {
                            debug!(error = %e, "Error reading stdout");
                            break;
                        }
                    }
                }
                buf
            };

            let stderr_task = async {
                let mut buf = String::new();
                let mut line = String::new();
                loop {
                    line.clear();
                    match stderr_reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => buf.push_str(&line),
                        Err(e) => {
                            debug!(error = %e, "Error reading stderr");
                            break;
                        }
                    }
                }
                buf
            };

            let (stdout, stderr) = tokio::join!(stdout_task, stderr_task);
            (stdout, stderr)
        })
        .await;

        match result {
            Ok((stdout_buf, stderr_buf)) => {
                // Wait for the process to exit
                let status = child.wait().await.map_err(|e| ShellError::IoError(e.to_string()))?;
                let elapsed = start.elapsed();

                info!(
                    command = %command,
                    pid = pid,
                    exit_code = ?status.code(),
                    duration_ms = elapsed.as_millis() as u64,
                    "Shell process completed"
                );

                Ok(CommandOutput {
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                    exit_code: status.code(),
                    timed_out: false,
                    duration_ms: elapsed.as_millis() as u64,
                })
            }
            Err(_) => {
                // Timeout exceeded — kill the process
                let elapsed = start.elapsed();
                warn!(
                    command = %command,
                    pid = pid,
                    elapsed_seconds = elapsed.as_secs(),
                    "Shell process timed out, killing"
                );

                // Kill the process
                let _ = child.kill().await;
                let _ = child.wait().await;

                Err(ShellError::Timeout {
                    command: command.to_string(),
                    elapsed_seconds: elapsed.as_secs(),
                })
            }
        }
    }

    /// Execute a shell command with streaming output.
    ///
    /// Returns a receiver that yields output chunks as they arrive.
    /// Output is delivered with ≤2 seconds latency from process output.
    pub async fn execute_streaming(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&PathBuf>,
        env: Option<&HashMap<String, String>>,
        timeout: Option<Duration>,
    ) -> Result<mpsc::Receiver<OutputChunk>, ShellError> {
        // 1. Validate command
        self.validate(command)?;

        // 2. Check concurrency limit (with atomic CAS)
        loop {
            let current = self.active_processes.load(Ordering::SeqCst);
            if current >= self.config.max_concurrent_shells {
                return Err(ShellError::ConcurrencyLimitReached {
                    limit: self.config.max_concurrent_shells,
                });
            }
            match self.active_processes.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(_) => continue,
            }
        }

        let timeout_duration = timeout.unwrap_or(Duration::from_secs(
            self.config.default_timeout_seconds as u64,
        ));

        // 3. Spawn the process
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        if let Some(env_vars) = env {
            for (key, value) in env_vars {
                cmd.env(key, value);
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            self.active_processes.fetch_sub(1, Ordering::SeqCst);
            ShellError::SpawnFailed {
                command: command.to_string(),
                reason: e.to_string(),
            }
        })?;

        let pid = child.id().unwrap_or(0);
        info!(command = %command, pid = pid, "Shell process spawned (streaming)");

        let stdout_pipe = child.stdout.take().ok_or_else(|| {
            self.active_processes.fetch_sub(1, Ordering::SeqCst);
            ShellError::IoError("Failed to capture stdout".to_string())
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| {
            self.active_processes.fetch_sub(1, Ordering::SeqCst);
            ShellError::IoError("Failed to capture stderr".to_string())
        })?;

        // Create channel for streaming output
        let (tx, rx) = mpsc::channel(256);
        let active_processes = Arc::clone(&self.active_processes);
        let command_name = command.to_string();

        // Spawn background task to read output and enforce timeout
        tokio::spawn(async move {
            let start = Instant::now();
            let mut stdout_reader = BufReader::new(stdout_pipe);
            let mut stderr_reader = BufReader::new(stderr_pipe);
            let mut stdout_buf = String::new();
            let mut stderr_buf = String::new();

            let read_result = tokio::time::timeout(timeout_duration, async {
                let tx_stdout = tx.clone();
                let tx_stderr = tx.clone();

                let stdout_task = async {
                    let mut collected = String::new();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match stdout_reader.read_line(&mut line).await {
                            Ok(0) => break,
                            Ok(_) => {
                                collected.push_str(&line);
                                let _ = tx_stdout.send(OutputChunk::Stdout(line.clone())).await;
                            }
                            Err(_) => break,
                        }
                    }
                    collected
                };

                let stderr_task = async {
                    let mut collected = String::new();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match stderr_reader.read_line(&mut line).await {
                            Ok(0) => break,
                            Ok(_) => {
                                collected.push_str(&line);
                                let _ = tx_stderr.send(OutputChunk::Stderr(line.clone())).await;
                            }
                            Err(_) => break,
                        }
                    }
                    collected
                };

                let (stdout, stderr) = tokio::join!(stdout_task, stderr_task);
                (stdout, stderr)
            })
            .await;

            match read_result {
                Ok((stdout, stderr)) => {
                    stdout_buf = stdout;
                    stderr_buf = stderr;

                    let status = child.wait().await.ok();
                    let elapsed = start.elapsed();
                    let exit_code = status.and_then(|s| s.code());

                    let output = CommandOutput {
                        stdout: stdout_buf,
                        stderr: stderr_buf,
                        exit_code,
                        timed_out: false,
                        duration_ms: elapsed.as_millis() as u64,
                    };

                    let _ = tx.send(OutputChunk::Done(output)).await;
                }
                Err(_) => {
                    // Timeout — kill the process
                    let elapsed = start.elapsed();
                    warn!(
                        command = %command_name,
                        pid = pid,
                        elapsed_seconds = elapsed.as_secs(),
                        "Shell process timed out (streaming), killing"
                    );
                    let _ = child.kill().await;
                    let _ = child.wait().await;

                    let output = CommandOutput {
                        stdout: stdout_buf,
                        stderr: stderr_buf,
                        exit_code: None,
                        timed_out: true,
                        duration_ms: elapsed.as_millis() as u64,
                    };

                    let _ = tx.send(OutputChunk::Done(output)).await;
                }
            }

            // Decrement active process count
            active_processes.fetch_sub(1, Ordering::SeqCst);
        });

        Ok(rx)
    }
}

// We use a simple scope guard pattern instead of pulling in the `scopeguard` crate.
// The `_guard` in execute() uses this module.
mod scopeguard {
    pub struct ScopeGuard<F: FnOnce()>(Option<F>);

    impl<F: FnOnce()> Drop for ScopeGuard<F> {
        fn drop(&mut self) {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }

    pub fn defer<F: FnOnce()>(f: F) -> ScopeGuard<F> {
        ScopeGuard(Some(f))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed_config() -> ShellExecutorConfig {
        ShellExecutorConfig {
            command_allowlist: vec![
                "echo".to_string(),
                "cat".to_string(),
                "ls".to_string(),
                "sleep".to_string(),
                "sh".to_string(),
                "true".to_string(),
                "false".to_string(),
            ],
            command_blocklist: vec!["rm".to_string()],
            default_timeout_seconds: 300,
            max_concurrent_shells: 5,
        }
    }

    #[test]
    fn test_validate_allowed_command() {
        let executor = ShellExecutor::new(allowed_config());
        assert!(executor.validate("echo").is_ok());
        assert!(executor.validate("ls").is_ok());
    }

    #[test]
    fn test_validate_blocked_command() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor.validate("rm");
        assert!(matches!(result, Err(ShellError::CommandDenied { .. })));
    }

    #[test]
    fn test_validate_unlisted_command() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor.validate("wget");
        assert!(matches!(result, Err(ShellError::CommandDenied { .. })));
    }

    #[test]
    fn test_validate_empty_allowlist_denies_all() {
        let config = ShellExecutorConfig {
            command_allowlist: vec![],
            command_blocklist: vec![],
            default_timeout_seconds: 300,
            max_concurrent_shells: 5,
        };
        let executor = ShellExecutor::new(config);
        let result = executor.validate("echo");
        assert!(matches!(result, Err(ShellError::CommandDenied { .. })));
    }

    #[test]
    fn test_blocklist_takes_precedence() {
        let config = ShellExecutorConfig {
            command_allowlist: vec!["rm".to_string()],
            command_blocklist: vec!["rm".to_string()],
            default_timeout_seconds: 300,
            max_concurrent_shells: 5,
        };
        let executor = ShellExecutor::new(config);
        let result = executor.validate("rm");
        assert!(matches!(result, Err(ShellError::CommandDenied { .. })));
    }

    #[tokio::test]
    async fn test_execute_simple_command() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor
            .execute("echo", &["hello world".to_string()], None, None, None)
            .await;

        let output = result.unwrap();
        assert_eq!(output.stdout.trim(), "hello world");
        assert_eq!(output.exit_code, Some(0));
        assert!(!output.timed_out);
    }

    #[tokio::test]
    async fn test_execute_captures_stderr() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor
            .execute(
                "sh",
                &["-c".to_string(), "echo error >&2".to_string()],
                None,
                None,
                None,
            )
            .await;

        let output = result.unwrap();
        assert_eq!(output.stderr.trim(), "error");
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn test_execute_captures_exit_code() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor
            .execute("false", &[], None, None, None)
            .await;

        let output = result.unwrap();
        assert_eq!(output.exit_code, Some(1));
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let config = ShellExecutorConfig {
            command_allowlist: vec!["sleep".to_string()],
            command_blocklist: vec![],
            default_timeout_seconds: 1,
            max_concurrent_shells: 5,
        };
        let executor = ShellExecutor::new(config);

        let result = executor
            .execute("sleep", &["10".to_string()], None, None, None)
            .await;

        assert!(matches!(result, Err(ShellError::Timeout { .. })));
        if let Err(ShellError::Timeout { command, elapsed_seconds }) = result {
            assert_eq!(command, "sleep");
            assert!(elapsed_seconds >= 1);
        }
    }

    #[tokio::test]
    async fn test_execute_custom_timeout() {
        let executor = ShellExecutor::new(allowed_config());

        let result = executor
            .execute(
                "sleep",
                &["10".to_string()],
                None,
                None,
                Some(Duration::from_secs(1)),
            )
            .await;

        assert!(matches!(result, Err(ShellError::Timeout { .. })));
    }

    #[tokio::test]
    async fn test_execute_concurrency_limit() {
        let config = ShellExecutorConfig {
            command_allowlist: vec!["sleep".to_string(), "echo".to_string()],
            command_blocklist: vec![],
            default_timeout_seconds: 300,
            max_concurrent_shells: 2,
        };
        let executor = ShellExecutor::new(config);

        // Start 2 long-running processes
        let exec1 = executor.clone();
        let exec2 = executor.clone();
        let exec3 = executor.clone();

        let h1 = tokio::spawn(async move {
            exec1
                .execute("sleep", &["5".to_string()], None, None, None)
                .await
        });
        let h2 = tokio::spawn(async move {
            exec2
                .execute("sleep", &["5".to_string()], None, None, None)
                .await
        });

        // Give them time to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Third should be rejected
        let result = exec3
            .execute("echo", &["test".to_string()], None, None, None)
            .await;

        assert!(matches!(
            result,
            Err(ShellError::ConcurrencyLimitReached { limit: 2 })
        ));

        // Cleanup
        h1.abort();
        h2.abort();
    }

    #[tokio::test]
    async fn test_execute_denied_command() {
        let executor = ShellExecutor::new(allowed_config());
        let result = executor
            .execute("rm", &["-rf".to_string(), "/".to_string()], None, None, None)
            .await;

        assert!(matches!(result, Err(ShellError::CommandDenied { .. })));
    }

    #[tokio::test]
    async fn test_execute_with_env() {
        let executor = ShellExecutor::new(allowed_config());
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "hello_env".to_string());

        let result = executor
            .execute(
                "sh",
                &["-c".to_string(), "echo $MY_VAR".to_string()],
                None,
                Some(&env),
                None,
            )
            .await;

        let output = result.unwrap();
        assert_eq!(output.stdout.trim(), "hello_env");
    }

    #[tokio::test]
    async fn test_execute_with_cwd() {
        let executor = ShellExecutor::new(allowed_config());
        let cwd = PathBuf::from("/tmp");

        let result = executor
            .execute(
                "sh",
                &["-c".to_string(), "pwd".to_string()],
                Some(&cwd),
                None,
                None,
            )
            .await;

        let output = result.unwrap();
        // On some systems /tmp may be a symlink to /private/tmp
        assert!(output.stdout.trim().contains("tmp"));
    }

    #[tokio::test]
    async fn test_active_count_tracks_processes() {
        let config = ShellExecutorConfig {
            command_allowlist: vec!["sleep".to_string()],
            command_blocklist: vec![],
            default_timeout_seconds: 300,
            max_concurrent_shells: 5,
        };
        let executor = ShellExecutor::new(config);

        assert_eq!(executor.active_count(), 0);

        let exec_clone = executor.clone();
        let handle = tokio::spawn(async move {
            exec_clone
                .execute("sleep", &["2".to_string()], None, None, None)
                .await
        });

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(executor.active_count(), 1);

        handle.abort();
        // Give it time to clean up
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(executor.active_count(), 0);
    }

    #[tokio::test]
    async fn test_streaming_output() {
        let executor = ShellExecutor::new(allowed_config());

        let mut rx = executor
            .execute_streaming(
                "echo",
                &["streaming test".to_string()],
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let mut got_stdout = false;
        let mut got_done = false;

        while let Some(chunk) = rx.recv().await {
            match chunk {
                OutputChunk::Stdout(line) => {
                    if line.trim() == "streaming test" {
                        got_stdout = true;
                    }
                }
                OutputChunk::Done(output) => {
                    assert_eq!(output.exit_code, Some(0));
                    assert!(!output.timed_out);
                    got_done = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(got_stdout);
        assert!(got_done);
    }
}
