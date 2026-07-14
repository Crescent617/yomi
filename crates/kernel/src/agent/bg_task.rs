use crate::notification::{Notification, NotificationBus};
use crate::types::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// Kind of asynchronous background work tracked for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskKind {
    Subagent,
    Shell,
}

/// Runtime details for a background shell command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackgroundShellTask {
    pub task_id: String,
    pub session_id: SessionId,
    pub pid: u32,
    pub command: String,
    pub output_path: String,
    pub started_at: DateTime<Utc>,
}

/// Tracks asynchronous background work that is still running for each session.
#[derive(Debug, Default)]
pub struct BgTaskTracker {
    counts: dashmap::DashMap<(SessionId, BackgroundTaskKind), usize>,
    shell_tasks: dashmap::DashMap<String, BackgroundShellTask>,
    notification_bus: OnceLock<Arc<NotificationBus>>,
}

impl BgTaskTracker {
    pub fn set_notification_bus(&self, notification_bus: Arc<NotificationBus>) {
        let _ = self.notification_bus.set(notification_bus);
    }

    pub fn start(self: &Arc<Self>, session_id: SessionId, kind: BackgroundTaskKind) -> BgTaskGuard {
        self.increment(&session_id, kind);
        BgTaskGuard {
            tracker: Arc::clone(self),
            session_id,
            kind,
            task_id: None,
        }
    }

    pub fn start_shell(self: &Arc<Self>, task: BackgroundShellTask) -> BgTaskGuard {
        let session_id = task.session_id.clone();
        let task_id = task.task_id.clone();
        self.shell_tasks.insert(task_id.clone(), task);
        self.increment(&session_id, BackgroundTaskKind::Shell);
        BgTaskGuard {
            tracker: Arc::clone(self),
            session_id,
            kind: BackgroundTaskKind::Shell,
            task_id: Some(task_id),
        }
    }

    pub fn count(&self, session_id: &SessionId, kind: BackgroundTaskKind) -> usize {
        self.counts
            .get(&(session_id.clone(), kind))
            .map_or(0, |count| *count)
    }

    pub fn shell_tasks(&self) -> Vec<BackgroundShellTask> {
        let mut tasks: Vec<_> = self
            .shell_tasks
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        tasks.sort_by_key(|task| task.started_at);
        tasks
    }

    pub fn is_running(&self, session_id: &SessionId) -> bool {
        self.count(session_id, BackgroundTaskKind::Subagent) > 0
            || self.count(session_id, BackgroundTaskKind::Shell) > 0
    }

    fn increment(&self, session_id: &SessionId, kind: BackgroundTaskKind) {
        self.counts
            .entry((session_id.clone(), kind))
            .and_modify(|count| *count += 1)
            .or_insert(1);
        self.notify(session_id, kind);
    }

    fn notify(&self, session_id: &SessionId, kind: BackgroundTaskKind) {
        if let Some(bus) = self.notification_bus.get() {
            let _ = bus.send(Notification::BackgroundTasksChanged {
                session_id: session_id.clone(),
                kind,
            });
        }
    }
}

pub struct BgTaskGuard {
    tracker: Arc<BgTaskTracker>,
    session_id: SessionId,
    kind: BackgroundTaskKind,
    task_id: Option<String>,
}

impl Drop for BgTaskGuard {
    fn drop(&mut self) {
        if let Some(task_id) = &self.task_id {
            self.tracker.shell_tasks.remove(task_id);
        }

        let key = (self.session_id.clone(), self.kind);
        let removed = if let dashmap::mapref::entry::Entry::Occupied(mut entry) =
            self.tracker.counts.entry(key)
        {
            if *entry.get() <= 1 {
                entry.remove();
            } else {
                *entry.get_mut() -= 1;
            }
            true
        } else {
            false
        };
        if removed {
            self.tracker.notify(&self.session_id, self.kind);
        }
    }
}

#[cfg(test)]
#[path = "bg_task_test.rs"]
mod tests;
