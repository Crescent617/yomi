use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

use kernel::client::CoordinatorApi;
use kernel::event::{Event, SystemEvent};
use kernel::permissions::Level;
use kernel::types::SessionId;

/// Transparent event pump that hides connection churn from the TUI.
///
/// Holds the kernel-side `broadcast::Receiver` and forwards events into
/// a stable `mpsc::Receiver`.  When the broadcast channel is closed
/// (daemon restart / network drop) the pump automatically re-subscribes
/// and restores the session — the TUI sees a continuous stream of events
/// plus explicit `Connected` / `ConnectionLost` notifications when the
/// connection state changes.
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
        initial_rx: broadcast::Receiver<Event>,
        coordinator: Arc<dyn CoordinatorApi>,
        session_id: String,
        auto_approve: Level,
    ) -> (Self, mpsc::Receiver<Event>) {
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let (tx, rx) = mpsc::channel(256);

        tokio::spawn(async move {
            let sid = SessionId(session_id);
            let mut current_rx = Some(initial_rx);

            // Notify TUI that the initial connection is ready.
            if let Err(e) = tx.try_send(Event::System(SystemEvent::Connected {
                session_id: sid.clone(),
            })) {
                tracing::warn!("EventPump failed to send initial connected notification: {e}");
            }

            'outer: loop {
                // When broadcast is closed, resubscribe (infinite retry).
                if current_rx.is_none() {
                    match Self::resubscribe(&coordinator, &sid, auto_approve, &cancel_for_task)
                        .await
                    {
                        Some(new_rx) => {
                            tracing::info!("EventPump re-subscribed to {}", sid.0);
                            // Notify TUI that connection is back.
                            if let Err(e) = tx.try_send(Event::System(SystemEvent::Connected {
                                session_id: sid.clone(),
                            })) {
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

                    result = r.recv() => {
                        match result {
                            Ok(ev) => {
                                if let Err(e) = tx.try_send(ev) {
                                    match e {
                                        mpsc::error::TrySendError::Full(_) => {
                                            tracing::warn!("EventPump mpsc full");
                                        }
                                        mpsc::error::TrySendError::Closed(_) => break 'outer,
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                tracing::warn!("Broadcast closed, will resubscribe");
                                current_rx = None;
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("EventPump lagged by {n} events");
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
        coordinator: &Arc<dyn CoordinatorApi>,
        session_id: &SessionId,
        auto_approve: Level,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Option<broadcast::Receiver<Event>> {
        let mut retries: u32 = 0;
        loop {
            if cancel.is_cancelled() {
                return None;
            }
            match tokio::time::timeout(
                Duration::from_secs(5),
                coordinator.subscribe_session_events(session_id, auto_approve),
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
                        match coordinator.restore_session(session_id, auto_approve).await {
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
                    tracing::debug!("Subscribe timed out (5s), will retry");
                }
            }
            retries += 1;
            let delay_ms = std::cmp::min(100 * (1_u64 << retries), 5000);
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }
}
