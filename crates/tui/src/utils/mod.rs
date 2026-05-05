//! Utility functions for the TUI crate

pub mod clipboard;
pub mod text;

// Re-export from kernel for consistency
pub use kernel::utils::{strs, tokens};

use std::time::{Duration, Instant};

/// A message with an optional timeout for automatic expiration.
/// Used for notifications and tips that should disappear after a duration.
#[derive(Debug, Clone)]
pub struct TimedMessage<T> {
    content: Option<T>,
    timeout: Option<Instant>,
}

impl<T> Default for TimedMessage<T> {
    fn default() -> Self {
        Self {
            content: None,
            timeout: None,
        }
    }
}

impl<T> TimedMessage<T> {
    /// Create a new timed message with no timeout (persistent)
    pub fn new(content: T) -> Self {
        Self {
            content: Some(content),
            timeout: None,
        }
    }

    /// Create a new timed message with a duration
    pub fn with_timeout(content: T, duration: Duration) -> Self {
        Self {
            content: Some(content),
            timeout: Some(Instant::now() + duration),
        }
    }

    /// Set content with no timeout
    pub fn set(&mut self, content: T) {
        self.content = Some(content);
        self.timeout = None;
    }

    /// Set content with a duration
    pub fn set_with_timeout(&mut self, content: T, duration: Duration) {
        self.content = Some(content);
        self.timeout = Some(Instant::now() + duration);
    }

    /// Clear the message
    pub fn clear(&mut self) {
        self.content = None;
        self.timeout = None;
    }

    /// Check if the message has expired and clear if so
    /// Returns true if the message was cleared
    pub fn check_timeout(&mut self) -> bool {
        if let Some(timeout) = self.timeout {
            if Instant::now() > timeout {
                self.clear();
                return true;
            }
        }
        false
    }

    /// Get the content if it hasn't expired
    pub fn content(&self) -> Option<&T> {
        self.content.as_ref()
    }

    /// Check if there is an active (non-expired) message
    pub fn is_active(&self) -> bool {
        if self.content.is_none() {
            return false;
        }
        if let Some(timeout) = self.timeout {
            Instant::now() <= timeout
        } else {
            true
        }
    }

    /// Take the content, clearing it from the message
    pub fn take(&mut self) -> Option<T> {
        self.timeout = None;
        self.content.take()
    }
}
