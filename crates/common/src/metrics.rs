//! Prometheus-compatible metrics for the VPS AI Agent Platform.
//!
//! Exposes request counts, latency histograms (p50/p90/p95/p99),
//! LLM token usage, CPU %, and memory bytes as Prometheus metrics.

use lazy_static::lazy_static;
use prometheus::{
    Encoder, GaugeVec, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder,
};

lazy_static! {
    /// Global metrics registry for the platform.
    pub static ref REGISTRY: Registry = Registry::new();

    /// Total HTTP requests, labeled by method, path, and status code.
    pub static ref HTTP_REQUESTS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("http_requests_total", "Total number of HTTP requests"),
        &["method", "path", "status"],
    )
    .expect("http_requests_total metric creation failed");

    /// HTTP request duration in seconds, labeled by method and path.
    /// Buckets are chosen to capture p50/p90/p95/p99 latency distribution.
    pub static ref HTTP_REQUEST_DURATION_SECONDS: HistogramVec = HistogramVec::new(
        HistogramOpts::new(
            "http_request_duration_seconds",
            "HTTP request duration in seconds",
        )
        .buckets(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        &["method", "path"],
    )
    .expect("http_request_duration_seconds metric creation failed");

    /// Total LLM tokens consumed, labeled by provider and direction (input/output).
    pub static ref LLM_TOKENS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("llm_tokens_total", "Total LLM tokens consumed"),
        &["provider", "direction"],
    )
    .expect("llm_tokens_total metric creation failed");

    /// Current system CPU usage as a percentage (0-100).
    pub static ref SYSTEM_CPU_USAGE_PERCENT: GaugeVec = GaugeVec::new(
        Opts::new("system_cpu_usage_percent", "Current system CPU usage percentage"),
        &[],
    )
    .expect("system_cpu_usage_percent metric creation failed");

    /// Current system memory usage in bytes.
    pub static ref SYSTEM_MEMORY_BYTES: GaugeVec = GaugeVec::new(
        Opts::new("system_memory_bytes", "Current system memory usage in bytes"),
        &[],
    )
    .expect("system_memory_bytes metric creation failed");

    /// Total agent tasks processed, labeled by status (completed/failed).
    pub static ref AGENT_TASKS_TOTAL: IntCounterVec = IntCounterVec::new(
        Opts::new("agent_tasks_total", "Total agent tasks processed"),
        &["status"],
    )
    .expect("agent_tasks_total metric creation failed");

    /// Agent uptime in seconds.
    pub static ref AGENT_UPTIME_SECONDS: GaugeVec = GaugeVec::new(
        Opts::new("agent_uptime_seconds", "Agent uptime in seconds"),
        &[],
    )
    .expect("agent_uptime_seconds metric creation failed");
}

/// Holds references to all registered metric handles.
/// Provides a convenient interface for accessing platform metrics.
pub struct MetricsRegistry {
    registry: Registry,
}

impl MetricsRegistry {
    /// Create a new MetricsRegistry and register all metrics with the internal registry.
    pub fn new() -> Self {
        let registry = Registry::new();

        registry
            .register(Box::new(HTTP_REQUESTS_TOTAL.clone()))
            .expect("Failed to register http_requests_total");
        registry
            .register(Box::new(HTTP_REQUEST_DURATION_SECONDS.clone()))
            .expect("Failed to register http_request_duration_seconds");
        registry
            .register(Box::new(LLM_TOKENS_TOTAL.clone()))
            .expect("Failed to register llm_tokens_total");
        registry
            .register(Box::new(SYSTEM_CPU_USAGE_PERCENT.clone()))
            .expect("Failed to register system_cpu_usage_percent");
        registry
            .register(Box::new(SYSTEM_MEMORY_BYTES.clone()))
            .expect("Failed to register system_memory_bytes");
        registry
            .register(Box::new(AGENT_TASKS_TOTAL.clone()))
            .expect("Failed to register agent_tasks_total");
        registry
            .register(Box::new(AGENT_UPTIME_SECONDS.clone()))
            .expect("Failed to register agent_uptime_seconds");

        Self { registry }
    }

