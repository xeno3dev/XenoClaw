//! Property-based tests for the alerting engine.
//!
//! **Validates: Requirements 18.3**
//!
//! Property 36: Alert Rule Evaluation Correctness
//!
//! For any alert rule with a numeric threshold condition (CpuAbove, MemoryAbove),
//! when the current metric value exceeds the threshold, the rule fires.
//! When the value is at or below the threshold, the rule does not fire.
//! The evaluation is deterministic and consistent.
//!
//! NOTE: These tests use global Prometheus metrics state, so they must be
//! serialized to avoid interference between parallel test threads.

use common::alerting::{evaluate_single_rule, AlertCondition, AlertRule, AlertSeverity};
use common::metrics;
use proptest::prelude::*;
use std::sync::Mutex;

/// Global mutex to serialize access to shared Prometheus metrics gauges.
/// Without this, parallel proptest threads would overwrite each other's metric values.
static METRICS_LOCK: Mutex<()> = Mutex::new(());

// ============================================================================
// Strategies for generating alert rule parameters
// ============================================================================

/// Generate a valid CPU threshold (0.0 to 100.0).
fn cpu_threshold_strategy() -> impl Strategy<Value = f64> {
    (0u32..=10000u32).prop_map(|v| v as f64 / 100.0)
}

/// Generate a CPU metric value (0.0 to 100.0).
fn cpu_value_strategy() -> impl Strategy<Value = f64> {
    (0u32..=10000u32).prop_map(|v| v as f64 / 100.0)
}

/// Generate a memory threshold in bytes (0 to 16GB).
fn memory_threshold_strategy() -> impl Strategy<Value = u64> {
    0u64..=16_000_000_000u64
}

/// Generate a memory metric value in bytes (0.0 to 16GB).
fn memory_value_strategy() -> impl Strategy<Value = f64> {
    (0u64..=16_000_000_000u64).prop_map(|v| v as f64)
}

/// Generate an alert severity.
fn severity_strategy() -> impl Strategy<Value = AlertSeverity> {
    prop_oneof![
        Just(AlertSeverity::Info),
        Just(AlertSeverity::Warning),
        Just(AlertSeverity::Critical),
    ]
}

/// Generate a rule name.
fn rule_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{2,20}".prop_map(|s| s)
}

