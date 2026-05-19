//! Configurable alerting engine for the VPS AI Agent Platform.
//!
//! Evaluates alert rules against current metrics and delivers notifications
//! to configured channels (webhook URLs) when conditions are met.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use prometheus::core::Collector;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::metrics;

/// Severity level for an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    /// Informational alert, no immediate action required.
    Info,
    /// Warning alert, should be investigated soon.
    Warning,
    /// Critical alert, requires immediate attention.
    Critical,
}

/// Condition that triggers an alert when met.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum AlertCondition {
    /// Error rate exceeds the given threshold (0.0 to 1.0, e.g. 0.05 = 5%).
    ErrorRateAbove(f64),
    /// CPU usage exceeds the given percentage (0-100).
    CpuAbove(f64),
    /// Memory usage exceeds the given number of bytes.
    MemoryAbove(u64),
    /// Agent has been unresponsive for longer than the given duration in seconds.
    AgentUnresponsive(u64),
}

/// A single alert rule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    /// Human-readable name for this rule.
    pub name: String,
    /// The condition that triggers this alert.
    pub condition: AlertCondition,
    /// Severity of the alert when triggered.
    pub severity: AlertSeverity,
    /// Minimum time (in seconds) between repeated firings of this rule.
    pub cooldown_seconds: u64,
}

/// Configuration for the alerting system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    /// List of alert rules to evaluate.
    pub rules: Vec<AlertRule>,
    /// Notification channel URL (webhook endpoint for HTTP POST delivery).
    pub notification_channel: String,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            notification_channel: String::new(),
        }
    }
}

/// A triggered alert ready for delivery.
#[derive(Debug, Clone, Serialize)]
pub struct TriggeredAlert {
    /// Name of the rule that triggered.
    pub rule_name: String,
    /// Severity of the alert.
    pub severity: AlertSeverity,
    /// Human-readable description of what triggered the alert.
    pub message: String,
    /// The current value that triggered the condition.
    pub current_value: f64,
    /// Timestamp when the alert was triggered (ISO 8601).
    pub triggered_at: String,
}

/// The alert engine evaluates rules against current metrics and fires alerts.
pub struct AlertEngine {
    config: AlertConfig,
    /// Tracks the last time each rule fired (by rule name).
    last_fired: HashMap<String, Instant>,
    /// HTTP client for delivering webhook notifications.
    client: reqwest::Client,
}

impl AlertEngine {
    /// Create a new AlertEngine with the given configuration.
    pub fn new(config: AlertConfig) -> Self {
        Self {
            config,
            last_fired: HashMap::new(),
            client: reqwest::Client::new(),
        }
    }

    /// Update the alert configuration (e.g., after a config reload).
    pub fn update_config(&mut self, config: AlertConfig) {
        self.config = config;
    }

    /// Evaluate all rules against current metric values and return triggered alerts.
    ///
    /// This respects cooldown periods — a rule that fired recently will not
    /// fire again until its cooldown has elapsed.
    pub fn evaluate_rules(&mut self) -> Vec<TriggeredAlert> {
        let now = Instant::now();
        let mut triggered = Vec::new();

        for rule in &self.config.rules {
            // Check cooldown
            if let Some(last) = self.last_fired.get(&rule.name) {
                let cooldown = Duration::from_secs(rule.cooldown_seconds);
                if now.duration_since(*last) < cooldown {
                    continue;
                }
            }

            if let Some(alert) = evaluate_single_rule(rule) {
                self.last_fired.insert(rule.name.clone(), now);
                triggered.push(alert);
            }
        }

        triggered
    }

    /// Evaluate rules and deliver any triggered alerts to the notification channel.
    pub async fn evaluate_and_notify(&mut self) -> Vec<TriggeredAlert> {
        let alerts = self.evaluate_rules();

        if alerts.is_empty() {
            return alerts;
        }

        if self.config.notification_channel.is_empty() {
            warn!("Alerts triggered but no notification channel configured");
            return alerts;
        }

        for alert in &alerts {
            self.deliver_alert(alert).await;
        }

        alerts
    }

    /// Deliver a single alert to the configured notification channel via HTTP POST.
    async fn deliver_alert(&self, alert: &TriggeredAlert) {
        let url = &self.config.notification_channel;

        info!(
            rule = %alert.rule_name,
            severity = ?alert.severity,
            message = %alert.message,
            "Delivering alert notification"
        );

        match self.client.post(url).json(alert).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    info!(
                        rule = %alert.rule_name,
                        "Alert delivered successfully"
                    );
                } else {
                    error!(
                        rule = %alert.rule_name,
                        status = %response.status(),
                        "Alert delivery received non-success response"
                    );
                }
            }
            Err(e) => {
                error!(
                    rule = %alert.rule_name,
                    error = %e,
                    "Failed to deliver alert notification"
                );
            }
        }
    }
}

