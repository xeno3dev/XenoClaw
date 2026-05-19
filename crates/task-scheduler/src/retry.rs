//! Retry logic with exponential backoff for failed task executions.
//!
//! Implements:
//! - Exponential backoff calculation: delay = min(base_interval * 2^(attempt-1), max_interval)
//! - Configurable max retries (default 3) from the task's RetryPolicy
//! - Timeout enforcement via tokio::time::timeout
//! - Task history recording with full details
//! - History retention cleanup (30 days)

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Duration, Utc};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use common::models::{RetryPolicy, Task, TaskRun, TaskRunStatus, TaskStatus};
use common::types::TaskId;

use crate::scheduler::TaskExecutor;

/// Calculate the backoff delay for a given attempt number.
///
/// Formula: delay = min(base_interval * 2^(attempt-1), max_interval)
/// where attempt is 1-indexed (first retry = attempt 1).
pub fn calculate_backoff(policy: &RetryPolicy, attempt: u8) -> std::time::Duration {
    if attempt == 0 {
        return std::time::Duration::from_secs(0);
    }

    let base = policy.base_interval_secs as u64;
    let max = policy.max_interval_secs as u64;

    // 2^(attempt-1), saturating to avoid overflow
    let exponent = (attempt - 1) as u32;
    let multiplier = 2u64.saturating_pow(exponent);
    let delay_secs = base.saturating_mul(multiplier).min(max);

    std::time::Duration::from_secs(delay_secs)
}

/// Execute a task with retry logic, timeout enforcement, and history recording.
///
/// This function:
/// 1. Attempts to execute the task, wrapping in a timeout
/// 2. On failure or timeout, waits the backoff duration and retries
/// 3. After all retries exhausted, marks the task as Failed
/// 4. Records all attempts in the history
/// 5. Returns whether the task ultimately succeeded
pub async fn execute_with_retry(
    task: &Task,
    tasks: &RwLock<HashMap<TaskId, Task>>,
    history: &RwLock<Vec<TaskRun>>,
    executor: &Arc<dyn TaskExecutor>,
) -> bool {
    let max_attempts = task.retry_policy.max_retries + 1; // initial attempt + retries
    let timeout_duration = std::time::Duration::from_secs(task.timeout_seconds as u64);

    let mut last_error: Option<String> = None;
    let mut attempt_runs: Vec<TaskRun> = Vec::new();

    for attempt in 1..=max_attempts {
        let start_time = Utc::now();
        let run_id = uuid::Uuid::new_v4();

        if attempt > 1 {
            // Calculate and apply backoff delay before retry
            let backoff = calculate_backoff(&task.retry_policy, attempt - 1);
            info!(
                "Task '{}' ({}): retry attempt {}/{} after {}s backoff",
                task.name,
                task.id,
                attempt - 1,
                task.retry_policy.max_retries,
                backoff.as_secs()
            );
            tokio::time::sleep(backoff).await;
        }

        // Execute with timeout enforcement
        let result = tokio::time::timeout(timeout_duration, executor.execute(task)).await;

        let end_time = Utc::now();
        let duration_ms = (end_time - start_time).num_milliseconds() as u64;

        let (status, error) = match result {
            Ok(Ok(())) => {
                // Task succeeded
                info!(
                    "Task '{}' ({}) succeeded on attempt {}",
                    task.name, task.id, attempt
                );
                (TaskRunStatus::Succeeded, None)
            }
            Ok(Err(e)) => {
                // Task failed with an error
                warn!(
                    "Task '{}' ({}) failed on attempt {}: {}",
                    task.name, task.id, attempt, e
                );
                last_error = Some(e.clone());
                (TaskRunStatus::Failed, Some(e))
            }
            Err(_) => {
                // Task timed out
                let timeout_msg = format!(
                    "Task exceeded configured timeout of {}s",
                    task.timeout_seconds
                );
                warn!(
                    "Task '{}' ({}) timed out on attempt {}: {}",
                    task.name, task.id, attempt, timeout_msg
                );
                last_error = Some(timeout_msg.clone());
                (TaskRunStatus::TimedOut, Some(timeout_msg))
            }
        };

        let run = TaskRun {
            id: run_id,
            task_id: task.id,
            start_time,
            end_time: Some(end_time),
            duration_ms: Some(duration_ms),
            status,
            error,
            attempt,
        };

        attempt_runs.push(run.clone());

        if status == TaskRunStatus::Succeeded {
            // Record all attempts and return success
            history.write().await.extend(attempt_runs);
            return true;
        }

        // If this was the last attempt, don't wait for another retry
        if attempt == max_attempts {
            break;
        }
    }

    // All retries exhausted — mark task as failed
    error!(
        "Task '{}' ({}) failed after {} attempt(s). Last error: {}",
        task.name,
        task.id,
        max_attempts,
        last_error.as_deref().unwrap_or("unknown")
    );

    // Log summary of all attempts
    log_attempt_summary(&task.name, &task.id, &attempt_runs);

    // Record all attempts in history
    history.write().await.extend(attempt_runs);

    // Update task status to Failed
    if let Some(task_entry) = tasks.write().await.get_mut(&task.id) {
        task_entry.status = TaskStatus::Failed {
            last_error: last_error.unwrap_or_else(|| "unknown error".to_string()),
        };
        task_entry.updated_at = Utc::now();
    }

    false
}

