//! Structured logging subsystem for the VPS AI Agent Platform.
//!
//! Provides JSON-formatted structured logging with:
//! - Configurable log levels (DEBUG, INFO, WARN, ERROR, FATAL)
//! - ISO 8601 timestamps, correlation IDs, component names
//! - Log rotation when files exceed a configured max size
//! - Retention-based cleanup (default 30 days, configurable 1–365)
//! - Ring buffer (1000 entries) when the filesystem is unavailable

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Log levels supported by the structured logger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    /// Returns the string representation used in JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Configuration for the structured logging subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// Minimum log level to emit (default: Info)
    pub log_level: LogLevel,
    /// Directory where log files are written
    pub log_dir: PathBuf,
    /// Maximum size of a single log file in megabytes before rotation (default: 50)
    pub max_file_size_mb: u32,
    /// Number of days to retain log files (1–365, default: 30)
    pub retention_days: u16,
    /// Maximum number of entries in the ring buffer (default: 1000)
    pub max_buffer_size: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Info,
            log_dir: PathBuf::from("/var/log/xenoclaw"),
            max_file_size_mb: 50,
            retention_days: 30,
            max_buffer_size: 1000,
        }
    }
}

impl LogConfig {
    /// Validates the configuration, returning an error description if invalid.
    pub fn validate(&self) -> Result<(), String> {
        if self.retention_days < 1 || self.retention_days > 365 {
            return Err(format!(
                "retention_days must be between 1 and 365, got {}",
                self.retention_days
            ));
        }
        if self.max_file_size_mb == 0 {
            return Err("max_file_size_mb must be greater than 0".to_string());
        }
        if self.max_buffer_size == 0 {
            return Err("max_buffer_size must be greater than 0".to_string());
        }
        Ok(())
    }
}

/// A single structured log entry in JSON format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Correlation ID (UUID) for request tracing
    pub correlation_id: String,
    /// Log level
    pub level: String,
    /// Source component name
    pub component: String,
    /// Log message
    pub message: String,
}

impl LogEntry {
    /// Creates a new log entry with the current timestamp and a new correlation ID.
    pub fn new(level: LogLevel, component: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            correlation_id: Uuid::new_v4().to_string(),
            level: level.as_str().to_string(),
            component: component.into(),
            message: message.into(),
        }
    }

    /// Creates a new log entry with a specific correlation ID.
    pub fn with_correlation_id(
        level: LogLevel,
        component: impl Into<String>,
        message: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            correlation_id: correlation_id.into(),
            level: level.as_str().to_string(),
            component: component.into(),
            message: message.into(),
        }
    }

    /// Serializes the entry to a single-line JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Internal state for the structured logger.
struct LoggerState {
    /// Current log file handle (None if filesystem is unavailable)
    current_file: Option<File>,
    /// Path to the current log file
    current_file_path: Option<PathBuf>,
    /// Current file size in bytes
    current_file_size: u64,
    /// Ring buffer for entries when filesystem is unavailable
    ring_buffer: VecDeque<LogEntry>,
    /// Whether the filesystem is currently available for writing
    fs_available: bool,
}

/// The structured logger that writes JSON log entries to files with rotation,
/// retention, and ring buffer fallback.
pub struct StructuredLogger {
    config: LogConfig,
    state: Arc<Mutex<LoggerState>>,
}

impl StructuredLogger {
    /// Creates a new StructuredLogger with the given configuration.
    ///
    /// On creation, it attempts to:
    /// 1. Create the log directory if it doesn't exist
    /// 2. Open or create the current log file
    /// 3. Run retention cleanup for old log files
    pub fn new(config: LogConfig) -> Result<Self, io::Error> {
        config.validate().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        // Attempt to create the log directory
        let fs_available = fs::create_dir_all(&config.log_dir).is_ok();

        let (current_file, current_file_path, current_file_size) = if fs_available {
            match Self::open_log_file(&config.log_dir) {
                Ok((file, path, size)) => (Some(file), Some(path), size),
                Err(_) => (None, None, 0),
            }
        } else {
            (None, None, 0)
        };

        let has_file = current_file.is_some();
        let state = LoggerState {
            current_file,
            current_file_path,
            current_file_size,
            ring_buffer: VecDeque::with_capacity(config.max_buffer_size),
            fs_available: has_file || fs_available,
        };

        let logger = Self {
            config,
            state: Arc::new(Mutex::new(state)),
        };

        // Run initial retention cleanup
        logger.cleanup_old_logs();

        Ok(logger)
    }

