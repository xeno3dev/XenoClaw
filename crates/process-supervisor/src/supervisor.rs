//! Process Supervisor — the main supervision logic.
//!
//! Manages the AgentCore lifecycle within the same process via health checks
//! and restart logic. The supervisor runs a background monitoring loop that
//! periodically checks agent health and triggers restarts when needed.
//!
//! Requirements: 2.2, 2.7, 18.4

use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::time;
use tracing::{error, info, warn};

use crate::health::HealthMonitor;
use crate::stats::StatsTracker;
use crate::types::{
    HealthStatus, RestartReason, SupervisorConfig, SupervisorError, SupervisorStats,
};

/// Callback type for starting/restarting the agent.
///
/// The supervisor calls this function to start or restart the agent.
/// It should return `Ok(())` if the agent started successfully, or
/// an error string if it failed.
pub type AgentStartFn = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// Callback type for checking agent health.
///
/// Should return `true` if the agent is responsive, `false` otherwise.
pub type AgentHealthCheckFn = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync>;

/// Callback type for restoring session state on restart.
///
/// Called after a successful agent restart to restore previous state.
pub type StateRestoreFn = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> + Send + Sync>;

/// The Process Supervisor manages the agent's lifecycle, health monitoring,
/// and automatic recovery.
///
/// It supervises the AgentCore within the same process (not as a separate OS
/// process) via health checks and restart logic.
///
/// # Usage
///
/// ```ignore
/// let config = SupervisorConfig::default();
/// let supervisor = ProcessSupervisor::new(config);
///
/// // Register callbacks
/// supervisor.set_agent_start_fn(start_fn).await;
/// supervisor.set_health_check_fn(check_fn).await;
/// supervisor.set_state_restore_fn(restore_fn).await;
///
/// // Start supervision
/// supervisor.start().await?;
/// ```
pub struct ProcessSupervisor {
    /// Supervisor configuration.
    config: SupervisorConfig,

    /// Health monitor tracking agent responsiveness.
    health_monitor: Arc<HealthMonitor>,

    /// Statistics tracker for uptime and restart history.
    stats: Arc<StatsTracker>,

    /// Whether the supervisor is currently running.
    running: Arc<RwLock<bool>>,

    /// Shutdown signal sender.
    shutdown_tx: Arc<Mutex<Option<watch::Sender<bool>>>>,

    /// Callback to start/restart the agent.
    agent_start_fn: Arc<RwLock<Option<AgentStartFn>>>,

    /// Callback to check agent health.
    health_check_fn: Arc<RwLock<Option<AgentHealthCheckFn>>>,

    /// Callback to restore session state after restart.
    state_restore_fn: Arc<RwLock<Option<StateRestoreFn>>>,
}