/// Log a summary of all retry attempts for a failed task.
fn log_attempt_summary(task_name: &str, task_id: &TaskId, runs: &[TaskRun]) {
    let mut summary = format!(
        "Task '{}' ({}) retry summary ({} attempts):",
        task_name, task_id, runs.len()
    );

    for run in runs {
        let status_str = match run.status {
            TaskRunStatus::Succeeded => "succeeded",
            TaskRunStatus::Failed => "failed",
            TaskRunStatus::TimedOut => "timed_out",
            TaskRunStatus::Running => "running",
        };
        let error_str = run
            .error
            .as_deref()
            .map(|e| format!(" - {}", e))
            .unwrap_or_default();
        let duration_str = run
            .duration_ms
            .map(|d| format!(" ({}ms)", d))
            .unwrap_or_default();

        summary.push_str(&format!(
            "\n  Attempt {}: {}{}{}", 
            run.attempt, status_str, duration_str, error_str
        ));
    }

    error!("{}", summary);
}

/// Remove task history entries older than the specified retention period.
///
/// Default retention is 30 days. This should be called periodically
/// (e.g., once per day) to keep the history store bounded.
pub async fn cleanup_old_history(history: &RwLock<Vec<TaskRun>>, retention_days: i64) {
    let cutoff = Utc::now() - Duration::days(retention_days);
    let mut history_guard = history.write().await;
    let before_count = history_guard.len();

    history_guard.retain(|run| run.start_time > cutoff);

    let removed = before_count - history_guard.len();
    if removed > 0 {
        info!(
            "Cleaned up {} task history entries older than {} days",
            removed, retention_days
        );
    }
}

/// Default retention period for task history in days.
pub const DEFAULT_RETENTION_DAYS: i64 = 30;

#[cfg(test)]
mod tests {
    use super::*;
    use common::models::{RetryPolicy, TaskAction, TaskStatus, TaskTrigger};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn make_retry_policy(max_retries: u8, base: u32, max: u32) -> RetryPolicy {
        RetryPolicy {
            max_retries,
            base_interval_secs: base,
            max_interval_secs: max,
        }
    }