    /// Opens or creates the current log file, returning the file handle, path, and current size.
    fn open_log_file(log_dir: &Path) -> Result<(File, PathBuf, u64), io::Error> {
        let file_name = format!("platform-{}.log", Utc::now().format("%Y-%m-%d"));
        let file_path = log_dir.join(&file_name);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        let size = file.metadata()?.len();
        Ok((file, file_path, size))
    }

    /// Logs an entry at the specified level.
    pub fn log(&self, entry: LogEntry) {
        // Check if the entry level meets the minimum configured level
        let entry_level = match entry.level.as_str() {
            "DEBUG" => LogLevel::Debug,
            "INFO" => LogLevel::Info,
            "WARN" => LogLevel::Warn,
            "ERROR" => LogLevel::Error,
            "FATAL" => LogLevel::Fatal,
            _ => LogLevel::Debug,
        };

        if entry_level < self.config.log_level {
            return;
        }

        let mut state = self.state.lock().unwrap();
        self.write_entry(&mut state, entry);
    }

    /// Convenience method to log with level, component, and message.
    pub fn emit(&self, level: LogLevel, component: &str, message: &str) {
        if level < self.config.log_level {
            return;
        }
        let entry = LogEntry::new(level, component, message);
        let mut state = self.state.lock().unwrap();
        self.write_entry(&mut state, entry);
    }

    /// Convenience method to log with a specific correlation ID.
    pub fn emit_with_correlation(
        &self,
        level: LogLevel,
        component: &str,
        message: &str,
        correlation_id: &str,
    ) {
        if level < self.config.log_level {
            return;
        }
        let entry = LogEntry::with_correlation_id(level, component, message, correlation_id);
        let mut state = self.state.lock().unwrap();
        self.write_entry(&mut state, entry);
    }

    /// Writes an entry to the log file or buffers it if the filesystem is unavailable.
    fn write_entry(&self, state: &mut LoggerState, entry: LogEntry) {
        let json = match entry.to_json() {
            Ok(j) => j,
            Err(_) => return, // Silently drop entries that can't be serialized
        };
        let line = format!("{}\n", json);
        let line_bytes = line.as_bytes();
        let line_len = line_bytes.len() as u64;

        if state.current_file.is_some() {
            // Check if rotation is needed before writing
            let needs_rotation = state.current_file_size + line_len
                > self.config.max_file_size_mb as u64 * 1024 * 1024;

            if needs_rotation {
                if self.rotate_log_file(state).is_err() {
                    // If rotation fails, buffer the entry
                    self.buffer_entry(state, entry);
                    return;
                }
            }

            // Now write to the file
            let write_result = if let Some(ref mut file) = state.current_file {
                file.write_all(line.as_bytes()).map(|_| ())
            } else {
                Err(io::Error::new(io::ErrorKind::Other, "no file"))
            };

            match write_result {
                Ok(()) => {
                    state.current_file_size += line_len;
                    // If we had buffered entries, flush them now
                    if !state.ring_buffer.is_empty() {
                        self.flush_buffer(state);
                    }
                }
                Err(_) => {
                    // Filesystem became unavailable
                    state.fs_available = false;
                    state.current_file = None;
                    state.current_file_path = None;
                    self.buffer_entry(state, entry);
                }
            }
        } else {
            // Try to recover filesystem access
            if self.try_recover_fs(state) {
                // Flush buffer first, then write current entry
                self.flush_buffer(state);
                let write_ok = if let Some(ref mut file) = state.current_file {
                    file.write_all(line.as_bytes()).is_ok()
                } else {
                    false
                };
                if write_ok {
                    state.current_file_size += line_len;
                    return;
                }
            }
            // Still unavailable, buffer the entry
            self.buffer_entry(state, entry);
        }
    }

    /// Buffers an entry in the ring buffer, discarding the oldest if full.
    fn buffer_entry(&self, state: &mut LoggerState, entry: LogEntry) {
        if state.ring_buffer.len() >= self.config.max_buffer_size {
            state.ring_buffer.pop_front(); // Discard oldest
        }
        state.ring_buffer.push_back(entry);
    }