    /// Get a reference to the underlying Prometheus registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Render all metrics in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .expect("Failed to encode metrics");
        String::from_utf8(buffer).expect("Metrics output is not valid UTF-8")
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum handler that returns Prometheus metrics in text exposition format.
/// Mount this at `/metrics` in your axum router.
///
/// # Example
/// ```ignore
/// use axum::{Router, routing::get};
/// use common::metrics::metrics_handler;
///
/// let app = Router::new().route("/metrics", get(metrics_handler));
/// ```
pub async fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let registry = MetricsRegistry::new();
    let metric_families = registry.registry().gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .expect("Failed to encode metrics");
    String::from_utf8(buffer).expect("Metrics output is not valid UTF-8")
}

/// Record an HTTP request metric.
pub fn record_http_request(method: &str, path: &str, status: u16, duration_secs: f64) {
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[method, path, &status.to_string()])
        .inc();
    HTTP_REQUEST_DURATION_SECONDS
        .with_label_values(&[method, path])
        .observe(duration_secs);
}

/// Record LLM token usage.
pub fn record_llm_tokens(provider: &str, input_tokens: u64, output_tokens: u64) {
    LLM_TOKENS_TOTAL
        .with_label_values(&[provider, "input"])
        .inc_by(input_tokens);
    LLM_TOKENS_TOTAL
        .with_label_values(&[provider, "output"])
        .inc_by(output_tokens);
}

/// Update system resource gauges.
pub fn update_system_metrics(cpu_percent: f64, memory_bytes: f64) {
    SYSTEM_CPU_USAGE_PERCENT
        .with_label_values(&[])
        .set(cpu_percent);
    SYSTEM_MEMORY_BYTES
        .with_label_values(&[])
        .set(memory_bytes);
}

/// Record a completed or failed agent task.
pub fn record_agent_task(status: &str) {
    AGENT_TASKS_TOTAL.with_label_values(&[status]).inc();
}

/// Update agent uptime gauge.
pub fn update_agent_uptime(seconds: f64) {
    AGENT_UPTIME_SECONDS.with_label_values(&[]).set(seconds);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_registry_creation() {
        let registry = MetricsRegistry::new();
        let output = registry.render();
        // Should produce valid (possibly empty) prometheus text output
        assert!(output.is_empty() || output.contains("# HELP") || output.contains("# TYPE"));
    }

    #[test]
    fn test_record_http_request() {
        // Reset by creating a fresh registry
        let _registry = MetricsRegistry::new();
        record_http_request("GET", "/api/v1/status", 200, 0.05);

        let val = HTTP_REQUESTS_TOTAL
            .with_label_values(&["GET", "/api/v1/status", "200"])
            .get();
        assert!(val >= 1);
    }

    #[test]
    fn test_record_llm_tokens() {
        let _registry = MetricsRegistry::new();
        record_llm_tokens("openai", 100, 50);

        let input_val = LLM_TOKENS_TOTAL
            .with_label_values(&["openai", "input"])
            .get();
        let output_val = LLM_TOKENS_TOTAL
            .with_label_values(&["openai", "output"])
            .get();
        assert!(input_val >= 100);
        assert!(output_val >= 50);
    }

    #[test]
    fn test_update_system_metrics() {
        let _lock = crate::test_utils::lock_metrics();
        let _registry = MetricsRegistry::new();
        update_system_metrics(45.5, 1_073_741_824.0);

        let cpu = SYSTEM_CPU_USAGE_PERCENT.with_label_values(&[]).get();
        let mem = SYSTEM_MEMORY_BYTES.with_label_values(&[]).get();
        assert!((cpu - 45.5).abs() < f64::EPSILON);
        assert!((mem - 1_073_741_824.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_record_agent_task() {
        let _registry = MetricsRegistry::new();
        record_agent_task("completed");
        record_agent_task("failed");

        let completed = AGENT_TASKS_TOTAL.with_label_values(&["completed"]).get();
        let failed = AGENT_TASKS_TOTAL.with_label_values(&["failed"]).get();
        assert!(completed >= 1);
        assert!(failed >= 1);
    }

    #[test]
    fn test_update_agent_uptime() {
        let _lock = crate::test_utils::lock_metrics();
        let _registry = MetricsRegistry::new();
        update_agent_uptime(3600.0);

        let uptime = AGENT_UPTIME_SECONDS.with_label_values(&[]).get();
        assert!((uptime - 3600.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_handler_returns_text() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let output = rt.block_on(metrics_handler());
        // Should be valid text (may be empty if no metrics recorded yet)
        assert!(output.is_empty() || output.is_ascii());
    }
}