// ============================================================================
// Property 36: Alert Rule Evaluation Correctness
//
// For any alert rule with a numeric threshold condition (CpuAbove, MemoryAbove),
// when the current metric value exceeds the threshold, the rule fires (returns Some).
// When the value is at or below the threshold, the rule does not fire (returns None).
// The evaluation is deterministic and consistent.
//
// **Validates: Requirements 18.3**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// **Validates: Requirements 18.3**
    ///
    /// Property 36: When the current CPU value exceeds the threshold,
    /// the CpuAbove rule fires (returns Some).
    #[test]
    fn prop_cpu_above_fires_when_value_exceeds_threshold(
        threshold in cpu_threshold_strategy(),
        delta in 0.01f64..50.0f64,
        name in rule_name_strategy(),
        severity in severity_strategy(),
    ) {
        let _lock = METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Ensure current value is strictly above threshold
        let current = (threshold + delta).min(100.0);
        // Skip cases where current doesn't actually exceed threshold due to clamping
        prop_assume!(current > threshold);

        // Set the CPU gauge to the current value
        metrics::update_system_metrics(current, 0.0);

        let rule = AlertRule {
            name: name.clone(),
            condition: AlertCondition::CpuAbove(threshold),
            severity,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        prop_assert!(
            result.is_some(),
            "CpuAbove rule should fire when current ({}) > threshold ({})",
            current,
            threshold
        );

        let alert = result.unwrap();
        prop_assert_eq!(&alert.rule_name, &name);
        prop_assert_eq!(alert.severity, severity);
        prop_assert!((alert.current_value - current).abs() < f64::EPSILON);
    }

    /// **Validates: Requirements 18.3**
    ///
    /// Property 36: When the current CPU value is at or below the threshold,
    /// the CpuAbove rule does not fire (returns None).
    #[test]
    fn prop_cpu_above_does_not_fire_when_value_at_or_below_threshold(
        threshold in cpu_threshold_strategy(),
        current in cpu_value_strategy(),
        name in rule_name_strategy(),
        severity in severity_strategy(),
    ) {
        let _lock = METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Only test cases where current <= threshold
        prop_assume!(current <= threshold);

        // Set the CPU gauge to the current value
        metrics::update_system_metrics(current, 0.0);

        let rule = AlertRule {
            name,
            condition: AlertCondition::CpuAbove(threshold),
            severity,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        prop_assert!(
            result.is_none(),
            "CpuAbove rule should NOT fire when current ({}) <= threshold ({})",
            current,
            threshold
        );
    }

    /// **Validates: Requirements 18.3**
    ///
    /// Property 36: When the current memory value exceeds the threshold,
    /// the MemoryAbove rule fires (returns Some).
    #[test]
    fn prop_memory_above_fires_when_value_exceeds_threshold(
        threshold in memory_threshold_strategy(),
        delta in 1u64..1_000_000_000u64,
        name in rule_name_strategy(),
        severity in severity_strategy(),
    ) {
        let _lock = METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Ensure current value is strictly above threshold
        let current = (threshold as f64) + (delta as f64);

        // Set the memory gauge to the current value
        metrics::update_system_metrics(0.0, current);

        let rule = AlertRule {
            name: name.clone(),
            condition: AlertCondition::MemoryAbove(threshold),
            severity,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        prop_assert!(
            result.is_some(),
            "MemoryAbove rule should fire when current ({}) > threshold ({})",
            current,
            threshold
        );

        let alert = result.unwrap();
        prop_assert_eq!(&alert.rule_name, &name);
        prop_assert_eq!(alert.severity, severity);
        prop_assert!((alert.current_value - current).abs() < f64::EPSILON);
    }

    /// **Validates: Requirements 18.3**
    ///
    /// Property 36: When the current memory value is at or below the threshold,
    /// the MemoryAbove rule does not fire (returns None).
    #[test]
    fn prop_memory_above_does_not_fire_when_value_at_or_below_threshold(
        threshold in memory_threshold_strategy(),
        current in memory_value_strategy(),
        name in rule_name_strategy(),
        severity in severity_strategy(),
    ) {
        let _lock = METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Only test cases where current <= threshold (as f64)
        let threshold_f64 = threshold as f64;
        prop_assume!(current <= threshold_f64);

        // Set the memory gauge to the current value
        metrics::update_system_metrics(0.0, current);

        let rule = AlertRule {
            name,
            condition: AlertCondition::MemoryAbove(threshold),
            severity,
            cooldown_seconds: 60,
        };

        let result = evaluate_single_rule(&rule);
        prop_assert!(
            result.is_none(),
            "MemoryAbove rule should NOT fire when current ({}) <= threshold ({})",
            current,
            threshold_f64
        );
    }

    /// **Validates: Requirements 18.3**
    ///
    /// Property 36: Evaluation is deterministic — calling evaluate_single_rule
    /// twice with the same metric state and rule produces the same result.
    #[test]
    fn prop_evaluation_is_deterministic(
        cpu_value in cpu_value_strategy(),
        cpu_threshold in cpu_threshold_strategy(),
        name in rule_name_strategy(),
        severity in severity_strategy(),
    ) {
        let _lock = METRICS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Set metrics to a known state
        metrics::update_system_metrics(cpu_value, 0.0);

        let rule = AlertRule {
            name,
            condition: AlertCondition::CpuAbove(cpu_threshold),
            severity,
            cooldown_seconds: 60,
        };

        let result1 = evaluate_single_rule(&rule);
        let result2 = evaluate_single_rule(&rule);

        // Both calls should produce the same firing decision
        prop_assert_eq!(result1.is_some(), result2.is_some());

        // If both fired, the current_value should be the same
        if let (Some(a1), Some(a2)) = (result1, result2) {
            prop_assert!((a1.current_value - a2.current_value).abs() < f64::EPSILON);
            prop_assert_eq!(a1.severity, a2.severity);
            prop_assert_eq!(a1.rule_name, a2.rule_name);
        }
    }
}
