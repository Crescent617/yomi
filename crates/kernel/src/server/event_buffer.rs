use crate::event::{ContentChunk, Event, ModelEvent};
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

    /// Get events after `after` (exclusive), coalescing consecutive streaming
    /// events (model chunks / tool-call deltas) so that replay on re-subscribe
    /// stays small. The buffer itself keeps raw events, so `after` can match
    /// any event id ever forwarded to a client.
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

        let mut out: Vec<Envelope> = Vec::new();
        for envelope in &buf[start..] {
            if let Some(last) = out.last_mut() {
                if try_merge(last, envelope) {
                    continue;
                }
            }
            out.push(envelope.clone());
        }
        out
    }
}

/// Try to merge `incoming` into `last` in place. Returns `true` on success.
///
/// Mergeable pairs (must be consecutive):
/// - `ModelEvent::Chunk` Text+Text or Thinking+Thinking with the same `message_id`
/// - `ModelEvent::ToolCallDelta` with the same `message_id` and `tool_id`
///
/// On merge the newer `event_id` is kept, so a client resuming from the last
/// event id it saw will not receive the merged content again.
fn try_merge(last: &mut Envelope, incoming: &Envelope) -> bool {
    match (&mut last.event, &incoming.event) {
        (
            Event::Model(ModelEvent::Chunk {
                message_id: last_mid,
                content: last_content,
            }),
            Event::Model(ModelEvent::Chunk {
                message_id: new_mid,
                content: new_content,
            }),
        ) if last_mid == new_mid => {
            match (last_content, new_content) {
                (ContentChunk::Text(acc), ContentChunk::Text(delta)) => {
                    acc.push_str(delta);
                }
                (
                    ContentChunk::Thinking {
                        thinking: acc,
                        signature: acc_sig,
                    },
                    ContentChunk::Thinking {
                        thinking: delta,
                        signature: delta_sig,
                    },
                ) => {
                    acc.push_str(delta);
                    if delta_sig.is_some() {
                        acc_sig.clone_from(delta_sig);
                    }
                }
                _ => return false,
            }
            last.event_id = incoming.event_id.clone();
            true
        }
        (
            Event::Model(ModelEvent::ToolCallDelta {
                message_id: last_mid,
                tool_id: last_tid,
                arguments_delta: acc,
                ..
            }),
            Event::Model(ModelEvent::ToolCallDelta {
                message_id: new_mid,
                tool_id: new_tid,
                arguments_delta: delta,
                ..
            }),
        ) if last_mid == new_mid && last_tid == new_tid => {
            acc.push_str(delta);
            last.event_id = incoming.event_id.clone();
            true
        }
        _ => false,
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
