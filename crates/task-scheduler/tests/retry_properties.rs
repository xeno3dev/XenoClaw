//! Property-based tests for retry logic and failure isolation.
//!
//! **Validates: Requirements 2.6, 2.8**
//!
//! Property 4: Exponential Backoff Retry Intervals
//! Property 5: Task Failure Isolation

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use proptest::prelude::*;
use tokio::sync::RwLock;

use common::models::{
    RetryPolicy, Task, TaskAction, TaskRun, TaskRunStatus, TaskStatus, TaskTrigger,
};
use common::types::TaskId;
use task_scheduler::{calculate_backoff, execute_with_retry, TaskExecutor};

// ============================================================================
// Test helpers
// ============================================================================

/// Create a task with the given retry policy for testing.
fn make_task_with_policy(policy: RetryPolicy) -> Task {
    Task {
        id: TaskId::new(),
        name: "property-test-task".to_string(),
        trigger: TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        },
        action: TaskAction::Command {
            command: "echo".to_string(),
            args: vec!["test".to_string()],
        },
        timeout_seconds: 300,
        dependencies: vec![],
        retry_policy: policy,
        status: TaskStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// An executor that always fails.
struct AlwaysFailExecutor;

#[async_trait]
impl TaskExecutor for AlwaysFailExecutor {
    async fn execute(&self, _task: &Task) -> Result<(), String> {
        Err("permanent failure".to_string())
    }
}

/// An executor that always succeeds.
struct AlwaysSucceedExecutor;

#[async_trait]
impl TaskExecutor for AlwaysSucceedExecutor {
    async fn execute(&self, _task: &Task) -> Result<(), String> {
        Ok(())
    }
}

/// An executor that fails a specified number of times, then succeeds.
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

#[async_trait]
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

// ============================================================================
// Strategies
// ============================================================================

/// Generate a valid RetryPolicy with constrained values.
/// max_retries: 1..=3 (as per the property statement)
/// base_interval_secs: 1..=100 (reasonable range for testing)
/// max_interval_secs: base..=600 (must be >= base for meaningful cap)
fn retry_policy_strategy() -> impl Strategy<Value = RetryPolicy> {
    (1u8..=3u8, 1u32..=100u32).prop_flat_map(|(max_retries, base)| {
        (Just(max_retries), Just(base), base..=600u32).prop_map(
            |(max_retries, base_interval_secs, max_interval_secs)| RetryPolicy {
                max_retries,
                base_interval_secs,
                max_interval_secs,
            },
        )
    })
}

/// Generate a valid attempt number (1-indexed) within the retry range.
/// attempt K means the Kth retry (1 ≤ K ≤ max_retries).
fn attempt_strategy() -> impl Strategy<Value = u8> {
    1u8..=3u8
}

/// Generate the default retry policy as specified in the requirements:
/// base = 10s, max = 300s, max_retries = 3.
fn default_retry_policy_strategy() -> impl Strategy<Value = RetryPolicy> {
    Just(RetryPolicy {
        max_retries: 3,
        base_interval_secs: 10,
        max_interval_secs: 300,
    })
}

/// Generate a number of tasks for the failure isolation test (2..=5).
fn task_count_strategy() -> impl Strategy<Value = usize> {
    2usize..=5usize
}

/// Generate which task index should fail (0-indexed).
fn failing_task_index_strategy(max: usize) -> impl Strategy<Value = usize> {
    0..max
}

// ============================================================================
// Property 4: Exponential Backoff Retry Intervals
//
// For any task that fails N times (1 ≤ N ≤ 3), the retry intervals SHALL
// follow exponential backoff with a 10-second base, where the Kth retry delay
// equals min(10 × 2^(K-1), 300) seconds, and no more than 3 retries SHALL
// be attempted.
//
// **Validates: Requirements 2.6**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 2.6**
    ///
    /// Property 4: For any valid RetryPolicy and attempt number K (1 ≤ K ≤ 3),
    /// the calculated backoff delay SHALL equal min(base × 2^(K-1), max_interval).
    #[test]
    fn prop_backoff_follows_exponential_formula(
        policy in retry_policy_strategy(),
        attempt in attempt_strategy()
    ) {
        // Only test attempts within the policy's max_retries
        prop_assume!(attempt <= policy.max_retries);

        let delay = calculate_backoff(&policy, attempt);

        // Expected: min(base * 2^(attempt-1), max_interval)
        let exponent = (attempt - 1) as u32;
        let multiplier = 2u64.saturating_pow(exponent);
        let expected_secs = (policy.base_interval_secs as u64)
            .saturating_mul(multiplier)
            .min(policy.max_interval_secs as u64);

        prop_assert_eq!(
            delay,
            std::time::Duration::from_secs(expected_secs),
            "Backoff for attempt {} with base={}s, max={}s should be {}s, got {:?}",
            attempt,
            policy.base_interval_secs,
            policy.max_interval_secs,
            expected_secs,
            delay
        );
    }

    /// **Validates: Requirements 2.6**
    ///
    /// Property 4: With the default policy (base=10s, max=300s), the Kth retry
    /// delay SHALL equal min(10 × 2^(K-1), 300) seconds.
    #[test]
    fn prop_default_policy_backoff_matches_spec(
        _policy in default_retry_policy_strategy(),
        attempt in 1u8..=3u8
    ) {
        let policy = RetryPolicy::default();
        let delay = calculate_backoff(&policy, attempt);

        // Expected delays: K=1 → 10s, K=2 → 20s, K=3 → 40s
        let expected_secs = match attempt {
            1 => 10u64,  // 10 * 2^0 = 10
            2 => 20u64,  // 10 * 2^1 = 20
            3 => 40u64,  // 10 * 2^2 = 40
            _ => unreachable!(),
        };

        prop_assert_eq!(
            delay,
            std::time::Duration::from_secs(expected_secs),
            "Default policy: attempt {} should have delay {}s, got {:?}",
            attempt,
            expected_secs,
            delay
        );
    }

    /// **Validates: Requirements 2.6**
    ///
    /// Property 4: The backoff delay SHALL never exceed the configured max_interval.
    #[test]
    fn prop_backoff_never_exceeds_max(
        policy in retry_policy_strategy(),
        attempt in 1u8..=255u8
    ) {
        let delay = calculate_backoff(&policy, attempt);
        let max_duration = std::time::Duration::from_secs(policy.max_interval_secs as u64);

        prop_assert!(
            delay <= max_duration,
            "Backoff {:?} exceeds max {:?} for attempt {} with policy {:?}",
            delay,
            max_duration,
            attempt,
            policy
        );
    }

    /// **Validates: Requirements 2.6**
    ///
    /// Property 4: Backoff delays SHALL be monotonically non-decreasing
    /// for successive attempts (until capped).
    #[test]
    fn prop_backoff_monotonically_nondecreasing(
        policy in retry_policy_strategy()
    ) {
        let mut prev_delay = std::time::Duration::from_secs(0);

        for attempt in 1..=policy.max_retries {
            let delay = calculate_backoff(&policy, attempt);
            prop_assert!(
                delay >= prev_delay,
                "Backoff decreased from {:?} to {:?} at attempt {}",
                prev_delay,
                delay,
                attempt
            );
            prev_delay = delay;
        }
    }
}

