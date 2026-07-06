use crate::types::{EventId, SessionId};
use crate::wire::Envelope;
use dashmap::DashMap;
use tokio::sync::mpsc;

/// Per-session event buffer for replay on re-subscribe.
pub(crate) struct EventBuffer {
    max_size: usize,
    buffers: DashMap<SessionId, Vec<Envelope>>,
}

impl EventBuffer {
    pub(crate) fn new(max_size: usize) -> Self {
        Self {
            max_size,
            buffers: DashMap::new(),
        }
    }

    pub(crate) fn push(&self, envelope: Envelope) {
        let sid = envelope.session_id.clone();
        let mut entry = self.buffers.entry(sid).or_default();
        entry.push(envelope);
        if entry.len() > self.max_size {
            let to_drop = entry.len() - self.max_size;
            entry.drain(..to_drop);
        }
    }

    pub(crate) fn clear(&self, sid: &SessionId) {
        self.buffers.remove(sid);
    }

    pub(crate) fn remove(&self, sid: &SessionId) {
        self.buffers.remove(sid);
    }

    pub(crate) fn get_after(&self, sid: &SessionId, after: Option<&EventId>) -> Vec<Envelope> {
        let buf = match self.buffers.get(sid) {
            Some(b) => b,
            None => return Vec::new(),
        };
        let start = match after {
            Some(id) => match buf.binary_search_by(|e| e.event_id.cmp(id)) {
                Ok(idx) => idx + 1, // exclusive
                Err(idx) => idx,    // not found, start from where it would be
            },
            None => 0,
        };
        buf[start..].to_vec()
    }
}

/// Real-time subscribers per session. Managed by the global event-forwarder task.
pub(crate) struct SessionSubscribers {
    senders: DashMap<SessionId, mpsc::Sender<Envelope>>,
}

impl SessionSubscribers {
    pub(crate) fn new() -> Self {
        Self {
            senders: DashMap::new(),
        }
    }

    pub(crate) fn insert(&self, sid: &SessionId, sender: mpsc::Sender<Envelope>) {
        self.senders.insert(sid.clone(), sender);
    }

    pub(crate) fn remove(&self, sid: &SessionId) {
        self.senders.remove(sid);
    }

    pub(crate) fn try_send(&self, sid: &SessionId, envelope: &Envelope) {
        if let Some(entry) = self.senders.get(sid) {
            match entry.value().try_send(envelope.clone()) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    drop(entry);
                    self.senders.remove(sid);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    // receiver still alive but slow; keep the sender.
                    tracing::warn!(
                        session_id = %sid,
                        "event subscriber channel full, dropping event"
                    );
                }
            }
        }
    }
}