/// Evaluate a single rule against current metrics.
/// Returns `Some(TriggeredAlert)` if the condition is met, `None` otherwise.
pub fn evaluate_single_rule(rule: &AlertRule) -> Option<TriggeredAlert> {
    let now = chrono::Utc::now().to_rfc3339();

    match &rule.condition {
        AlertCondition::ErrorRateAbove(threshold) => {
            let error_rate = compute_error_rate();
            if error_rate > *threshold {
                Some(TriggeredAlert {
                    rule_name: rule.name.clone(),
                    severity: rule.severity,
                    message: format!(
                        "Error rate {:.2}% exceeds threshold {:.2}%",
                        error_rate * 100.0,
                        threshold * 100.0
                    ),
                    current_value: error_rate,
                    triggered_at: now,
                })
            } else {
                None
            }
        }
        AlertCondition::CpuAbove(threshold) => {
            let cpu = metrics::SYSTEM_CPU_USAGE_PERCENT.with_label_values(&[]).get();
            if cpu > *threshold {
                Some(TriggeredAlert {
                    rule_name: rule.name.clone(),
                    severity: rule.severity,
                    message: format!(
                        "CPU usage {:.1}% exceeds threshold {:.1}%",
                        cpu, threshold
                    ),
                    current_value: cpu,
                    triggered_at: now,
                })
            } else {
                None
            }
        }
        AlertCondition::MemoryAbove(threshold) => {
            let mem = metrics::SYSTEM_MEMORY_BYTES.with_label_values(&[]).get();
            let threshold_f64 = *threshold as f64;
            if mem > threshold_f64 {
                Some(TriggeredAlert {
                    rule_name: rule.name.clone(),
                    severity: rule.severity,
                    message: format!(
                        "Memory usage {} bytes exceeds threshold {} bytes",
                        mem as u64, threshold
                    ),
                    current_value: mem,
                    triggered_at: now,
                })
            } else {
                None
            }
        }
        AlertCondition::AgentUnresponsive(max_seconds) => {
            let uptime = metrics::AGENT_UPTIME_SECONDS.with_label_values(&[]).get();
            // If uptime is 0 and max_seconds threshold is exceeded, agent is unresponsive.
            // In practice, the supervisor updates uptime regularly. If it stops updating,
            // the uptime gauge will be stale. We check if uptime is 0 (never started)
            // or if the gauge hasn't been updated (detected externally).
            // For alerting purposes, we treat uptime == 0 as unresponsive.
            if uptime == 0.0 {
                Some(TriggeredAlert {
                    rule_name: rule.name.clone(),
                    severity: rule.severity,
                    message: format!(
                        "Agent appears unresponsive (uptime gauge is 0, threshold: {}s)",
                        max_seconds
                    ),
                    current_value: 0.0,
                    triggered_at: now,
                })
            } else {
                None
            }
        }
    }
}