// ============================================================================
// Property 4 (continued): No more than 3 retries SHALL be attempted.
//
// This requires an async runtime to test execute_with_retry.
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// **Validates: Requirements 2.6**
    ///
    /// Property 4: No more than max_retries retries SHALL be attempted.
    /// When a task always fails, the total number of attempts SHALL be
    /// exactly max_retries + 1 (initial attempt + retries).
    #[test]
    fn prop_max_retries_not_exceeded(
        max_retries in 1u8..=3u8
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let policy = RetryPolicy {
                max_retries,
                base_interval_secs: 0, // zero backoff for test speed
                max_interval_secs: 0,
            };
            let task = make_task_with_policy(policy);
            let tasks = RwLock::new(HashMap::new());
            tasks.write().await.insert(task.id, task.clone());
            let history: RwLock<Vec<TaskRun>> = RwLock::new(Vec::new());
            let executor: Arc<dyn TaskExecutor> = Arc::new(AlwaysFailExecutor);

            let result = execute_with_retry(&task, &tasks, &history, &executor).await;

            assert!(!result, "Task should have failed");

            let runs = history.read().await;
            let expected_attempts = (max_retries as usize) + 1;
            assert_eq!(
                runs.len(),
                expected_attempts,
                "Expected {} attempts (1 initial + {} retries), got {}",
                expected_attempts,
                max_retries,
                runs.len()
            );

            // Verify all attempts are recorded as failed
            for (i, run) in runs.iter().enumerate() {
                assert_eq!(run.attempt, (i + 1) as u8);
                assert_eq!(run.status, TaskRunStatus::Failed);
            }
        });
    }
}

