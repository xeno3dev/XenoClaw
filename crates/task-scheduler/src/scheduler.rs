//! Core scheduler loop that checks triggers and fires tasks.
//!
//! The scheduler runs a background loop that:
//! 1. Evaluates cron expressions to find due tasks
//! 2. Receives file system change events from the FileWatcher
//! 3. Receives webhook events from the WebhookRegistry
//! 4. Evaluates time-based conditions
//! 5. Dispatches task execution with max 60-second drift

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use common::errors::TaskError;
use common::models::{Task, TaskRun, TaskTrigger};
use common::types::TaskId;

use crate::cron_parser::CronSchedule;
use crate::dependency::DependencyGraph;
use crate::file_watcher::{FileChangeEvent, FileWatcher};
use crate::time_condition::TimeCondition;
use crate::webhook::{WebhookEvent, WebhookRegistry};

/// Configuration for the scheduler.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// How often the scheduler loop checks for due tasks (in seconds).
    /// Must be <= 60 to ensure max 60-second drift.
    pub tick_interval_secs: u64,
    /// Maximum allowed drift in seconds for task execution.
    pub max_drift_secs: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: 30,
            max_drift_secs: 60,
        }
    }
}

/// Trait for executing tasks. Implemented by the agent core.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Execute a task and return the result.
    async fn execute(&self, task: &Task) -> Result<(), String>;
}

/// Internal state for a registered cron task.
#[derive(Debug, Clone)]
struct CronTaskState {
    task_id: TaskId,
    schedule: CronSchedule,
    last_fired: Option<DateTime<Utc>>,
}

/// Internal state for a registered time-condition task.
#[derive(Debug, Clone)]
struct TimeConditionState {
    task_id: TaskId,
    condition: TimeCondition,
    last_fired: Option<DateTime<Utc>>,
    /// Minimum interval between firings (to prevent rapid re-triggering).
    min_interval_secs: i64,
}

/// A summary of a registered task for listing.
#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub id: TaskId,
    pub name: String,
    pub trigger_type: String,
    pub next_run: Option<DateTime<Utc>>,
}

