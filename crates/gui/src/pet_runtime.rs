use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::error::GuiError;
use crate::pet::{PetConnectionStatus, PetSnapshot};
use crate::state::AppState;

const PET_RUNTIME_TICK: Duration = Duration::from_secs(1);

pub async fn run_pet_runtime(
    state: AppState,
    app_handle: AppHandle,
    notifications: tokio::sync::mpsc::Receiver<kernel::notification::Notification>,
) {
    let mut tick = tokio::time::interval(PET_RUNTIME_TICK);
    let mut last_snapshot = None;
    let mut notifications = Some(notifications);
    let mut reconnect_task = None;
    let mut recovery_task = None;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let mut runtime = state.pet_runtime.lock().await;
                runtime.expire(Instant::now());
            }
            () = state.pet_runtime_notify.notified() => {}
            notification = recv_notification(&mut notifications), if notifications.is_some() => {
                match notification {
                    Some(notification) => {
                        let connection_lost = matches!(
                            notification,
                            kernel::notification::Notification::ConnectionLost { .. }
                        );
                        let background_tasks_changed = matches!(
                            notification,
                            kernel::notification::Notification::BackgroundTasksChanged { .. }
                        );
                        let mut runtime = state.pet_runtime.lock().await;
                        apply_notification(&mut runtime, &notification, Instant::now());
                        drop(runtime);
                        if connection_lost && recovery_task.is_none() {
                            recovery_task = Some(spawn_connection_recovery(state.clone()));
                        } else if background_tasks_changed {
                            if let Err(error) = sync_running_sessions(&state).await {
                                tracing::debug!("Desktop pet background task sync failed: {error}");
                            }
                        }
                    }
                    None => {
                        notifications = None;
                        state
                            .pet_runtime
                            .lock()
                            .await
                            .set_connection_status(PetConnectionStatus::Disconnected);
                        reconnect_task = Some(spawn_notification_reconnect(state.clone()));
                    }
                }
            }
            result = await_reconnect(&mut reconnect_task), if reconnect_task.is_some() => {
                reconnect_task = None;
                match result {
                    Ok(receiver) => {
                        notifications = Some(receiver);
                        state
                            .pet_runtime
                            .lock()
                            .await
                            .set_connection_status(PetConnectionStatus::Connected);
                        if let Err(error) = sync_running_sessions(&state).await {
                            tracing::debug!("Desktop pet reconnect session sync failed: {error}");
                        }
                    }
                    Err(error) => {
                        tracing::warn!("Desktop pet notification reconnect task failed: {error}");
                        reconnect_task = Some(spawn_notification_reconnect(state.clone()));
                    }
                }
            }
            result = await_recovery(&mut recovery_task), if recovery_task.is_some() => {
                recovery_task = None;
                match result {
                    Ok(sessions) => {
                        reconcile_sessions(&state, &sessions).await;
                        state
                            .pet_runtime
                            .lock()
                            .await
                            .set_connection_status(PetConnectionStatus::Connected);
                    }
                    Err(error) => {
                        tracing::warn!("Desktop pet connection recovery task failed: {error}");
                        recovery_task = Some(spawn_connection_recovery(state.clone()));
                    }
                }
            }
        }

        emit_current_snapshot(&state, &app_handle, &mut last_snapshot).await;
    }
}

async fn recv_notification(
    notifications: &mut Option<tokio::sync::mpsc::Receiver<kernel::notification::Notification>>,
) -> Option<kernel::notification::Notification> {
    notifications.as_mut()?.recv().await
}

fn spawn_notification_reconnect(
    state: AppState,
) -> tokio::task::JoinHandle<tokio::sync::mpsc::Receiver<kernel::notification::Notification>> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(PET_RUNTIME_TICK).await;
            match state.kernel_snapshot().subscribe_notifications().await {
                Ok(receiver) => return receiver,
                Err(error) => {
                    tracing::debug!("Desktop pet notification retry failed: {error}");
                }
            }
        }
    })
}

async fn await_reconnect(
    task: &mut Option<
        tokio::task::JoinHandle<tokio::sync::mpsc::Receiver<kernel::notification::Notification>>,
    >,
) -> Result<tokio::sync::mpsc::Receiver<kernel::notification::Notification>, tokio::task::JoinError>
{
    task.as_mut().expect("guarded reconnect task").await
}