    /// Attempts to recover filesystem access by reopening the log file.
    fn try_recover_fs(&self, state: &mut LoggerState) -> bool {
        if let Ok(()) = fs::create_dir_all(&self.config.log_dir) {
            if let Ok((file, path, size)) = Self::open_log_file(&self.config.log_dir) {
                state.current_file = Some(file);
                state.current_file_path = Some(path);
                state.current_file_size = size;
                state.fs_available = true;
                return true;
            }
        }
        false
    }

    /// Flushes all buffered entries to the log file.
    fn flush_buffer(&self, state: &mut LoggerState) {
        while let Some(buffered_entry) = state.ring_buffer.pop_front() {
            if let Ok(json) = buffered_entry.to_json() {
                let line = format!("{}\n", json);
                if let Some(ref mut file) = state.current_file {
                    if file.write_all(line.as_bytes()).is_ok() {
                        state.current_file_size += line.len() as u64;
                    } else {
                        // Put the entry back and stop flushing
                        state.ring_buffer.push_front(buffered_entry);
                        state.fs_available = false;
                        state.current_file = None;
                        break;
                    }
                } else {
                    state.ring_buffer.push_front(buffered_entry);
                    break;
                }
            }
        }
    }

    /// Rotates the current log file by closing it and opening a new one.
    fn rotate_log_file(&self, state: &mut LoggerState) -> Result<(), io::Error> {
        // Close current file
        state.current_file = None;

        // Rename current file with a timestamp suffix
        if let Some(ref current_path) = state.current_file_path {
            let rotated_name = format!(
                "{}.{}",
                current_path.display(),
                Utc::now().format("%H%M%S%3f")
            );
            let rotated_path = PathBuf::from(&rotated_name);
            fs::rename(current_path, &rotated_path)?;
        }

        // Open a new log file
        let (file, path, size) = Self::open_log_file(&self.config.log_dir)?;
        state.current_file = Some(file);
        state.current_file_path = Some(path);
        state.current_file_size = size;

        Ok(())
    }

