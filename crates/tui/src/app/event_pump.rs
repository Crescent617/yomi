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
    BackgroundTasksChanged {
        session_id: String,
        kind: kernel::agent::BackgroundTaskKind,
        count: usize,
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

/// Try to send a droppable (informational) event without backpressure.
///
/// If the channel is full the event is dropped with a warning; this must only
/// be used for events the UI can afford to lose (e.g. subagent progress).
/// Returns `false` if the channel is closed and the caller should stop.
fn try_send_droppable(tx: &mpsc::Sender<TaggedEvent>, event: TaggedEvent, what: &str) -> bool {
    match tx.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!("EventPump channel full, dropping {what}");
            true
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::warn!("EventPump channel closed, cannot send {what}");
            false
        }
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
        let (tx, rx) = mpsc::channel(4096);

        tokio::spawn(async move {
            let sid = SessionId::from(session_id);
            let mut current_rx = Some(initial_rx);
            let mut notification_rx = match kernel.subscribe_notifications().await {
                Ok(rx) => Some(rx),
                Err(e) => {
                    tracing::warn!("Failed to subscribe to background task notifications: {e}");
                    None
                }
            };

            // Notify TUI that the initial connection is ready (only in daemon mode).
            // Connection-state transitions must not be dropped, so use the
            // backpressured `send` rather than `try_send`.
            if crate::daemon_mode() {
                if let Err(e) = tx.send(TaggedEvent::Connected).await {
                    tracing::warn!("EventPump failed to send initial connected notification: {e}");
                    return;
                }
            }

            'outer: loop {
                // When subscriber is closed, resubscribe (infinite retry).
                if current_rx.is_none() {
                    match Self::resubscribe(&kernel, &sid, _auto_approve, &cancel_for_task).await {
                        Some(new_rx) => {
                            tracing::info!("EventPump re-subscribed to {}", sid.0);
                            // Notify TUI that connection is back (must not be dropped).
                            if let Err(e) = tx.send(TaggedEvent::Connected).await {
                                tracing::warn!(
                                    "EventPump failed to send connected notification: {e}"
                                );
                                break 'outer;
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

                    noti = async {
                        match notification_rx.as_mut() {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        match noti {
                            Some(kernel::notification::Notification::BackgroundTasksChanged {
                                session_id,
                                kind,
                                count,
                            }) if session_id == sid => {
                                if tx.send(TaggedEvent::BackgroundTasksChanged {
                                    session_id: session_id.to_string(),
                                    kind,
                                    count,
                                }).await.is_err() {
                                    break 'outer;
                                }
                            }
                            Some(_) => {}
                            None => notification_rx = None,
                        }
                    }

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
                                                                        // Subagent events are informational; safe to drop when full.
                                                                        if !try_send_droppable(&tx_clone, TaggedEvent::Subagent {
                                                                            parent_tool_id: parent_tool_id.clone(),
                                                                            session_id: sub_sid.0.to_string(),
                                                                            event: ev,
                                                                        }, "subagent event") {
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

                                // Main session events must never be silently dropped:
                                // losing e.g. ModelEvent::End or a tool result corrupts
                                // the visible transcript. Use backpressured send.
                                if let Err(e) = tx.send(TaggedEvent::Main(event)).await {
                                    tracing::warn!("EventPump mpsc closed: {e}");
                                    break 'outer;
                                }
                            }
                            None => {
                                tracing::warn!("Subscriber closed, will resubscribe");
                                // Connection-state transitions must not be dropped.
                                if let Err(e) = tx.send(TaggedEvent::ConnectionLost).await {
                                    tracing::warn!("EventPump failed to send connection lost notification: {e}");
                                    break 'outer;
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
            let delay_ms = std::cmp::min(100 * (1_u64 << retries.min(6)), 5000);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }
}