fn spawn_connection_recovery(
    state: AppState,
) -> tokio::task::JoinHandle<Vec<kernel::types::RunningSessionResponse>> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(PET_RUNTIME_TICK).await;
            match state.kernel_snapshot().list_running_sessions().await {
                Ok(sessions) => return sessions,
                Err(error) => {
                    tracing::debug!("Desktop pet connection recovery failed: {error}");
                }
            }
        }
    })
}

async fn await_recovery(
    task: &mut Option<tokio::task::JoinHandle<Vec<kernel::types::RunningSessionResponse>>>,
) -> Result<Vec<kernel::types::RunningSessionResponse>, tokio::task::JoinError> {
    task.as_mut().expect("guarded recovery task").await
}

async fn emit_current_snapshot(
    state: &AppState,
    app_handle: &AppHandle,
    last_snapshot: &mut Option<PetSnapshot>,
) {
    let snapshot = state.pet_runtime.lock().await.snapshot(Instant::now());
    if !state.is_pet_enabled() || last_snapshot.as_ref() == Some(&snapshot) {
        return;
    }

    emit_pet_snapshot(app_handle, snapshot.clone());
    *last_snapshot = Some(snapshot);
}

pub async fn start_pet_runtime(state: &AppState, app_handle: &AppHandle) -> Result<(), GuiError> {
    let mut task = state.pet_runtime_task.lock().await;
    if task.as_ref().is_some_and(|handle| !handle.is_finished()) {
        return Ok(());
    }
    if let Some(finished) = task.take() {
        if let Err(error) = finished.await {
            tracing::warn!("Previous desktop pet runtime exited unexpectedly: {error}");
        }
    }
    let notifications = state
        .kernel_snapshot()
        .subscribe_notifications()
        .await
        .map_err(GuiError::kernel)?;
    let state = state.clone();
    let app_handle = app_handle.clone();
    *task = Some(tokio::spawn(run_pet_runtime(
        state,
        app_handle,
        notifications,
    )));
    Ok(())
}

pub async fn sync_running_sessions(state: &AppState) -> Result<(), GuiError> {
    let sessions = state
        .kernel_snapshot()
        .list_running_sessions()
        .await
        .map_err(GuiError::kernel)?;
    reconcile_sessions(state, &sessions).await;
    Ok(())
}

async fn reconcile_sessions(state: &AppState, sessions: &[kernel::types::RunningSessionResponse]) {
    let mut runtime = state.pet_runtime.lock().await;
    runtime.reconcile_running_sessions(sessions.iter().map(|session| {
        (
            session.id.0.as_str(),
            session.title.as_deref(),
            session_is_running(session),
        )
    }));
    for session in sessions
        .iter()
        .filter(|session| !session_is_running(session))
    {
        runtime.clear_session_requests(session.id.0.as_str());
    }
    drop(runtime);
    state.pet_runtime_notify.notify_one();
}

fn session_is_running(session: &kernel::types::RunningSessionResponse) -> bool {
    session.background_task_count > 0
        || !matches!(session.phase.as_str(), "idle" | "stopped" | "closed")
}

fn apply_notification(
    runtime: &mut crate::pet::PetRuntime,
    notification: &kernel::notification::Notification,
    now: Instant,
) -> bool {
    use kernel::notification::Notification;

    if !matches!(notification, Notification::ConnectionLost { .. }) {
        runtime.record_activity(now);
    }
    match notification {
        Notification::StateChanged { session_id, status } => {
            let running = !matches!(status, kernel::agent::AgentState::Idle);
            let changed = runtime.update_session_running(session_id.0.as_str(), running);
            if running {
                changed
            } else {
                let requests_changed = runtime.clear_session_requests(session_id.0.as_str());
                changed || requests_changed
            }
        }
        Notification::TitleUpdated { session_id, title } => {
            runtime.update_session_title(session_id.0.as_str(), Some(title))
        }
        Notification::ConnectionLost { .. } => {
            runtime.set_connection_status(PetConnectionStatus::Disconnected)
        }
        Notification::BackgroundTasksChanged { .. } => {
            // The caller refreshes the authoritative running-session snapshot.
            false
        }
        Notification::AgentActivity {
            session_id,
            event_id,
            activity,
        } => runtime.process_activity(session_id.0.as_str(), event_id, activity, now),
    }
}

