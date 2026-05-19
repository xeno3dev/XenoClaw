//! File system change triggers using the `notify` crate.
//!
//! Watches configured paths for creation, modification, and deletion events
//! and triggers associated tasks when changes are detected.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use notify::{
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};

use common::models::{FsEvent, TaskTrigger};
use common::types::TaskId;

/// A triggered file system event with associated task information.
#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    /// The task that should be triggered.
    pub task_id: TaskId,
    /// The path that changed.
    pub path: PathBuf,
    /// The type of filesystem event.
    pub event: FsEvent,
}

/// Manages file system watchers for task triggers.
pub struct FileWatcher {
    /// Active watcher instance.
    watcher: Arc<Mutex<Option<RecommendedWatcher>>>,
    /// Map of watched paths to their associated task IDs and event filters.
    watches: Arc<RwLock<HashMap<TaskId, WatchConfig>>>,
    /// Channel for sending triggered events to the scheduler.
    event_tx: mpsc::UnboundedSender<FileChangeEvent>,
}

#[derive(Debug, Clone)]
struct WatchConfig {
    paths: Vec<PathBuf>,
    events: Vec<FsEvent>,
}

impl FileWatcher {
    /// Create a new FileWatcher with the given event channel.
    pub fn new(event_tx: mpsc::UnboundedSender<FileChangeEvent>) -> Self {
        Self {
            watcher: Arc::new(Mutex::new(None)),
            watches: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    /// Start the file watcher background process.
    pub async fn start(&self) -> Result<(), FileWatcherError> {
        let watches = Arc::clone(&self.watches);
        let event_tx = self.event_tx.clone();

        // Create a channel for notify events
        let (notify_tx, mut notify_rx) = mpsc::unbounded_channel();

        // Create the watcher
        let watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let _ = notify_tx.send(event);
                }
            },
            Config::default(),
        )
        .map_err(|e| FileWatcherError::InitFailed(e.to_string()))?;

        *self.watcher.lock().await = Some(watcher);

        // Spawn the event processing task
        tokio::spawn(async move {
            while let Some(event) = notify_rx.recv().await {
                Self::process_notify_event(&watches, &event_tx, event).await;
            }
        });

        info!("File watcher started");
        Ok(())
    }

    /// Register a task's file change trigger.
    pub async fn watch_task(&self, task_id: TaskId, trigger: &TaskTrigger) -> Result<(), FileWatcherError> {
        if let TaskTrigger::FileChange { paths, events } = trigger {
            let config = WatchConfig {
                paths: paths.clone(),
                events: events.clone(),
            };

            // Add paths to the watcher
            if let Some(watcher) = self.watcher.lock().await.as_mut() {
                for path in &config.paths {
                    if path.exists() {
                        watcher
                            .watch(path, RecursiveMode::NonRecursive)
                            .map_err(|e| FileWatcherError::WatchFailed {
                                path: path.clone(),
                                reason: e.to_string(),
                            })?;
                        debug!("Watching path: {:?} for task {}", path, task_id);
                    } else {
                        // Watch the parent directory for creation events
                        if let Some(parent) = path.parent() {
                            if parent.exists() {
                                watcher
                                    .watch(parent, RecursiveMode::NonRecursive)
                                    .map_err(|e| FileWatcherError::WatchFailed {
                                        path: parent.to_path_buf(),
                                        reason: e.to_string(),
                                    })?;
                                debug!(
                                    "Watching parent {:?} for creation of {:?} for task {}",
                                    parent, path, task_id
                                );
                            } else {
                                warn!(
                                    "Cannot watch path {:?}: neither path nor parent exists",
                                    path
                                );
                            }
                        }
                    }
                }
            } else {
                return Err(FileWatcherError::NotStarted);
            }

            self.watches.write().await.insert(task_id, config);
            Ok(())
        } else {
            Err(FileWatcherError::InvalidTrigger)
        }
    }

    /// Unregister a task's file change trigger.
    pub async fn unwatch_task(&self, task_id: &TaskId) -> Result<(), FileWatcherError> {
        if let Some(config) = self.watches.write().await.remove(task_id) {
            if let Some(watcher) = self.watcher.lock().await.as_mut() {
                for path in &config.paths {
                    let target = if path.exists() {
                        path.clone()
                    } else {
                        path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
                    };
                    if target.exists() {
                        let _ = watcher.unwatch(&target);
                    }
                }
            }
            debug!("Unwatched task {}", task_id);
        }
        Ok(())
    }

    /// Stop the file watcher.
    pub async fn stop(&self) {
        *self.watcher.lock().await = None;
        self.watches.write().await.clear();
        info!("File watcher stopped");
    }

    /// Process a raw notify event and dispatch to matching tasks.
    async fn process_notify_event(
        watches: &RwLock<HashMap<TaskId, WatchConfig>>,
        event_tx: &mpsc::UnboundedSender<FileChangeEvent>,
        event: Event,
    ) {
        let fs_event = match event.kind {
            EventKind::Create(_) => Some(FsEvent::Created),
            EventKind::Modify(_) => Some(FsEvent::Modified),
            EventKind::Remove(_) => Some(FsEvent::Deleted),
            _ => None,
        };

        let Some(fs_event) = fs_event else {
            return;
        };

        let watches = watches.read().await;
        for (task_id, config) in watches.iter() {
            // Check if this event matches the task's watched paths and event types
            if !config.events.contains(&fs_event) {
                continue;
            }

            for event_path in &event.paths {
                let matches = config.paths.iter().any(|watched_path| {
                    // Match if the event path is the watched path or is inside it
                    event_path == watched_path
                        || event_path.starts_with(watched_path)
                        || watched_path
                            .file_name()
                            .map(|name| event_path.ends_with(name))
                            .unwrap_or(false)
                });

                if matches {
                    let change_event = FileChangeEvent {
                        task_id: *task_id,
                        path: event_path.clone(),
                        event: fs_event,
                    };
                    if let Err(e) = event_tx.send(change_event) {
                        error!("Failed to send file change event: {}", e);
                    }
                }
            }
        }
    }
}

