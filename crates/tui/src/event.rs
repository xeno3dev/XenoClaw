//! Event handling for the TUI.
//!
//! Manages terminal events (key presses, resize) and periodic ticks
//! for status refresh. Uses crossterm's event-stream feature for async
//! event polling, which handles SSH latency gracefully by buffering input.

use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};
use futures::StreamExt;

/// Application events that drive the main loop.
#[derive(Debug)]
pub enum AppEvent {
    /// Periodic tick for status refresh and reconnection logic.
    Tick,
    /// A keyboard event.
    Key(KeyEvent),
    /// Terminal was resized.
    Resize(u16, u16),
}

/// Handles terminal events and produces AppEvents.
///
/// Uses a tick rate to ensure periodic status updates even when no
/// user input is received. The event stream approach handles SSH latency
/// by buffering keystrokes at the terminal level.
pub struct EventHandler {
    /// Interval between tick events.
    tick_rate: Duration,
}

impl EventHandler {
    /// Create a new EventHandler with the given tick rate.
    ///
    /// The tick rate controls how often status refreshes occur.
    /// A 100ms tick rate provides responsive UI while the actual
    /// status refresh happens every 2 seconds (controlled by App).
    pub fn new(_status_refresh_interval: Duration) -> Self {
        Self {
            // Use a fast tick rate for responsive UI; the App controls
            // when to actually refresh status based on elapsed time.
            tick_rate: Duration::from_millis(100),
        }
    }

    /// Wait for and return the next application event.
    ///
    /// This method polls for terminal events with a timeout equal to
    /// the tick rate. If no event arrives within the timeout, a Tick
    /// event is returned to drive periodic updates.
    ///
    /// Input buffering for SSH latency:
    /// - crossterm buffers keystrokes at the OS/terminal level
    /// - Even with 500ms SSH latency, keystrokes are queued and delivered
    ///   in order without loss
    /// - The event stream processes all buffered events on each poll
    pub async fn next(&mut self) -> AppEvent {
        // Use tokio::select to race between event polling and tick timeout
        let tick_delay = tokio::time::sleep(self.tick_rate);
        tokio::pin!(tick_delay);

        let mut event_stream = crossterm::event::EventStream::new();

        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => match event {
                        Event::Key(key) => AppEvent::Key(key),
                        Event::Resize(w, h) => AppEvent::Resize(w, h),
                        _ => AppEvent::Tick,
                    },
                    _ => AppEvent::Tick,
                }
            }
            _ = &mut tick_delay => {
                AppEvent::Tick
            }
        }
    }
}

/// Drain all pending events from the terminal event queue.
///
/// This is useful after operations that may have caused events to
/// accumulate (e.g., during a reconnection attempt), preventing
/// stale events from being processed.
pub fn drain_pending_events() {
    while event::poll(Duration::from_millis(0)).unwrap_or(false) {
        let _ = event::read();
    }
}