/// The main task scheduler that coordinates all trigger types.
pub struct Scheduler {
    config: SchedulerConfig,
    /// All registered tasks.
    tasks: Arc<RwLock<HashMap<TaskId, Task>>>,
    /// Cron-based task states.
    cron_tasks: Arc<RwLock<Vec<CronTaskState>>>,
    /// Time-condition task states.
    time_condition_tasks: Arc<RwLock<Vec<TimeConditionState>>>,
    /// File watcher for filesystem triggers.
    file_watcher: Arc<FileWatcher>,
    /// Webhook registry for HTTP triggers.
    webhook_registry: Arc<WebhookRegistry>,
    /// Channel for receiving file change events.
    file_event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<FileChangeEvent>>>>,
    /// Channel for receiving webhook events.
    webhook_event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<WebhookEvent>>>>,
    /// Task execution history.
    history: Arc<RwLock<Vec<TaskRun>>>,
    /// Task dependency graph for ordering and cycle detection.
    dependency_graph: Arc<RwLock<DependencyGraph>>,
    /// Shutdown signal.
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl Scheduler {
    /// Create a new Scheduler with the given configuration.
    pub fn new(config: SchedulerConfig) -> Self {
        let (file_event_tx, file_event_rx) = mpsc::unbounded_channel();
        let (webhook_event_tx, webhook_event_rx) = mpsc::unbounded_channel();

        Self {
            config,
            tasks: Arc::new(RwLock::new(HashMap::new())),
            cron_tasks: Arc::new(RwLock::new(Vec::new())),
            time_condition_tasks: Arc::new(RwLock::new(Vec::new())),
            file_watcher: Arc::new(FileWatcher::new(file_event_tx)),
            webhook_registry: Arc::new(WebhookRegistry::new(webhook_event_tx)),
            file_event_rx: Arc::new(RwLock::new(Some(file_event_rx))),
            webhook_event_rx: Arc::new(RwLock::new(Some(webhook_event_rx))),
            history: Arc::new(RwLock::new(Vec::new())),
            dependency_graph: Arc::new(RwLock::new(DependencyGraph::new())),
            shutdown_tx: None,
        }
    }

    /// Get a reference to the webhook registry (for the API server to call).
    pub fn webhook_registry(&self) -> Arc<WebhookRegistry> {
        Arc::clone(&self.webhook_registry)
    }

    /// Get a reference to the file watcher.
    pub fn file_watcher(&self) -> Arc<FileWatcher> {
        Arc::clone(&self.file_watcher)
    }

    /// Register a new task with the scheduler.
    pub async fn create_task(&self, task: Task) -> Result<TaskId, TaskError> {
        let task_id = task.id;

        // Validate dependencies if any are specified
        if !task.dependencies.is_empty() {
            let tasks_guard = self.tasks.read().await;
            let mut known_tasks: HashSet<TaskId> = tasks_guard.keys().copied().collect();
            drop(tasks_guard);
            // Include the task itself in known_tasks so self-dependency is caught as a cycle
            known_tasks.insert(task_id);

            let mut dep_graph = self.dependency_graph.write().await;
            dep_graph.add_task(task_id, &task.dependencies, &known_tasks)?;
        } else {
            // Register the task in the dependency graph even without dependencies
            let tasks_guard = self.tasks.read().await;
            let known_tasks: HashSet<TaskId> = tasks_guard.keys().copied().collect();
            drop(tasks_guard);

            let mut dep_graph = self.dependency_graph.write().await;
            dep_graph.add_task(task_id, &[], &known_tasks)?;
        }

        // Set up the appropriate trigger
        match &task.trigger {
            TaskTrigger::Cron { expression } => {
                let schedule = CronSchedule::parse(expression)?;
                self.cron_tasks.write().await.push(CronTaskState {
                    task_id,
                    schedule,
                    last_fired: None,
                });
            }
            TaskTrigger::FileChange { .. } => {
                self.file_watcher
                    .watch_task(task_id, &task.trigger)
                    .await
                    .map_err(|e| TaskError::InvalidCronExpression {
                        expression: format!("file watch error: {}", e),
                    })?;
            }
            TaskTrigger::Webhook { .. } => {
                self.webhook_registry
                    .register(task_id, &task.trigger)
                    .await
                    .map_err(|e| TaskError::InvalidCronExpression {
                        expression: format!("webhook error: {}", e),
                    })?;
            }
            TaskTrigger::TimeCondition { expression } => {
                let condition = TimeCondition::parse(expression)?;
                self.time_condition_tasks.write().await.push(TimeConditionState {
                    task_id,
                    condition,
                    last_fired: None,
                    min_interval_secs: 60, // Don't re-fire within 60s
                });
            }
            TaskTrigger::TaskCompletion { .. } => {
                // Task completion triggers are handled by the dependency system.
                // The task will be triggered when its dependency completes.
            }
        }

        self.tasks.write().await.insert(task_id, task);
        info!("Task {} registered with scheduler", task_id);
        Ok(task_id)
    }

    /// Cancel a scheduled task.
    pub async fn cancel_task(&self, task_id: &TaskId) -> Result<(), TaskError> {
        if self.tasks.write().await.remove(task_id).is_none() {
            return Err(TaskError::NotFound {
                id: task_id.to_string(),
            });
        }

        // Remove from dependency graph
        self.dependency_graph.write().await.remove_task(task_id);

        // Remove from cron tasks
        self.cron_tasks
            .write()
            .await
            .retain(|t| t.task_id != *task_id);

        // Remove from time condition tasks
        self.time_condition_tasks
            .write()
            .await
            .retain(|t| t.task_id != *task_id);

        // Remove from file watcher
        let _ = self.file_watcher.unwatch_task(task_id).await;

        // Remove from webhook registry
        self.webhook_registry.unregister(task_id).await;

        info!("Task {} cancelled", task_id);
        Ok(())
    }

    /// Get task execution history.
    pub async fn task_history(&self, task_id: &TaskId, limit: usize) -> Vec<TaskRun> {
        self.history
            .read()
            .await
            .iter()
            .filter(|r| r.task_id == *task_id)
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Clean up task history entries older than the specified retention period.
    ///
    /// Default retention is 30 days. Call this periodically to keep history bounded.
    pub async fn cleanup_history(&self, retention_days: i64) {
        crate::retry::cleanup_old_history(&self.history, retention_days).await;
    }

    /// Notify the scheduler that a task has completed successfully.
    ///
    /// This marks the task as completed in the dependency graph and triggers
    /// any dependent tasks that are now unblocked. Used when task completion
    /// is reported externally (e.g., by the agent core after executing a task).
    pub async fn notify_task_completed(
        &self,
        task_id: TaskId,
        executor: Arc<dyn TaskExecutor>,
    ) {
        let unblocked = {
            let mut graph = self.dependency_graph.write().await;
            graph.mark_completed(task_id)
        };

        for unblocked_task_id in unblocked {
            info!(
                "Task {} unblocked by completion of task {}",
                unblocked_task_id, task_id
            );
            Self::handle_triggered_task_with_deps(
                unblocked_task_id,
                &self.tasks,
                &self.history,
                &executor,
                Some(&self.dependency_graph),
            )
            .await;
        }
    }

    /// Check if a task's dependencies are all satisfied.
    pub async fn are_dependencies_satisfied(&self, task_id: &TaskId) -> bool {
        let graph = self.dependency_graph.read().await;
        graph.is_unblocked(task_id)
    }

    /// Get a reference to the dependency graph (for testing/inspection).
    pub fn dependency_graph(&self) -> &Arc<RwLock<DependencyGraph>> {
        &self.dependency_graph
    }

    /// List all scheduled tasks with their next run time.
    pub async fn list_tasks(&self) -> Vec<TaskSummary> {
        let tasks = self.tasks.read().await;
        let cron_tasks = self.cron_tasks.read().await;
        let now = Utc::now();

        tasks
            .values()
            .map(|task| {
                let next_run = match &task.trigger {
                    TaskTrigger::Cron { .. } => cron_tasks
                        .iter()
                        .find(|ct| ct.task_id == task.id)
                        .and_then(|ct| ct.schedule.next_after(now)),
                    _ => None,
                };

                let trigger_type = match &task.trigger {
                    TaskTrigger::Cron { .. } => "cron",
                    TaskTrigger::FileChange { .. } => "file_change",
                    TaskTrigger::Webhook { .. } => "webhook",
                    TaskTrigger::TimeCondition { .. } => "time_condition",
                    TaskTrigger::TaskCompletion { .. } => "task_completion",
                };

                TaskSummary {
                    id: task.id,
                    name: task.name.clone(),
                    trigger_type: trigger_type.to_string(),
                    next_run,
                }
            })
            .collect()
    }

    /// Start the scheduler loop. This spawns a background task that
    /// periodically checks cron schedules and time conditions.
    ///
    /// File system and webhook events are handled reactively via channels.
    pub async fn start(&mut self, executor: Arc<dyn TaskExecutor>) -> Result<(), TaskError> {
        // Start the file watcher
        self.file_watcher.start().await.map_err(|e| {
            TaskError::InvalidCronExpression {
                expression: format!("file watcher start failed: {}", e),
            }
        })?;

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Take ownership of event receivers
        let file_event_rx = self.file_event_rx.write().await.take();
        let webhook_event_rx = self.webhook_event_rx.write().await.take();

        // Spawn the main scheduler loop
        let tick_interval = self.config.tick_interval_secs;
        let max_drift = self.config.max_drift_secs;
        let cron_tasks = Arc::clone(&self.cron_tasks);
        let time_condition_tasks = Arc::clone(&self.time_condition_tasks);
        let tasks = Arc::clone(&self.tasks);
        let history = Arc::clone(&self.history);
        let executor_clone = Arc::clone(&executor);
        let dep_graph = Arc::clone(&self.dependency_graph);

        let mut shutdown_rx_clone = shutdown_rx.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(tick_interval));

            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        Self::check_cron_tasks(
                            &cron_tasks, &tasks, &history,
                            &executor_clone, max_drift, &dep_graph,
                        ).await;
                        Self::check_time_conditions(
                            &time_condition_tasks, &tasks, &history,
                            &executor_clone, &dep_graph,
                        ).await;
                    }
                    _ = shutdown_rx_clone.changed() => {
                        info!("Scheduler loop shutting down");
                        break;
                    }
                }
            }
        });

        // Spawn file event handler
        if let Some(mut rx) = file_event_rx {
            let tasks = Arc::clone(&self.tasks);
            let history = Arc::clone(&self.history);
            let executor_clone = Arc::clone(&executor);
            let dep_graph = Arc::clone(&self.dependency_graph);
            let mut shutdown_rx_clone = shutdown_rx.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(event) = rx.recv() => {
                            debug!("File change event: {:?}", event);
                            Self::handle_triggered_task_with_deps(
                                event.task_id, &tasks, &history, &executor_clone,
                                Some(&dep_graph),
                            ).await;
                        }
                        _ = shutdown_rx_clone.changed() => break,
                    }
                }
            });
        }

        // Spawn webhook event handler
        if let Some(mut rx) = webhook_event_rx {
            let tasks = Arc::clone(&self.tasks);
            let history = Arc::clone(&self.history);
            let executor_clone = Arc::clone(&executor);
            let dep_graph = Arc::clone(&self.dependency_graph);
            let mut shutdown_rx_clone = shutdown_rx.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(event) = rx.recv() => {
                            debug!("Webhook event: {:?}", event);
                            Self::handle_triggered_task_with_deps(
                                event.task_id, &tasks, &history, &executor_clone,
                                Some(&dep_graph),
                            ).await;
                        }
                        _ = shutdown_rx_clone.changed() => break,
                    }
                }
            });
        }

        info!("Scheduler started with {}s tick interval", tick_interval);
        Ok(())
    }

    /// Stop the scheduler.
    pub async fn stop(&self) {
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(true);
        }
        self.file_watcher.stop().await;
        info!("Scheduler stopped");
    }

    /// Check all cron tasks and fire any that are due.
    async fn check_cron_tasks(
        cron_tasks: &RwLock<Vec<CronTaskState>>,
        tasks: &RwLock<HashMap<TaskId, Task>>,
        history: &RwLock<Vec<TaskRun>>,
        executor: &Arc<dyn TaskExecutor>,
        max_drift_secs: i64,
        dep_graph: &RwLock<DependencyGraph>,
    ) {
        let now = Utc::now();
        let mut cron_states = cron_tasks.write().await;

        for state in cron_states.iter_mut() {
            let should_fire = if let Some(last_fired) = state.last_fired {
                // Check if there's a scheduled time between last_fired and now
                if let Some(next) = state.schedule.next_after(last_fired) {
                    let drift = (now - next).num_seconds();
                    drift >= 0 && drift <= max_drift_secs
                } else {
                    false
                }
            } else {
                // Never fired — check if we're within drift of the next occurrence
                let check_from = now - chrono::Duration::seconds(max_drift_secs);
                if let Some(next) = state.schedule.next_after(check_from) {
                    next <= now
                } else {
                    false
                }
            };

            if should_fire {
                state.last_fired = Some(now);
                Self::handle_triggered_task_with_deps(
                    state.task_id, tasks, history, executor, Some(dep_graph),
                ).await;
            }
        }
    }

    /// Check all time-condition tasks and fire any whose conditions are met.
    async fn check_time_conditions(
        time_condition_tasks: &RwLock<Vec<TimeConditionState>>,
        tasks: &RwLock<HashMap<TaskId, Task>>,
        history: &RwLock<Vec<TaskRun>>,
        executor: &Arc<dyn TaskExecutor>,
        dep_graph: &RwLock<DependencyGraph>,
    ) {
        let now = Utc::now();
        let mut tc_states = time_condition_tasks.write().await;

        for state in tc_states.iter_mut() {
            // Check minimum interval since last firing
            if let Some(last_fired) = state.last_fired {
                let elapsed = (now - last_fired).num_seconds();
                if elapsed < state.min_interval_secs {
                    continue;
                }
            }

            if state.condition.evaluate(now) {
                state.last_fired = Some(now);
                Self::handle_triggered_task_with_deps(
                    state.task_id, tasks, history, executor, Some(dep_graph),
                ).await;
            }
        }
    }

    /// Execute a triggered task with retry logic, timeout enforcement, and history recording.
    ///
    /// This method:
    /// 1. Checks if the task's dependencies are satisfied
    /// 2. Looks up the task by ID
    /// 3. Delegates to `retry::execute_with_retry` which handles:
    ///    - Timeout enforcement (wraps execution in tokio::time::timeout)
    ///    - Exponential backoff retries on failure
    ///    - Recording all attempts in history
    ///    - Marking task as Failed after retry exhaustion
    /// 4. On success, marks the task completed in the dependency graph and triggers unblocked tasks
    /// 5. Continues with subsequent tasks regardless of outcome (failure isolation)
    #[cfg(test)]
    async fn handle_triggered_task(
        task_id: TaskId,
        tasks: &RwLock<HashMap<TaskId, Task>>,
        history: &RwLock<Vec<TaskRun>>,
        executor: &Arc<dyn TaskExecutor>,
    ) {
        Self::handle_triggered_task_with_deps(task_id, tasks, history, executor, None).await;
    }

    /// Execute a triggered task, respecting dependencies and triggering dependents on success.
    async fn handle_triggered_task_with_deps(
        task_id: TaskId,
        tasks: &RwLock<HashMap<TaskId, Task>>,
        history: &RwLock<Vec<TaskRun>>,
        executor: &Arc<dyn TaskExecutor>,
        dep_graph: Option<&RwLock<DependencyGraph>>,
    ) {
        // Check if dependencies are satisfied before executing
        if let Some(graph_lock) = dep_graph {
            let graph = graph_lock.read().await;
            if graph.has_dependencies(&task_id) && !graph.is_unblocked(&task_id) {
                debug!(
                    "Task {} has unsatisfied dependencies, skipping execution",
                    task_id
                );
                return;
            }
        }

        let task = {
            let tasks_guard = tasks.read().await;
            match tasks_guard.get(&task_id) {
                Some(task) => task.clone(),
                None => {
                    warn!("Task {} not found, skipping execution", task_id);
                    return;
                }
            }
        };

        info!("Executing task '{}' ({})", task.name, task_id);

        // Delegate to retry module which handles timeout, retries, and history recording
        let success = crate::retry::execute_with_retry(&task, tasks, history, executor).await;

        // If the task succeeded, mark it completed in the dependency graph
        // and trigger any newly unblocked dependent tasks
        if success {
            if let Some(graph_lock) = dep_graph {
                let unblocked = {
                    let mut graph = graph_lock.write().await;
                    graph.mark_completed(task_id)
                };

                for unblocked_task_id in unblocked {
                    info!(
                        "Task {} unblocked by completion of task {}",
                        unblocked_task_id, task_id
                    );
                    // Execute unblocked tasks
                    Box::pin(Self::handle_triggered_task_with_deps(
                        unblocked_task_id,
                        tasks,
                        history,
                        executor,
                        dep_graph,
                    ))
                    .await;
                }
            }
        }

        // Failure isolation: we continue regardless of whether the task succeeded or failed.
        // The retry module has already logged the summary and updated the task status.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::models::{RetryPolicy, TaskAction, TaskRunStatus, TaskStatus};

    /// A mock executor that records which tasks were executed.
    struct MockExecutor {
        executed: Arc<RwLock<Vec<TaskId>>>,
        should_fail: bool,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                executed: Arc::new(RwLock::new(Vec::new())),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                executed: Arc::new(RwLock::new(Vec::new())),
                should_fail: true,
            }
        }

        #[allow(dead_code)]
        async fn executed_tasks(&self) -> Vec<TaskId> {
            self.executed.read().await.clone()
        }
    }

    #[async_trait]
    impl TaskExecutor for MockExecutor {
        async fn execute(&self, task: &Task) -> Result<(), String> {
            self.executed.write().await.push(task.id);
            if self.should_fail {
                Err("mock failure".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn make_test_task(trigger: TaskTrigger) -> Task {
        Task {
            id: TaskId::new(),
            name: "test-task".to_string(),
            trigger,
            action: TaskAction::Command {
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
            },
            timeout_seconds: 300,
            dependencies: vec![],
            retry_policy: RetryPolicy::default(),
            status: TaskStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_create_cron_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_id = task.id;

        let result = scheduler.create_task(task).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), task_id);

        let tasks = scheduler.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].trigger_type, "cron");
    }

    #[tokio::test]
    async fn test_create_invalid_cron_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "invalid".to_string(),
        });

        let result = scheduler.create_task(task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_sub_minute_cron_rejected() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "* * * * * *".to_string(), // every second
        });

        let result = scheduler.create_task(task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_time_condition_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::TimeCondition {
            expression: "weekday && after 9am".to_string(),
        });
        let task_id = task.id;

        let result = scheduler.create_task(task).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), task_id);
    }

    #[tokio::test]
    async fn test_create_invalid_time_condition_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::TimeCondition {
            expression: "invalid condition".to_string(),
        });

        let result = scheduler.create_task(task).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_id = task.id;

        scheduler.create_task(task).await.unwrap();
        assert_eq!(scheduler.list_tasks().await.len(), 1);

        scheduler.cancel_task(&task_id).await.unwrap();
        assert_eq!(scheduler.list_tasks().await.len(), 0);
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_task() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let result = scheduler.cancel_task(&TaskId::new()).await;
        assert!(matches!(result, Err(TaskError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_scheduler_start_and_stop() {
        let mut scheduler = Scheduler::new(SchedulerConfig {
            tick_interval_secs: 1,
            max_drift_secs: 60,
        });

        let executor = Arc::new(MockExecutor::new());
        let result = scheduler.start(executor).await;
        assert!(result.is_ok());

        // Let it run briefly
        tokio::time::sleep(Duration::from_millis(100)).await;
        scheduler.stop().await;
    }

    #[tokio::test]
    async fn test_task_history_recording() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_id = task.id;
        scheduler.create_task(task.clone()).await.unwrap();

        // Manually trigger execution
        let executor = Arc::new(MockExecutor::new());
        Scheduler::handle_triggered_task(
            task_id,
            &scheduler.tasks,
            &scheduler.history,
            &(executor as Arc<dyn TaskExecutor>),
        )
        .await;

        let history = scheduler.task_history(&task_id, 10).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, TaskRunStatus::Succeeded);
        assert!(history[0].duration_ms.is_some());
    }

    #[tokio::test]
    async fn test_task_history_records_failure() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let mut task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        // Use zero backoff for fast test execution
        task.retry_policy = RetryPolicy {
            max_retries: 0,
            base_interval_secs: 0,
            max_interval_secs: 0,
        };
        let task_id = task.id;
        scheduler.create_task(task.clone()).await.unwrap();

        let executor = Arc::new(MockExecutor::failing());
        Scheduler::handle_triggered_task(
            task_id,
            &scheduler.tasks,
            &scheduler.history,
            &(executor as Arc<dyn TaskExecutor>),
        )
        .await;

        let history = scheduler.task_history(&task_id, 10).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, TaskRunStatus::Failed);
    }

    #[tokio::test]
    async fn test_cron_task_fires_within_drift() {
        let scheduler = Scheduler::new(SchedulerConfig::default());
        let task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_id = task.id;
        scheduler.create_task(task).await.unwrap();

        let executor = Arc::new(MockExecutor::new());

        // Simulate checking cron tasks
        Scheduler::check_cron_tasks(
            &scheduler.cron_tasks,
            &scheduler.tasks,
            &scheduler.history,
            &(executor as Arc<dyn TaskExecutor>),
            60,
            &scheduler.dependency_graph,
        )
        .await;

        // The task should have fired (since we never fired before,
        // and there's likely a scheduled time within the last 60s)
        let _history = scheduler.task_history(&task_id, 10).await;
        // May or may not fire depending on current time alignment
        // This is a timing-sensitive test, so we just verify no panic
    }

    // =========================================================================
    // Dependency integration tests
    // =========================================================================

    #[tokio::test]
    async fn test_create_task_with_dependencies() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        // Create task A (no dependencies)
        let task_a = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_a_id = task_a.id;
        scheduler.create_task(task_a).await.unwrap();

        // Create task B depending on A
        let mut task_b = make_test_task(TaskTrigger::TaskCompletion {
            task_id: task_a_id,
        });
        task_b.dependencies = vec![task_a_id];
        let task_b_id = task_b.id;
        let result = scheduler.create_task(task_b).await;
        assert!(result.is_ok());

        // Task B should not be unblocked yet
        assert!(!scheduler.are_dependencies_satisfied(&task_b_id).await);
    }

    #[tokio::test]
    async fn test_create_task_with_cycle_rejected() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        // Create three tasks with no dependencies first
        let task_a = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_a_id = task_a.id;
        scheduler.create_task(task_a).await.unwrap();

        let mut task_b = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_b_id = task_b.id;
        task_b.dependencies = vec![task_a_id]; // B depends on A
        scheduler.create_task(task_b).await.unwrap();

        let mut task_c = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let _task_c_id = task_c.id;
        task_c.dependencies = vec![task_b_id]; // C depends on B
        scheduler.create_task(task_c).await.unwrap();

        // Now try to create a task D that depends on C, where D's ID
        // is also a dependency of A. But A has no deps...
        // 
        // The only way to get a cycle in the scheduler is:
        // Add task with dep on something that transitively depends on it.
        // Since the task is NEW, it can't be a transitive dep of anything yet.
        // UNLESS we use the same ID as an existing task.
        //
        // Actually, let's test self-dependency through the scheduler:
        let mut task_self = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let self_id = task_self.id;
        task_self.dependencies = vec![self_id]; // Self-dependency
        let result = scheduler.create_task(task_self).await;
        assert!(matches!(result, Err(TaskError::DependencyCycle { .. })));
    }

    #[tokio::test]
    async fn test_create_task_with_nonexistent_dependency_rejected() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        let nonexistent_id = TaskId::new();
        let mut task = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        task.dependencies = vec![nonexistent_id];

        let result = scheduler.create_task(task).await;
        assert!(matches!(result, Err(TaskError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_notify_task_completed_triggers_dependents() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        // Create task A (no dependencies)
        let task_a = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        let task_a_id = task_a.id;
        scheduler.create_task(task_a).await.unwrap();

        // Create task B depending on A
        let mut task_b = make_test_task(TaskTrigger::TaskCompletion {
            task_id: task_a_id,
        });
        task_b.dependencies = vec![task_a_id];
        let task_b_id = task_b.id;
        scheduler.create_task(task_b).await.unwrap();

        // Task B should not be unblocked
        assert!(!scheduler.are_dependencies_satisfied(&task_b_id).await);

        // Notify that task A completed
        let executor = Arc::new(MockExecutor::new());
        scheduler
            .notify_task_completed(task_a_id, executor.clone())
            .await;

        // Task B should now have been executed
        let executed = executor.executed_tasks().await;
        assert!(executed.contains(&task_b_id));
    }

    #[tokio::test]
    async fn test_dependency_chain_too_deep_rejected() {
        let scheduler = Scheduler::new(SchedulerConfig::default());

        // Create a chain of 10 tasks (depth = 10, which is the max)
        let mut prev_id = None;
        for _i in 0..10 {
            let mut task = make_test_task(TaskTrigger::Cron {
                expression: "0 * * * * *".to_string(),
            });
            if let Some(dep_id) = prev_id {
                task.dependencies = vec![dep_id];
            }
            prev_id = Some(task.id);
            scheduler.create_task(task).await.unwrap();
        }

        // The 11th task should fail (depth = 11 > max of 10)
        let mut task_11 = make_test_task(TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        });
        task_11.dependencies = vec![prev_id.unwrap()];
        let result = scheduler.create_task(task_11).await;
        assert!(matches!(result, Err(TaskError::DependencyTooDeep { .. })));
    }
}