/// Errors specific to the file watcher subsystem.
#[derive(Debug, thiserror::Error)]
pub enum FileWatcherError {
    #[error("File watcher initialization failed: {0}")]
    InitFailed(String),

    #[error("File watcher not started")]
    NotStarted,

    #[error("Failed to watch path '{}': {reason}", path.display())]
    WatchFailed { path: PathBuf, reason: String },

    #[error("Invalid trigger type for file watcher")]
    InvalidTrigger,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_file_watcher_creation() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(tx);
        assert!(watcher.start().await.is_ok());
    }

    #[tokio::test]
    async fn test_watch_task_with_existing_path() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(tx);
        watcher.start().await.unwrap();

        let tmp_dir = TempDir::new().unwrap();
        let task_id = TaskId::new();
        let trigger = TaskTrigger::FileChange {
            paths: vec![tmp_dir.path().to_path_buf()],
            events: vec![FsEvent::Created, FsEvent::Modified],
        };

        let result = watcher.watch_task(task_id, &trigger).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_watch_task_invalid_trigger() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(tx);
        watcher.start().await.unwrap();

        let task_id = TaskId::new();
        let trigger = TaskTrigger::Cron {
            expression: "0 * * * * *".to_string(),
        };

        let result = watcher.watch_task(task_id, &trigger).await;
        assert!(matches!(result, Err(FileWatcherError::InvalidTrigger)));
    }

    #[tokio::test]
    async fn test_file_creation_triggers_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(tx);
        watcher.start().await.unwrap();

        let tmp_dir = TempDir::new().unwrap();
        let task_id = TaskId::new();
        let trigger = TaskTrigger::FileChange {
            paths: vec![tmp_dir.path().to_path_buf()],
            events: vec![FsEvent::Created],
        };

        watcher.watch_task(task_id, &trigger).await.unwrap();

        // Give the watcher time to set up
        sleep(Duration::from_millis(100)).await;

        // Create a file in the watched directory
        let file_path = tmp_dir.path().join("test.txt");
        fs::write(&file_path, "hello").unwrap();

        // Wait for the event (with timeout)
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(event.is_ok());
        if let Ok(Some(event)) = event {
            assert_eq!(event.task_id, task_id);
        }
    }

    #[tokio::test]
    async fn test_unwatch_task() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let watcher = FileWatcher::new(tx);
        watcher.start().await.unwrap();

        let tmp_dir = TempDir::new().unwrap();
        let task_id = TaskId::new();
        let trigger = TaskTrigger::FileChange {
            paths: vec![tmp_dir.path().to_path_buf()],
            events: vec![FsEvent::Modified],
        };

        watcher.watch_task(task_id, &trigger).await.unwrap();
        let result = watcher.unwatch_task(&task_id).await;
        assert!(result.is_ok());
    }
}
