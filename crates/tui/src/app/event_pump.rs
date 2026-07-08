use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use kernel::client::KernelApi;
use kernel::comms::EventBusSubscriber;
use kernel::event::{AgentEvent, AgentStatus, Event, ModelEvent, ToolEvent};
use kernel::permission::Level;
use kernel::types::SessionId;

/// Tagged event that carries provenance information.
///
/// `Main` events come from the primary session; `Subagent` events come from
/// a child agent and include the `parent_tool_id` so the UI can associate
/// them with the correct `Agent` tool call.
pub enum TaggedEvent {
    Main(Event),
    Subagent {
        parent_tool_id: String,
        session_id: String,
        event: Event,
    },
    Connected,
    ConnectionLost,
}

/// Transparent event pump that hides connection churn from the TUI.
///
/// Holds the kernel-side `EventBusSubscriber` and forwards events into
/// a stable `mpsc::Receiver`.  When the subscriber is closed
/// (daemon restart / network drop) the pump automatically re-subscribes
/// and restores the session — the TUI sees a continuous stream of events
/// plus explicit `Connected` / `ConnectionLost` notifications when the
/// connection state changes.
///
/// In addition, the pump dynamically subscribes to any subagent sessions
/// discovered via `ToolEvent::Metadata`, forwarding their events as
/// [`TaggedEvent::Subagent`].
pub(crate) struct EventPump {
    cancel: tokio_util::sync::CancellationToken,
}

impl Drop for EventPump {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl EventPump {
    pub fn spawn(
        initial_rx: EventBusSubscriber,
        kernel: Arc<dyn KernelApi>,
        session_id: String,
        _auto_approve: Level,
    ) -> (Self, mpsc::Receiver<TaggedEvent>) {
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            let sid = SessionId::from(session_id);
            let mut current_rx = Some(initial_rx);

            // Notify TUI that the initial connection is ready (only in daemon mode).
            if crate::daemon_mode() {
                if let Err(e) = tx.try_send(TaggedEvent::Connected) {
                    tracing::warn!("EventPump failed to send initial connected notification: {e}");
                }
            }

            'outer: loop {
                // When subscriber is closed, resubscribe (infinite retry).
                if current_rx.is_none() {
                    match Self::resubscribe(&kernel, &sid, _auto_approve, &cancel_for_task).await {
                        Some(new_rx) => {
                            tracing::info!("EventPump re-subscribed to {}", sid.0);
                            // Notify TUI that connection is back.
                            if let Err(e) = tx.try_send(TaggedEvent::Connected) {
                                tracing::warn!(
                                    "EventPump failed to send connected notification: {e}"
                                );
                            }
                            current_rx = Some(new_rx);
                        }
                        None => break 'outer,
                    }
                }

                let Some(ref mut r) = current_rx else {
                    continue;
                };

                tokio::select! {
                    biased;
                    () = cancel_for_task.cancelled() => break 'outer,

                    opt = r.recv() => {
                        match opt {
                            Some((_sid, envelope)) => {
                                let event = envelope.event;
                                // Detect subagent launch and spawn a dedicated subscriber.
                                if let Event::Tool(ToolEvent::Metadata { ref tool_id, ref metadata, .. }) = event {
                                    if let Some(subagent_sid) = metadata.get("subagent_session_id") {
                                        let parent_tool_id = metadata
                                            .get("parent_tool_id")
                                            .cloned()
                                            .unwrap_or_else(|| tool_id.clone());
                                        let sub_sid = SessionId::from(subagent_sid.clone());
                                        let coord_clone = kernel.clone();
                                        let tx_clone = tx.clone();
                                        let cancel_clone = cancel_for_task.clone();
                                        tokio::spawn(async move {
                                            match coord_clone.subscribe_session_events(&sub_sid, None).await {
                                                Ok(mut sub_rx) => {
                                                    loop {
                                                        tokio::select! {
                                                            biased;
                                                            () = cancel_clone.cancelled() => break,
                                                            opt = sub_rx.recv() => {
                                                                match opt {
                                                                    Some((_sid, envelope)) => {
                                                                        let ev = envelope.event;
                                                                        // Skip high-frequency delta events to avoid TUI spam.
                                                                        // Only forward structural events (tool start/end, lifecycle, usage).
                                                                        let is_delta = matches!(
                                                                            &ev,
                                                                            Event::Model(ModelEvent::Chunk { .. } | ModelEvent::ToolCallDelta { .. })
                                                                        );
                                                                        if is_delta {
                                                                            continue;
                                                                        }

                                                                        let is_stopped = matches!(
                                                                            &ev,
                                                                            Event::Agent(AgentEvent::Lifecycle {
                                                                                state: AgentStatus::Stopped { .. },
                                                                                ..
                                                                            })
                                                                        );
                                                                        if let Err(e) = tx_clone.try_send(TaggedEvent::Subagent {
                                                                            parent_tool_id: parent_tool_id.clone(),
                                                                            session_id: sub_sid.0.to_string(),
                                                                            event: ev,
                                                                        }) {
                                                                            tracing::warn!("Subagent event pump mpsc closed: {e}");
                                                                            break;
                                                                        }
                                                                        if is_stopped {
                                                                            break;
                                                                        }
                                                                    }
                                                                    None => {
                                                                        tracing::warn!("Subagent subscriber closed");
                                                                        break;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::warn!("Failed to subscribe to subagent session {}: {}", sub_sid.0, e);
                                                }
                                            }
                                        });
                                    }
                                }

                                if let Err(e) = tx.send(TaggedEvent::Main(event)).await {
                                    tracing::warn!("EventPump mpsc closed: {e}");
                                    break 'outer;
                                }
                            }
                            None => {
                                tracing::warn!("Subscriber closed, will resubscribe");
                                if let Err(e) = tx.try_send(TaggedEvent::ConnectionLost) {
                                    tracing::warn!("EventPump failed to send connection lost notification: {e}");
                                }
                                current_rx = None;
                            }
                        }
                    }
                }
            }
            tracing::info!("EventPump stopped for {}", sid.0);
        });

        (Self { cancel }, rx)
    }

    /// Retry subscribe until success or cancellation.  No deadline — the
    /// pump keeps trying forever so the TUI can recover from a daemon
    /// restart at any time.
    async fn resubscribe(
        kernel: &Arc<dyn KernelApi>,
        session_id: &SessionId,
        _auto_approve: Level,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<EventBusSubscriber> {
        let mut retries: u32 = 0;
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            match tokio::time::timeout(
                Duration::from_secs(5),
                kernel.subscribe_session_events(session_id, None),
            )
            .await
            {
                Ok(Ok(rx)) => return Some(rx),
                Ok(Err(e)) => {
                    if e.is_session_not_found() {
                        tracing::info!(
                            "Session {} missing on daemon, attempting restore…",
                            session_id.0
                        );
                        match kernel.restore_session(session_id).await {
                            Ok(_) => {
                                // Session restored — immediately retry subscribe.
                                continue;
                            }
                            Err(restore_err) => {
                                tracing::warn!(
                                    "Failed to restore session {}: {}, will retry",
                                    session_id.0,
                                    restore_err
                                );
                            }
                        }
                    } else {
                        tracing::debug!("Subscribe failed: {e}, will retry");
                    }
                }
                Err(_) => {
                    tracing::debug!(
                        "Subscribe timed out ({}ms), will retry",
                        crate::app::types::SUBSCRIBE_TIMEOUT_MS
                    );
                }
            }
            retries += 1;
            let delay_ms = std::cmp::min(100 * (1_u64 << retries.min(63)), 5000);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }
}