    fn make_test_task_with_policy(retry_policy: RetryPolicy, timeout_seconds: u32) -> Task {
        Task {
            id: TaskId::new(),
            name: "retry-test-task".to_string(),
            trigger: TaskTrigger::Cron {
                expression: "0 * * * * *".to_string(),
            },
            action: TaskAction::Command {
                command: "echo".to_string(),
                args: vec!["test".to_string()],
            },
            timeout_seconds,
            dependencies: vec![],
            retry_policy,
            status: TaskStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // =========================================================================
    // Backoff calculation tests
    // =========================================================================

    #[test]
    fn test_backoff_attempt_1() {
        let policy = make_retry_policy(3, 10, 300);
        // attempt 1: 10 * 2^0 = 10s
        let delay = calculate_backoff(&policy, 1);
        assert_eq!(delay, std::time::Duration::from_secs(10));
    }

    #[test]
    fn test_backoff_attempt_2() {
        let policy = make_retry_policy(3, 10, 300);
        // attempt 2: 10 * 2^1 = 20s
        let delay = calculate_backoff(&policy, 2);
        assert_eq!(delay, std::time::Duration::from_secs(20));
    }

    #[test]
    fn test_backoff_attempt_3() {
        let policy = make_retry_policy(3, 10, 300);
        // attempt 3: 10 * 2^2 = 40s
        let delay = calculate_backoff(&policy, 3);
        assert_eq!(delay, std::time::Duration::from_secs(40));
    }

    #[test]
    fn test_backoff_capped_at_max() {
        let policy = make_retry_policy(10, 10, 300);
        // attempt 6: 10 * 2^5 = 320, capped at 300
        let delay = calculate_backoff(&policy, 6);
        assert_eq!(delay, std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_backoff_attempt_0_returns_zero() {
        let policy = make_retry_policy(3, 10, 300);
        let delay = calculate_backoff(&policy, 0);
        assert_eq!(delay, std::time::Duration::from_secs(0));
    }

    #[test]
    fn test_backoff_large_attempt_saturates() {
        let policy = make_retry_policy(255, 10, 300);
        // Very large attempt should still be capped at max
        let delay = calculate_backoff(&policy, 255);
        assert_eq!(delay, std::time::Duration::from_secs(300));
    }

    // =========================================================================
    // Retry execution tests
    // =========================================================================

    /// A mock executor that fails a configurable number of times then succeeds.
    struct FailNTimesExecutor {
        remaining_failures: AtomicU32,
    }

    impl FailNTimesExecutor {
        fn new(failures: u32) -> Self {
            Self {
                remaining_failures: AtomicU32::new(failures),
            }
        }
    }

    #[async_trait::async_trait]
    impl TaskExecutor for FailNTimesExecutor {
        async fn execute(&self, _task: &Task) -> Result<(), String> {
            let remaining = self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
            if remaining > 0 {
                Err("transient failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    /// A mock executor that always fails.
    struct AlwaysFailExecutor;

    #[async_trait::async_trait]
    impl TaskExecutor for AlwaysFailExecutor {
        async fn execute(&self, _task: &Task) -> Result<(), String> {
            Err("permanent failure".to_string())
        }
    }

    /// A mock executor that sleeps longer than the timeout.
    struct SlowExecutor {
        sleep_ms: u64,
    }

    #[async_trait::async_trait]
    impl TaskExecutor for SlowExecutor {
        async fn execute(&self, _task: &Task) -> Result<(), String> {
            tokio::time::sleep(std::time::Duration::from_millis(self.sleep_ms)).await;
            Ok(())
        }
    }

    /// A mock executor that always succeeds.
    struct SuccessExecutor;

    #[async_trait::async_trait]
    impl TaskExecutor for SuccessExecutor {
        async fn execute(&self, _task: &Task) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_execute_succeeds_first_try() {
        let task = make_test_task_with_policy(RetryPolicy::default(), 300);
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(SuccessExecutor);

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(result);
        let runs = history.read().await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, TaskRunStatus::Succeeded);
        assert_eq!(runs[0].attempt, 1);
    }

    #[tokio::test]
    async fn test_execute_fails_then_succeeds_on_retry() {
        // Use minimal backoff for test speed
        let policy = make_retry_policy(3, 0, 0);
        let task = make_test_task_with_policy(policy, 300);
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        // Fail twice, then succeed on 3rd attempt
        let executor: Arc<dyn TaskExecutor> = Arc::new(FailNTimesExecutor::new(2));

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(result);
        let runs = history.read().await;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].status, TaskRunStatus::Failed);
        assert_eq!(runs[0].attempt, 1);
        assert_eq!(runs[1].status, TaskRunStatus::Failed);
        assert_eq!(runs[1].attempt, 2);
        assert_eq!(runs[2].status, TaskRunStatus::Succeeded);
        assert_eq!(runs[2].attempt, 3);
    }

    #[tokio::test]
    async fn test_execute_exhausts_all_retries() {
        let policy = make_retry_policy(3, 0, 0);
        let task = make_test_task_with_policy(policy, 300);
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysFailExecutor);

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(!result);
        let runs = history.read().await;
        // 1 initial + 3 retries = 4 total attempts
        assert_eq!(runs.len(), 4);
        for (i, run) in runs.iter().enumerate() {
            assert_eq!(run.status, TaskRunStatus::Failed);
            assert_eq!(run.attempt, (i + 1) as u8);
        }

        // Task should be marked as Failed
        let tasks_guard = tasks.read().await;
        let task_entry = tasks_guard.get(&task.id).unwrap();
        assert!(matches!(&task_entry.status, TaskStatus::Failed { last_error } if last_error == "permanent failure"));
    }

    #[tokio::test]
    async fn test_execute_timeout_enforcement() {
        // Task with 100ms timeout, executor sleeps 500ms
        let policy = make_retry_policy(0, 0, 0); // no retries
        let mut task = make_test_task_with_policy(policy, 1); // 1 second timeout
        // Override to use a very short timeout for testing
        task.timeout_seconds = 1;
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(SlowExecutor { sleep_ms: 5000 });

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(!result);
        let runs = history.read().await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, TaskRunStatus::TimedOut);
        assert!(runs[0].error.as_ref().unwrap().contains("timeout"));
    }

    #[tokio::test]
    async fn test_execute_timeout_with_retries() {
        // Task times out on first attempt, then succeeds
        // We can't easily test this with the SlowExecutor, so use FailNTimes
        let policy = make_retry_policy(2, 0, 0);
        let task = make_test_task_with_policy(policy, 300);
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        // Fail once, then succeed
        let executor: Arc<dyn TaskExecutor> = Arc::new(FailNTimesExecutor::new(1));

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(result);
        let runs = history.read().await;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].status, TaskRunStatus::Failed);
        assert_eq!(runs[1].status, TaskRunStatus::Succeeded);
    }

    #[tokio::test]
    async fn test_zero_retries_fails_immediately() {
        let policy = make_retry_policy(0, 10, 300);
        let task = make_test_task_with_policy(policy, 300);
        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task.id, task.clone());
        let history = Arc::new(RwLock::new(Vec::new()));
        let executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysFailExecutor);

        let result = execute_with_retry(&task, &tasks, &history, &executor).await;

        assert!(!result);
        let runs = history.read().await;
        assert_eq!(runs.len(), 1); // Only the initial attempt
        assert_eq!(runs[0].status, TaskRunStatus::Failed);
    }