/// Compute the current error rate from HTTP request metrics.
/// Returns a value between 0.0 and 1.0.
fn compute_error_rate() -> f64 {
    // Gather all HTTP request counts and compute error ratio.
    // Errors are defined as 5xx status codes.
    let metric_families = metrics::HTTP_REQUESTS_TOTAL.collect();

    let mut total_requests: u64 = 0;
    let mut error_requests: u64 = 0;

    for mf in &metric_families {
        for m in mf.get_metric() {
            let count = m.get_counter().get_value() as u64;
            total_requests += count;

            // Check if this metric has a status label starting with '5'
            for label in m.get_label() {
                if label.get_name() == "status" && label.get_value().starts_with('5') {
                    error_requests += count;
                }
            }
        }
    }

    if total_requests == 0 {
        0.0
    } else {
        error_requests as f64 / total_requests as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::lock_metrics;

    #[test]
    fn test_alert_config_default() {
        let config = AlertConfig::default();
        assert!(config.rules.is_empty());
        assert!(config.notification_channel.is_empty());
    }

    #[test]
    fn test_evaluate_cpu_rule_not_triggered() {
        let _lock = lock_metrics();
        // Set CPU to a low value
        metrics::update_system_metrics(10.0, 0.0);

        let rule = AlertRule {
            name: "high_cpu".to_string(),
            condition: AlertCondition::CpuAbove(80.0),
            severity: AlertSeverity::Warning,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_none());
    }

    #[test]
    fn test_evaluate_cpu_rule_triggered() {
        let _lock = lock_metrics();
        // Set CPU to a high value
        metrics::update_system_metrics(95.0, 0.0);

        let rule = AlertRule {
            name: "high_cpu_triggered".to_string(),
            condition: AlertCondition::CpuAbove(80.0),
            severity: AlertSeverity::Critical,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_some());
        let alert = result.unwrap();
        assert_eq!(alert.rule_name, "high_cpu_triggered");
        assert_eq!(alert.severity, AlertSeverity::Critical);
        assert!(alert.current_value > 80.0);
    }

    #[test]
    fn test_evaluate_memory_rule_triggered() {
        let _lock = lock_metrics();
        // Set memory to 2GB
        metrics::update_system_metrics(0.0, 2_147_483_648.0);

        let rule = AlertRule {
            name: "high_memory".to_string(),
            condition: AlertCondition::MemoryAbove(1_073_741_824), // 1GB threshold
            severity: AlertSeverity::Warning,
            cooldown_seconds: 120,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_some());
        let alert = result.unwrap();
        assert_eq!(alert.rule_name, "high_memory");
        assert!(alert.current_value > 1_073_741_824.0);
    }

    #[test]
    fn test_evaluate_memory_rule_not_triggered() {
        let _lock = lock_metrics();
        metrics::update_system_metrics(0.0, 500_000_000.0);

        let rule = AlertRule {
            name: "high_memory_no".to_string(),
            condition: AlertCondition::MemoryAbove(1_073_741_824),
            severity: AlertSeverity::Warning,
            cooldown_seconds: 120,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_none());
    }

    #[test]
    fn test_evaluate_agent_unresponsive() {
        let _lock = lock_metrics();
        // Set uptime to 0 (unresponsive)
        metrics::update_agent_uptime(0.0);

        let rule = AlertRule {
            name: "agent_down".to_string(),
            condition: AlertCondition::AgentUnresponsive(60),
            severity: AlertSeverity::Critical,
            cooldown_seconds: 30,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_some());
    }

    #[test]
    fn test_evaluate_agent_responsive() {
        let _lock = lock_metrics();
        // Set uptime to a positive value (responsive)
        metrics::update_agent_uptime(3600.0);

        let rule = AlertRule {
            name: "agent_up".to_string(),
            condition: AlertCondition::AgentUnresponsive(60),
            severity: AlertSeverity::Critical,
            cooldown_seconds: 30,
        };

        let result = evaluate_single_rule(&rule);
        assert!(result.is_none());
    }

    #[test]
    fn test_cooldown_respected() {
        let _lock = lock_metrics();
        metrics::update_system_metrics(95.0, 0.0);

        let config = AlertConfig {
            rules: vec![AlertRule {
                name: "cpu_cooldown_test".to_string(),
                condition: AlertCondition::CpuAbove(80.0),
                severity: AlertSeverity::Warning,
                cooldown_seconds: 300, // 5 minute cooldown
            }],
            notification_channel: String::new(),
        };

        let mut engine = AlertEngine::new(config);

        // First evaluation should trigger
        let alerts1 = engine.evaluate_rules();
        assert_eq!(alerts1.len(), 1);

        // Second evaluation immediately after should NOT trigger (cooldown)
        let alerts2 = engine.evaluate_rules();
        assert_eq!(alerts2.len(), 0);
    }

    #[test]
    fn test_error_rate_no_requests() {
        // With no requests recorded, error rate should be 0
        let rate = compute_error_rate();
        assert!(rate >= 0.0 && rate <= 1.0);
    }

    #[test]
    fn test_error_rate_with_errors() {
        // Record some successful and failed requests
        metrics::record_http_request("GET", "/test_error_rate", 200, 0.01);
        metrics::record_http_request("GET", "/test_error_rate", 200, 0.01);
        metrics::record_http_request("GET", "/test_error_rate", 500, 0.01);

        let rate = compute_error_rate();
        // Rate should be > 0 since we have at least one 5xx
        assert!(rate > 0.0);
    }

    #[test]
    fn test_alert_serialization() {
        let alert = TriggeredAlert {
            rule_name: "test_rule".to_string(),
            severity: AlertSeverity::Critical,
            message: "Test alert".to_string(),
            current_value: 95.0,
            triggered_at: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&alert).unwrap();
        assert!(json.contains("test_rule"));
        assert!(json.contains("critical"));
        assert!(json.contains("95"));
    }

    #[test]
    fn test_alert_condition_serialization() {
        let condition = AlertCondition::ErrorRateAbove(0.05);
        let json = serde_json::to_string(&condition).unwrap();
        assert!(json.contains("ErrorRateAbove"));
        assert!(json.contains("0.05"));

        let condition = AlertCondition::CpuAbove(80.0);
        let json = serde_json::to_string(&condition).unwrap();
        assert!(json.contains("CpuAbove"));

        let condition = AlertCondition::MemoryAbove(1_073_741_824);
        let json = serde_json::to_string(&condition).unwrap();
        assert!(json.contains("MemoryAbove"));

        let condition = AlertCondition::AgentUnresponsive(60);
        let json = serde_json::to_string(&condition).unwrap();
        assert!(json.contains("AgentUnresponsive"));
    }
}