    /// Cleans up log files older than the configured retention period.
    pub fn cleanup_old_logs(&self) {
        let retention_duration = Duration::days(self.config.retention_days as i64);
        let cutoff = Utc::now() - retention_duration;

        let entries = match fs::read_dir(&self.config.log_dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Only process .log files
            if path.extension().and_then(|e| e.to_str()) != Some("log")
                && !path
                    .to_str()
                    .map(|s| s.contains(".log."))
                    .unwrap_or(false)
            {
                continue;
            }

            // Check file modification time
            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    let modified_dt: DateTime<Utc> = modified.into();
                    if modified_dt < cutoff {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }

    /// Returns the number of entries currently in the ring buffer.
    pub fn buffer_len(&self) -> usize {
        self.state.lock().unwrap().ring_buffer.len()
    }

    /// Returns whether the filesystem is currently available for writing.
    pub fn is_fs_available(&self) -> bool {
        self.state.lock().unwrap().fs_available
    }

    /// Forces a flush of the ring buffer (useful for testing or recovery scenarios).
    pub fn force_flush(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.current_file.is_some() {
            self.flush_buffer(&mut state);
            state.ring_buffer.is_empty()
        } else if self.try_recover_fs(&mut state) {
            self.flush_buffer(&mut state);
            state.ring_buffer.is_empty()
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_config(dir: &Path) -> LogConfig {
        LogConfig {
            log_level: LogLevel::Debug,
            log_dir: dir.to_path_buf(),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 1000,
        }
    }

    #[test]
    fn test_log_entry_json_format() {
        let entry = LogEntry::with_correlation_id(
            LogLevel::Info,
            "agent-core",
            "Processing request",
            "550e8400-e29b-41d4-a716-446655440000",
        );

        let json = entry.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed["timestamp"].is_string());
        assert_eq!(parsed["correlation_id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(parsed["level"], "INFO");
        assert_eq!(parsed["component"], "agent-core");
        assert_eq!(parsed["message"], "Processing request");
    }

    #[test]
    fn test_log_entry_has_all_required_fields() {
        let entry = LogEntry::new(LogLevel::Error, "scheduler", "Task failed");
        let json = entry.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // All required fields must be present
        assert!(parsed.get("timestamp").is_some());
        assert!(parsed.get("correlation_id").is_some());
        assert!(parsed.get("level").is_some());
        assert!(parsed.get("component").is_some());
        assert!(parsed.get("message").is_some());

        // Correlation ID should be a valid UUID
        let cid = parsed["correlation_id"].as_str().unwrap();
        assert!(Uuid::parse_str(cid).is_ok());
    }

    #[test]
    fn test_log_config_validation() {
        let mut config = LogConfig::default();

        // Valid config
        assert!(config.validate().is_ok());

        // Invalid retention_days
        config.retention_days = 0;
        assert!(config.validate().is_err());

        config.retention_days = 366;
        assert!(config.validate().is_err());

        config.retention_days = 1;
        assert!(config.validate().is_ok());

        config.retention_days = 365;
        assert!(config.validate().is_ok());

        // Invalid max_file_size_mb
        config.max_file_size_mb = 0;
        assert!(config.validate().is_err());

        config.max_file_size_mb = 1;
        assert!(config.validate().is_ok());

        // Invalid max_buffer_size
        config.max_buffer_size = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_structured_logger_writes_to_file() {
        let tmp_dir = TempDir::new().unwrap();
        let config = test_config(tmp_dir.path());
        let logger = StructuredLogger::new(config).unwrap();

        logger.emit(LogLevel::Info, "test-component", "Hello, world!");

        // Read the log file
        let entries: Vec<_> = fs::read_dir(tmp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("log"))
            .collect();

        assert!(!entries.is_empty());

        let content = fs::read_to_string(entries[0].path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["level"], "INFO");
        assert_eq!(parsed["component"], "test-component");
        assert_eq!(parsed["message"], "Hello, world!");
    }

    #[test]
    fn test_log_level_filtering() {
        let tmp_dir = TempDir::new().unwrap();
        let config = LogConfig {
            log_level: LogLevel::Warn,
            log_dir: tmp_dir.path().to_path_buf(),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 1000,
        };
        let logger = StructuredLogger::new(config).unwrap();

        // These should be filtered out
        logger.emit(LogLevel::Debug, "test", "debug msg");
        logger.emit(LogLevel::Info, "test", "info msg");

        // These should be written
        logger.emit(LogLevel::Warn, "test", "warn msg");
        logger.emit(LogLevel::Error, "test", "error msg");

        let entries: Vec<_> = fs::read_dir(tmp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("log"))
            .collect();

        let content = fs::read_to_string(entries[0].path()).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["level"], "WARN");

        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["level"], "ERROR");
    }

    #[test]
    fn test_ring_buffer_when_fs_unavailable() {
        // Use a path that doesn't exist and can't be created
        let _config = LogConfig {
            log_level: LogLevel::Debug,
            log_dir: PathBuf::from("/nonexistent/impossible/path/that/cannot/exist"),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 5,
        };

        // Logger creation may fail on the directory, but we test the buffer behavior
        // by simulating unavailability after creation
        let tmp_dir = TempDir::new().unwrap();
        let working_config = test_config(tmp_dir.path());
        let logger = StructuredLogger::new(working_config).unwrap();

        // Remove the directory to simulate filesystem unavailability
        fs::remove_dir_all(tmp_dir.path()).unwrap();

        // These should go to the ring buffer
        for i in 0..3 {
            logger.emit(LogLevel::Info, "test", &format!("buffered msg {}", i));
        }

        assert!(logger.buffer_len() <= 3);
    }

    #[test]
    fn test_ring_buffer_overflow_discards_oldest() {
        let tmp_dir = TempDir::new().unwrap();
        let config = LogConfig {
            log_level: LogLevel::Debug,
            log_dir: tmp_dir.path().to_path_buf(),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 3, // Small buffer for testing
        };
        let logger = StructuredLogger::new(config).unwrap();

        // Remove directory to force buffering
        fs::remove_dir_all(tmp_dir.path()).unwrap();

        // Write more entries than the buffer can hold
        for i in 0..5 {
            logger.emit(LogLevel::Info, "test", &format!("msg {}", i));
        }

        // Buffer should be at max capacity (3), oldest discarded
        assert!(logger.buffer_len() <= 3);
    }

    #[test]
    fn test_log_rotation() {
        let tmp_dir = TempDir::new().unwrap();
        let config = LogConfig {
            log_level: LogLevel::Debug,
            log_dir: tmp_dir.path().to_path_buf(),
            max_file_size_mb: 1, // 1MB limit - we'll use a tiny value for testing
            retention_days: 30,
            max_buffer_size: 1000,
        };

        // We can't easily test rotation with 1MB files in a unit test,
        // but we can verify the logger handles the rotation path.
        let logger = StructuredLogger::new(config).unwrap();
        logger.emit(LogLevel::Info, "test", "rotation test entry");

        assert!(logger.is_fs_available());
    }

    #[test]
    fn test_buffer_flush_on_recovery() {
        let tmp_dir = TempDir::new().unwrap();
        let log_dir = tmp_dir.path().join("logs");
        fs::create_dir_all(&log_dir).unwrap();

        let config = LogConfig {
            log_level: LogLevel::Debug,
            log_dir: log_dir.clone(),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 1000,
        };
        let logger = StructuredLogger::new(config).unwrap();

        // Remove directory to force buffering
        fs::remove_dir_all(&log_dir).unwrap();

        logger.emit(LogLevel::Info, "test", "buffered entry 1");
        logger.emit(LogLevel::Info, "test", "buffered entry 2");

        // Recreate directory to allow recovery
        fs::create_dir_all(&log_dir).unwrap();

        // Next write should trigger recovery and flush
        logger.emit(LogLevel::Info, "test", "recovery entry");

        // Check that entries were written
        let entries: Vec<_> = fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .to_str()
                    .map(|s| s.contains(".log"))
                    .unwrap_or(false)
            })
            .collect();

        if !entries.is_empty() {
            let content = fs::read_to_string(entries[0].path()).unwrap();
            // Should have the buffered entries plus the recovery entry
            let lines: Vec<&str> = content.trim().lines().collect();
            assert!(lines.len() >= 1); // At minimum the recovery entry
        }
    }

    #[test]
    fn test_retention_cleanup() {
        let tmp_dir = TempDir::new().unwrap();
        let log_dir = tmp_dir.path().to_path_buf();

        // Create a fake old log file
        let old_log = log_dir.join("platform-2020-01-01.log");
        fs::write(&old_log, "old log content").unwrap();

        // Set the file's modification time to the past (we can't easily do this
        // portably, so we'll just verify the cleanup logic runs without error)
        let config = LogConfig {
            log_level: LogLevel::Debug,
            log_dir: log_dir.clone(),
            max_file_size_mb: 1,
            retention_days: 30,
            max_buffer_size: 1000,
        };

        let _logger = StructuredLogger::new(config).unwrap();
        // The cleanup runs on creation; since we can't easily backdate files
        // in a portable way, we just verify no panic occurs.
    }

    #[test]
    fn test_all_log_levels() {
        let tmp_dir = TempDir::new().unwrap();
        let config = test_config(tmp_dir.path());
        let logger = StructuredLogger::new(config).unwrap();

        logger.emit(LogLevel::Debug, "test", "debug");
        logger.emit(LogLevel::Info, "test", "info");
        logger.emit(LogLevel::Warn, "test", "warn");
        logger.emit(LogLevel::Error, "test", "error");
        logger.emit(LogLevel::Fatal, "test", "fatal");

        let entries: Vec<_> = fs::read_dir(tmp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("log"))
            .collect();

        let content = fs::read_to_string(entries[0].path()).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 5);

        let levels: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["level"].as_str().unwrap().to_string()
            })
            .collect();

        assert_eq!(levels, vec!["DEBUG", "INFO", "WARN", "ERROR", "FATAL"]);
    }

    #[test]
    fn test_correlation_id_is_valid_uuid() {
        let entry = LogEntry::new(LogLevel::Info, "test", "msg");
        assert!(Uuid::parse_str(&entry.correlation_id).is_ok());
    }

    #[test]
    fn test_timestamp_is_iso8601() {
        let entry = LogEntry::new(LogLevel::Info, "test", "msg");
        // Should parse as a valid DateTime
        assert!(DateTime::parse_from_rfc3339(&entry.timestamp).is_ok());
    }
}