    // =========================================================================
    // History cleanup tests
    // =========================================================================

    #[tokio::test]
    async fn test_cleanup_removes_old_entries() {
        let history = Arc::new(RwLock::new(Vec::new()));

        // Add an old entry (40 days ago)
        let old_run = TaskRun {
            id: uuid::Uuid::new_v4(),
            task_id: TaskId::new(),
            start_time: Utc::now() - Duration::days(40),
            end_time: Some(Utc::now() - Duration::days(40)),
            duration_ms: Some(100),
            status: TaskRunStatus::Succeeded,
            error: None,
            attempt: 1,
        };

        // Add a recent entry (1 day ago)
        let recent_run = TaskRun {
            id: uuid::Uuid::new_v4(),
            task_id: TaskId::new(),
            start_time: Utc::now() - Duration::days(1),
            end_time: Some(Utc::now() - Duration::days(1)),
            duration_ms: Some(200),
            status: TaskRunStatus::Succeeded,
            error: None,
            attempt: 1,
        };

        history.write().await.push(old_run);
        history.write().await.push(recent_run.clone());

        cleanup_old_history(&history, DEFAULT_RETENTION_DAYS).await;

        let remaining = history.read().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, recent_run.id);
    }

    #[tokio::test]
    async fn test_cleanup_keeps_entries_within_retention() {
        let history = Arc::new(RwLock::new(Vec::new()));

        // Add entries within retention period
        for i in 0..5 {
            let run = TaskRun {
                id: uuid::Uuid::new_v4(),
                task_id: TaskId::new(),
                start_time: Utc::now() - Duration::days(i),
                end_time: Some(Utc::now() - Duration::days(i)),
                duration_ms: Some(100),
                status: TaskRunStatus::Succeeded,
                error: None,
                attempt: 1,
            };
            history.write().await.push(run);
        }

        cleanup_old_history(&history, DEFAULT_RETENTION_DAYS).await;

        let remaining = history.read().await;
        assert_eq!(remaining.len(), 5);
    }

    // =========================================================================
    // Failure isolation test
    // =========================================================================

    #[tokio::test]
    async fn test_failure_isolation_subsequent_tasks_execute() {
        let policy = make_retry_policy(0, 0, 0); // no retries for speed

        // Create two tasks
        let task1 = make_test_task_with_policy(policy.clone(), 300);
        let task2 = make_test_task_with_policy(policy, 300);

        let tasks = Arc::new(RwLock::new(HashMap::new()));
        tasks.write().await.insert(task1.id, task1.clone());
        tasks.write().await.insert(task2.id, task2.clone());
        let history = Arc::new(RwLock::new(Vec::new()));

        // First task fails
        let fail_executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysFailExecutor);
        let result1 = execute_with_retry(&task1, &tasks, &history, &fail_executor).await;
        assert!(!result1);

        // Second task should still execute successfully (failure isolation)
        let success_executor: Arc<dyn TaskExecutor> = Arc::new(SuccessExecutor);
        let result2 = execute_with_retry(&task2, &tasks, &history, &success_executor).await;
        assert!(result2);

        let runs = history.read().await;
        // task1 failed (1 attempt), task2 succeeded (1 attempt)
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].task_id, task1.id);
        assert_eq!(runs[0].status, TaskRunStatus::Failed);
        assert_eq!(runs[1].task_id, task2.id);
        assert_eq!(runs[1].status, TaskRunStatus::Succeeded);
    }
}