pub fn emit_pet_snapshot(app_handle: &AppHandle, snapshot: PetSnapshot) {
    let _ = app_handle.emit_to("pet", "pet:state", snapshot);
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{apply_notification, session_is_running};
    use crate::pet::{PetConnectionStatus, PetMood, PetRuntime};
    use kernel::event::StopReason;
    use kernel::notification::{AgentActivity, Notification};
    use kernel::types::SessionId;

    fn notification(
        session_id: &SessionId,
        event_id: &str,
        activity: AgentActivity,
    ) -> Notification {
        Notification::AgentActivity {
            session_id: session_id.clone(),
            event_id: event_id.into(),
            activity,
        }
    }

    #[test]
    fn background_tasks_keep_session_running() {
        let session = kernel::types::RunningSessionResponse {
            id: SessionId::from("session-1".to_string()),
            parent_id: None,
            title: Some("Background task".into()),
            project_id: None,
            phase: "idle".into(),
            background_task_count: 1,
            background_shells: vec![],
        };
        assert!(session_is_running(&session));
    }

    #[test]
    fn global_agent_notifications_update_pet_requests_and_notices() {
        let now = Instant::now();
        let mut runtime = PetRuntime::new(now);
        let session_id = SessionId::from("session-1".to_string());

        assert!(apply_notification(
            &mut runtime,
            &notification(
                &session_id,
                "event-1",
                AgentActivity::PermissionRequested {
                    req_id: "req-1".into(),
                    target_session_id: "session-1".into(),
                },
            ),
            now,
        ));
        assert_eq!(runtime.snapshot(now).mood, PetMood::Alert);

        assert!(apply_notification(
            &mut runtime,
            &notification(
                &session_id,
                "event-2",
                AgentActivity::RequestResolved {
                    req_id: "req-1".into(),
                },
            ),
            now,
        ));
        assert_eq!(runtime.snapshot(now).mood, PetMood::Idle);

        assert!(apply_notification(
            &mut runtime,
            &notification(&session_id, "event-3", AgentActivity::Started),
            now,
        ));
        assert_eq!(runtime.snapshot(now).running_count, 1);

        assert!(apply_notification(
            &mut runtime,
            &notification(
                &session_id,
                "event-4",
                AgentActivity::Stopped {
                    reason: StopReason::Completed {
                        finish_reason: None,
                    },
                },
            ),
            now,
        ));
        let snapshot = runtime.snapshot(now);
        assert_eq!(snapshot.running_count, 0);
        assert_eq!(snapshot.mood, PetMood::Happy);
        assert!(snapshot.notice.is_some());
    }

    #[test]
    fn idle_state_clears_stale_session_requests() {
        let now = Instant::now();
        let mut runtime = PetRuntime::new(now);
        let session_id = SessionId::from("session-1".to_string());
        apply_notification(
            &mut runtime,
            &notification(
                &session_id,
                "event-1",
                AgentActivity::AskUserRequested {
                    req_id: "req-1".into(),
                    target_session_id: "session-1".into(),
                },
            ),
            now,
        );

        apply_notification(
            &mut runtime,
            &Notification::StateChanged {
                session_id,
                status: kernel::agent::AgentState::Idle,
            },
            now,
        );

        assert!(runtime.snapshot(now).request.is_none());
    }

    #[test]
    fn activity_recovers_connection_status() {
        let now = Instant::now();
        let mut runtime = PetRuntime::new(now);
        let session_id = SessionId::from("session-1".to_string());

        runtime.set_connection_status(PetConnectionStatus::Disconnected);
        apply_notification(
            &mut runtime,
            &Notification::TitleUpdated {
                session_id,
                title: "Recovered".into(),
            },
            now,
        );

        assert_eq!(
            runtime.snapshot(now).connection_status,
            PetConnectionStatus::Connected
        );
    }
}
