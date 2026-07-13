use crate::types::SessionId;
use std::sync::Arc;

/// Tracks asynchronous background work that is still running for each session.
#[derive(Debug, Default)]
pub struct BgTaskTracker {
    counts: dashmap::DashMap<SessionId, usize>,
}

impl BgTaskTracker {
    pub fn start(self: &Arc<Self>, session_id: SessionId) -> BgTaskGuard {
        self.counts
            .entry(session_id.clone())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        BgTaskGuard {
            tracker: Arc::clone(self),
            session_id,
        }
    }

    pub fn is_running(&self, session_id: &SessionId) -> bool {
        self.counts.get(session_id).is_some_and(|count| *count > 0)
    }
}

pub struct BgTaskGuard {
    tracker: Arc<BgTaskTracker>,
    session_id: SessionId,
}

impl Drop for BgTaskGuard {
    fn drop(&mut self) {
        if let dashmap::mapref::entry::Entry::Occupied(mut entry) =
            self.tracker.counts.entry(self.session_id.clone())
        {
            if *entry.get() <= 1 {
                entry.remove();
            } else {
                *entry.get_mut() -= 1;
            }
        }
    }
}

#[cfg(test)]
#[path = "bg_task_test.rs"]
mod tests;