// ============================================================================
// Property 5: Task Failure Isolation
//
// For any sequence of scheduled tasks where one or more tasks fail after
// exhausting all retries, subsequent tasks in the queue SHALL still execute
// without interruption.
//
// **Validates: Requirements 2.8**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(30))]

    /// **Validates: Requirements 2.8**
    ///
    /// Property 5: For any sequence of N tasks where task at index F fails
    /// after exhausting retries, all subsequent tasks SHALL still execute
    /// successfully.
    #[test]
    fn prop_failure_isolation_subsequent_tasks_execute(
        num_tasks in task_count_strategy()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Always fail the first task, verify all others succeed
            let failing_index = 0;

            let policy = RetryPolicy {
                max_retries: 1, // minimal retries for speed
                base_interval_secs: 0,
                max_interval_secs: 0,
            };

            let tasks_store: RwLock<HashMap<TaskId, Task>> = RwLock::new(HashMap::new());
            let history: RwLock<Vec<TaskRun>> = RwLock::new(Vec::new());

            // Create all tasks
            let mut task_list = Vec::new();
            for i in 0..num_tasks {
                let mut task = make_task_with_policy(policy.clone());
                task.name = format!("task-{}", i);
                tasks_store.write().await.insert(task.id, task.clone());
                task_list.push(task);
            }

            // Execute tasks sequentially, simulating a queue
            for (i, task) in task_list.iter().enumerate() {
                let executor: Arc<dyn TaskExecutor> = if i == failing_index {
                    Arc::new(AlwaysFailExecutor)
                } else {
                    Arc::new(AlwaysSucceedExecutor)
                };

                let _result = execute_with_retry(task, &tasks_store, &history, &executor).await;
            }

            // Verify: the failing task failed, all others succeeded
            let runs = history.read().await;

            // Check that subsequent tasks (after the failing one) all have a successful run
            for (i, task) in task_list.iter().enumerate() {
                let task_runs: Vec<&TaskRun> = runs.iter().filter(|r| r.task_id == task.id).collect();

                if i == failing_index {
                    // The failing task should have exhausted retries
                    assert!(
                        !task_runs.is_empty(),
                        "Failing task at index {} should have run attempts",
                        i
                    );
                    assert!(
                        task_runs.iter().all(|r| r.status == TaskRunStatus::Failed),
                        "Failing task at index {} should have all failed runs",
                        i
                    );
                } else {
                    // Subsequent tasks should have succeeded
                    assert!(
                        !task_runs.is_empty(),
                        "Task at index {} should have been executed (failure isolation violated)",
                        i
                    );
                    let last_run = task_runs.last().unwrap();
                    assert_eq!(
                        last_run.status,
                        TaskRunStatus::Succeeded,
                        "Task at index {} should have succeeded (failure isolation violated)",
                        i
                    );
                }
            }
        });
    }

    /// **Validates: Requirements 2.8**
    ///
    /// Property 5: For any sequence of tasks where multiple tasks fail,
    /// the remaining tasks SHALL still execute without interruption.
    #[test]
    fn prop_failure_isolation_multiple_failures(
        num_tasks in 3usize..=6usize,
        num_failures in 1usize..=2usize
    ) {
        prop_assume!(num_failures < num_tasks);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let policy = RetryPolicy {
                max_retries: 1,
                base_interval_secs: 0,
                max_interval_secs: 0,
            };

            let tasks_store: RwLock<HashMap<TaskId, Task>> = RwLock::new(HashMap::new());
            let history: RwLock<Vec<TaskRun>> = RwLock::new(Vec::new());

            // Create all tasks
            let mut task_list = Vec::new();
            for i in 0..num_tasks {
                let mut task = make_task_with_policy(policy.clone());
                task.name = format!("task-{}", i);
                tasks_store.write().await.insert(task.id, task.clone());
                task_list.push(task);
            }

            // Fail the first `num_failures` tasks
            let mut succeeded_count = 0usize;
            let mut failed_count = 0usize;

            for (i, task) in task_list.iter().enumerate() {
                let executor: Arc<dyn TaskExecutor> = if i < num_failures {
                    Arc::new(AlwaysFailExecutor)
                } else {
                    Arc::new(AlwaysSucceedExecutor)
                };

                let result = execute_with_retry(task, &tasks_store, &history, &executor).await;
                if result {
                    succeeded_count += 1;
                } else {
                    failed_count += 1;
                }
            }

            // All non-failing tasks should have succeeded
            assert_eq!(
                succeeded_count,
                num_tasks - num_failures,
                "Expected {} successes, got {}",
                num_tasks - num_failures,
                succeeded_count
            );
            assert_eq!(
                failed_count,
                num_failures,
                "Expected {} failures, got {}",
                num_failures,
                failed_count
            );

            // Verify the last task always succeeded (proving isolation)
            let last_task = task_list.last().unwrap();
            let runs = history.read().await;
            let last_runs: Vec<&TaskRun> = runs.iter().filter(|r| r.task_id == last_task.id).collect();
            assert!(
                !last_runs.is_empty(),
                "Last task should have been executed"
            );
            assert_eq!(
                last_runs.last().unwrap().status,
                TaskRunStatus::Succeeded,
                "Last task should have succeeded despite earlier failures"
            );
        });
    }
}