impl ProcessSupervisor {
    /// Create a new ProcessSupervisor with the given configuration.
    pub fn new(config: SupervisorConfig) -> Self {
        let health_monitor = Arc::new(HealthMonitor::new(config.unresponsive_timeout));
        let stats = Arc::new(StatsTracker::new(config.max_restart_history));

        Self {
            config,
            health_monitor,
            stats,
            running: Arc::new(RwLock::new(false)),
            shutdown_tx: Arc::new(Mutex::new(None)),
            agent_start_fn: Arc::new(RwLock::new(None)),
            health_check_fn: Arc::new(RwLock::new(None)),
            state_restore_fn: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the callback function used to start/restart the agent.
    pub async fn set_agent_start_fn(&self, f: AgentStartFn) {
        *self.agent_start_fn.write().await = Some(f);
    }

    /// Set the callback function used to check agent health.
    pub async fn set_health_check_fn(&self, f: AgentHealthCheckFn) {
        *self.health_check_fn.write().await = Some(f);
    }

    /// Set the callback function used to restore session state after restart.
    pub async fn set_state_restore_fn(&self, f: StateRestoreFn) {
        *self.state_restore_fn.write().await = Some(f);
    }

    /// Start supervising the agent process.
    ///
    /// This performs the initial agent start and then spawns a background
    /// monitoring loop that checks health and triggers restarts as needed.
    ///
    /// Requirements: 2.2 (auto-start within 30 seconds of OS boot)
    pub async fn start(&self) -> Result<(), SupervisorError> {
        // Check if already running
        {
            let running = *self.running.read().await;
            if running {
                return Err(SupervisorError::AlreadyRunning);
            }
        }

        info!("Process Supervisor starting");

        // Perform initial agent start
        let start_time = Instant::now();
        self.start_agent(RestartReason::InitialStart).await?;

        let start_duration = start_time.elapsed();
        info!(
            duration_ms = start_duration.as_millis(),
            "Agent started successfully"
        );

        // Mark as running
        *self.running.write().await = true;

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // Spawn the monitoring loop
        self.spawn_monitoring_loop(shutdown_rx);

        info!("Process Supervisor monitoring loop started");
        Ok(())
    }

    /// Check if the agent is responsive.
    ///
    /// Performs an immediate health check and returns the current status.
    pub async fn health_check(&self) -> HealthStatus {
        let check_fn = self.health_check_fn.read().await;
        match check_fn.as_ref() {
            Some(f) => {
                let f = Arc::clone(f);
                self.health_monitor.perform_check(|| f()).await
            }
            None => {
                // No health check function registered, check based on alive status
                self.health_monitor.current_status().await
            }
        }
    }

    /// Force restart the agent with the given reason.
    ///
    /// Requirements: 2.7 (restart within 10 seconds on unexpected termination)
    pub async fn restart(&self, reason: &str) -> Result<(), SupervisorError> {
        let running = *self.running.read().await;
        if !running {
            return Err(SupervisorError::NotRunning);
        }

        info!(reason = reason, "Manual restart requested");

        let restart_reason = RestartReason::Manual {
            reason: reason.to_string(),
        };

        self.perform_restart(restart_reason).await
    }

    /// Get uptime and restart history statistics.
    pub async fn stats(&self) -> SupervisorStats {
        let health = self.health_monitor.current_status().await;
        self.stats.snapshot(health).await
    }

    /// Stop the supervisor and its monitoring loop.
    pub async fn stop(&self) -> Result<(), SupervisorError> {
        let running = *self.running.read().await;
        if !running {
            return Err(SupervisorError::NotRunning);
        }

        info!("Process Supervisor stopping");

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(true);
        }

        *self.running.write().await = false;
        self.health_monitor.mark_terminated().await;

        info!("Process Supervisor stopped");
        Ok(())
    }

    /// Check if the supervisor is currently running.
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    // =========================================================================
    // Internal methods
    // =========================================================================

    /// Start the agent using the registered callback.
    async fn start_agent(&self, reason: RestartReason) -> Result<(), SupervisorError> {
        let start_time = Instant::now();

        // Call the agent start function
        let start_fn = self.agent_start_fn.read().await;
        let result = match start_fn.as_ref() {
            Some(f) => {
                let f = Arc::clone(f);
                f().await
            }
            None => {
                // No start function registered — treat as a no-op success
                // (useful for testing or when the agent is managed externally)
                Ok(())
            }
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                // Mark agent as alive
                self.health_monitor.mark_alive().await;

                // Record the restart event
                self.stats.record_restart(reason.clone(), true, duration_ms).await;

                // Attempt state restoration
                self.restore_state().await;

                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Failed to start agent");
                self.stats.record_restart(reason, false, duration_ms).await;
                Err(SupervisorError::AgentStartFailed { reason: e })
            }
        }
    }

