use crate::agent::AgentState;
use crate::event::StopReason;
use crate::types::SessionId;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Lightweight notification broadcast to all connected clients.
/// Separate from Event — consumed by inactive session tabs for
/// summary updates (e.g. phase changes). Pushed automatically on
/// every connection; no explicit subscribe/unsubscribe RPC needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notification {
    StateChanged {
        session_id: SessionId,
        status: AgentState,
    },
    TitleUpdated {
        session_id: SessionId,
        title: String,
    },
    BackgroundTasksChanged {
        session_id: SessionId,
        kind: crate::agent::BackgroundTaskKind,
    },
    ConnectionLost {
        session_id: SessionId,
    },
    AgentActivity {
        session_id: SessionId,
        event_id: String,
        activity: AgentActivity,
    },
    /// Mailbox pending counts changed (enqueue/consume/remove/clear) —
    /// session-list pending badges and mailbox views refresh on this.
    MailboxChanged {
        session_id: SessionId,
        steer: usize,
        queued: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentActivity {
    PermissionRequested {
        req_id: String,
        target_session_id: String,
    },
    AskUserRequested {
        req_id: String,
        target_session_id: String,
    },
    RequestResolved {
        req_id: String,
    },
    Started,
    Stopped {
        reason: StopReason,
    },
}

/// Broadcast bus for notifications.
#[derive(Clone, Debug)]
pub struct NotificationBus {
    tx: broadcast::Sender<Notification>,
}

impl NotificationBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.tx.subscribe()
    }

    pub fn send(
        &self,
        noti: Notification,
    ) -> Result<usize, broadcast::error::SendError<Notification>> {
        self.tx.send(noti)
    }
}

impl Default for NotificationBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "notification_test.rs"]
mod tests;