    /// Perform a restart of the agent.
    async fn perform_restart(&self, reason: RestartReason) -> Result<(), SupervisorError> {
        let start_time = Instant::now();

        info!(reason = %reason, "Performing agent restart");

        // Mark agent as terminated during restart
        self.health_monitor.mark_terminated().await;

        // Wait the configured restart delay
        time::sleep(self.config.restart_delay).await;

        // Start the agent
        let start_fn = self.agent_start_fn.read().await;
        let result = match start_fn.as_ref() {
            Some(f) => {
                let f = Arc::clone(f);
                f().await
            }
            None => Ok(()),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                self.health_monitor.mark_alive().await;
                self.stats.record_restart(reason, true, duration_ms).await;

                // Restore state after restart
                self.restore_state().await;

                info!(duration_ms = duration_ms, "Agent restart successful");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Agent restart failed");
                self.stats.record_restart(reason, false, duration_ms).await;
                Err(SupervisorError::RestartFailed { reason: e })
            }
        }
    }

    /// Attempt to restore session state after a restart.
    async fn restore_state(&self) {
        let restore_fn = self.state_restore_fn.read().await;
        if let Some(f) = restore_fn.as_ref() {
            let f = Arc::clone(f);
            match f().await {
                Ok(()) => {
                    info!("Session state restored successfully");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to restore session state, continuing without it");
                }
            }
        }
    }

    /// Spawn the background monitoring loop.
    ///
    /// This loop runs every `health_check_interval` and:
    /// 1. Checks if the agent is alive
    /// 2. If alive, performs a health check
    /// 3. If unresponsive for > timeout, triggers restart
    /// 4. If terminated unexpectedly, triggers restart within 10 seconds
    fn spawn_monitoring_loop(&self, mut shutdown_rx: watch::Receiver<bool>) {
        let health_monitor = Arc::clone(&self.health_monitor);
        let stats = Arc::clone(&self.stats);
        let config = self.config.clone();
        let health_check_fn = Arc::clone(&self.health_check_fn);
        let agent_start_fn = Arc::clone(&self.agent_start_fn);
        let state_restore_fn = Arc::clone(&self.state_restore_fn);
        let running = Arc::clone(&self.running);

        tokio::spawn(async move {
            let mut interval = time::interval(config.health_check_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Check if we should stop
                        if *shutdown_rx.borrow() {
                            info!("Monitoring loop received shutdown signal");
                            break;
                        }

                        // Perform health check
                        let status = {
                            let check_fn = health_check_fn.read().await;
                            match check_fn.as_ref() {
                                Some(f) => {
                                    let f = Arc::clone(f);
                                    health_monitor.perform_check(|| f()).await
                                }
                                None => health_monitor.current_status().await,
                            }
                        };

                        match status {
                            HealthStatus::Healthy => {
                                // All good, nothing to do
                            }
                            HealthStatus::Unresponsive { since } => {
                                let elapsed = (Utc::now() - since).num_seconds() as u64;
                                warn!(
                                    unresponsive_seconds = elapsed,
                                    "Agent unresponsive, triggering restart"
                                );

                                // Trigger restart due to unresponsiveness
                                let reason = RestartReason::Unresponsive {
                                    duration_seconds: elapsed,
                                };

                                Self::do_restart(
                                    &health_monitor,
                                    &stats,
                                    &agent_start_fn,
                                    &state_restore_fn,
                                    &config,
                                    reason,
                                )
                                .await;
                            }
                            HealthStatus::Terminated => {
                                warn!("Agent terminated unexpectedly, restarting within 10 seconds");

                                // Restart within 10 seconds (requirement 2.7)
                                let reason = RestartReason::UnexpectedTermination;

                                Self::do_restart(
                                    &health_monitor,
                                    &stats,
                                    &agent_start_fn,
                                    &state_restore_fn,
                                    &config,
                                    reason,
                                )
                                .await;
                            }
                            HealthStatus::Starting | HealthStatus::Degraded { .. } => {
                                // Agent is starting or degraded, wait for next check
                            }
                            HealthStatus::Unknown => {
                                // Supervisor not monitoring, shouldn't happen in the loop
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Monitoring loop received shutdown signal");
                            break;
                        }
                    }
                }
            }

            *running.write().await = false;
            info!("Monitoring loop exited");
        });
    }

    /// Perform a restart from within the monitoring loop.
    ///
    /// This is a static helper to avoid borrowing `self` in the spawned task.
    async fn do_restart(
        health_monitor: &Arc<HealthMonitor>,
        stats: &Arc<StatsTracker>,
        agent_start_fn: &Arc<RwLock<Option<AgentStartFn>>>,
        state_restore_fn: &Arc<RwLock<Option<StateRestoreFn>>>,
        config: &SupervisorConfig,
        reason: RestartReason,
    ) {
        let start_time = Instant::now();

        // Mark as terminated during restart
        health_monitor.mark_terminated().await;

        // Wait the restart delay (ensures we don't restart too quickly)
        time::sleep(config.restart_delay).await;

        // Attempt to start the agent
        let start_fn = agent_start_fn.read().await;
        let result = match start_fn.as_ref() {
            Some(f) => {
                let f = Arc::clone(f);
                f().await
            }
            None => Ok(()),
        };

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(()) => {
                health_monitor.mark_alive().await;
                stats.record_restart(reason, true, duration_ms).await;

                // Restore state
                let restore_fn = state_restore_fn.read().await;
                if let Some(f) = restore_fn.as_ref() {
                    let f = Arc::clone(f);
                    match f().await {
                        Ok(()) => info!("Session state restored after restart"),
                        Err(e) => warn!(error = %e, "Failed to restore state after restart"),
                    }
                }

                info!(duration_ms = duration_ms, "Agent restarted successfully");
            }
            Err(e) => {
                error!(error = %e, "Failed to restart agent");
                stats.record_restart(reason, false, duration_ms).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Helper to create a supervisor with mock callbacks that always succeed.
    fn create_test_supervisor() -> ProcessSupervisor {
        let config = SupervisorConfig {
            health_check_interval: Duration::from_millis(50),
            unresponsive_timeout: Duration::from_millis(200),
            restart_delay: Duration::from_millis(10),
            boot_start_timeout: Duration::from_secs(30),
            max_restart_history: 10,
        };
        ProcessSupervisor::new(config)
    }

    fn make_start_fn() -> AgentStartFn {
        Arc::new(|| Box::pin(async { Ok(()) }))
    }

    fn make_healthy_check_fn() -> AgentHealthCheckFn {
        Arc::new(|| Box::pin(async { true }))
    }

    fn make_unhealthy_check_fn() -> AgentHealthCheckFn {
        Arc::new(|| Box::pin(async { false }))
    }

    fn make_restore_fn() -> StateRestoreFn {
        Arc::new(|| Box::pin(async { Ok(()) }))
    }

    #[tokio::test]
    async fn test_start_supervisor() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;
        supervisor.set_state_restore_fn(make_restore_fn()).await;

        let result = supervisor.start().await;
        assert!(result.is_ok());
        assert!(supervisor.is_running().await);

        // Clean up
        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_start_already_running() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;

        supervisor.start().await.unwrap();

        let result = supervisor.start().await;
        assert!(matches!(result, Err(SupervisorError::AlreadyRunning)));

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_stop_supervisor() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;

        supervisor.start().await.unwrap();
        assert!(supervisor.is_running().await);

        supervisor.stop().await.unwrap();

        // Give the monitoring loop time to exit
        time::sleep(Duration::from_millis(100)).await;
        assert!(!supervisor.is_running().await);
    }

    #[tokio::test]
    async fn test_stop_not_running() {
        let supervisor = create_test_supervisor();
        let result = supervisor.stop().await;
        assert!(matches!(result, Err(SupervisorError::NotRunning)));
    }

    #[tokio::test]
    async fn test_health_check_healthy() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;

        supervisor.start().await.unwrap();

        let status = supervisor.health_check().await;
        assert_eq!(status, HealthStatus::Healthy);

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_manual_restart() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;
        supervisor.set_state_restore_fn(make_restore_fn()).await;

        supervisor.start().await.unwrap();

        let result = supervisor.restart("testing manual restart").await;
        assert!(result.is_ok());

        // Check stats
        let stats = supervisor.stats().await;
        // Initial start + manual restart = 2
        assert_eq!(stats.restart_count, 2);

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_restart_not_running() {
        let supervisor = create_test_supervisor();
        let result = supervisor.restart("test").await;
        assert!(matches!(result, Err(SupervisorError::NotRunning)));
    }

    #[tokio::test]
    async fn test_stats_initial() {
        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;

        supervisor.start().await.unwrap();

        let stats = supervisor.stats().await;
        assert_eq!(stats.restart_count, 1); // Initial start counts
        assert!(stats.last_restart.is_some());
        assert_eq!(
            stats.last_restart_reason.as_deref(),
            Some("initial start")
        );
        assert_eq!(stats.restart_history.len(), 1);

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_auto_restart_on_unresponsive() {
        let config = SupervisorConfig {
            health_check_interval: Duration::from_millis(30),
            unresponsive_timeout: Duration::from_millis(50),
            restart_delay: Duration::from_millis(5),
            boot_start_timeout: Duration::from_secs(30),
            max_restart_history: 10,
        };
        let supervisor = ProcessSupervisor::new(config);
        supervisor.set_agent_start_fn(make_start_fn()).await;
        // Use unhealthy check to simulate unresponsiveness
        supervisor
            .set_health_check_fn(make_unhealthy_check_fn())
            .await;
        supervisor.set_state_restore_fn(make_restore_fn()).await;

        supervisor.start().await.unwrap();

        // Wait for the unresponsive timeout + a few check intervals
        time::sleep(Duration::from_millis(200)).await;

        let stats = supervisor.stats().await;
        // Should have restarted at least once due to unresponsiveness
        assert!(stats.restart_count >= 2); // 1 initial + at least 1 auto-restart

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_auto_restart_on_termination() {
        let config = SupervisorConfig {
            health_check_interval: Duration::from_millis(30),
            unresponsive_timeout: Duration::from_millis(200),
            restart_delay: Duration::from_millis(5),
            boot_start_timeout: Duration::from_secs(30),
            max_restart_history: 10,
        };
        let supervisor = ProcessSupervisor::new(config);
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;
        supervisor.set_state_restore_fn(make_restore_fn()).await;

        supervisor.start().await.unwrap();

        // Simulate unexpected termination
        supervisor.health_monitor.mark_terminated().await;

        // Wait for the monitoring loop to detect and restart
        time::sleep(Duration::from_millis(100)).await;

        let stats = supervisor.stats().await;
        // Should have restarted due to termination detection
        assert!(stats.restart_count >= 2);

        // Check that the last restart reason mentions termination
        let history = &stats.restart_history;
        let has_termination_restart = history.iter().any(|e| {
            matches!(e.reason, RestartReason::UnexpectedTermination)
        });
        assert!(has_termination_restart);

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_state_restore_called_on_restart() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let restore_count = Arc::new(AtomicU32::new(0));
        let restore_count_clone = Arc::clone(&restore_count);

        let restore_fn: StateRestoreFn = Arc::new(move || {
            let count = Arc::clone(&restore_count_clone);
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;
        supervisor.set_state_restore_fn(restore_fn).await;

        supervisor.start().await.unwrap();

        // Initial start should call restore
        assert_eq!(restore_count.load(Ordering::SeqCst), 1);

        // Manual restart should also call restore
        supervisor.restart("test").await.unwrap();
        assert_eq!(restore_count.load(Ordering::SeqCst), 2);

        supervisor.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_start_failure_recorded() {
        let fail_start_fn: AgentStartFn = Arc::new(|| {
            Box::pin(async { Err("simulated failure".to_string()) })
        });

        let supervisor = create_test_supervisor();
        supervisor.set_agent_start_fn(fail_start_fn).await;

        let result = supervisor.start().await;
        assert!(matches!(result, Err(SupervisorError::AgentStartFailed { .. })));
        assert!(!supervisor.is_running().await);
    }

    #[tokio::test]
    async fn test_restart_history_bounded() {
        let config = SupervisorConfig {
            // Use a long check interval so the monitoring loop doesn't interfere
            health_check_interval: Duration::from_secs(60),
            unresponsive_timeout: Duration::from_secs(120),
            restart_delay: Duration::from_millis(5),
            boot_start_timeout: Duration::from_secs(30),
            max_restart_history: 3,
        };
        let supervisor = ProcessSupervisor::new(config);
        supervisor.set_agent_start_fn(make_start_fn()).await;
        supervisor.set_health_check_fn(make_healthy_check_fn()).await;
        supervisor.set_state_restore_fn(make_restore_fn()).await;

        supervisor.start().await.unwrap(); // restart #1

        // Perform multiple restarts
        for i in 0..5 {
            supervisor
                .restart(&format!("test restart {}", i))
                .await
                .unwrap();
        }

        let stats = supervisor.stats().await;
        // History should be bounded to max_restart_history (3)
        assert!(stats.restart_history.len() <= 3);
        // Total count should be at least 6 (1 initial + 5 manual)
        // May be slightly higher if monitoring loop detected a brief termination
        assert!(stats.restart_count >= 6);

        supervisor.stop().await.unwrap();
    }
}
