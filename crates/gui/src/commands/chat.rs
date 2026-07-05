use kernel::event::{Event, SystemEvent, UserEvent};
use kernel::goal::GoalState;
use kernel::permissions::Level;
use kernel::tools::AskUserResponse;
use kernel::types::{ContentBlock, SessionId};
use tauri::{AppHandle, Emitter, State};

use crate::error::GuiError;
use crate::state::AppState;

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let block = ContentBlock::Text { text: content };
    coord
        .send_message(&sid, vec![block])
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_message_blocks(
    state: State<'_, AppState>,
    session_id: String,
    blocks: Vec<ContentBlock>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .send_message(&sid, blocks)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn subscribe(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id.clone());
    let mut rx = match coord.subscribe_session_events(&sid).await {
        Ok(rx) => rx,
        Err(_) => {
            coord
                .restore_session(&sid, Vec::new())
                .await
                .map_err(GuiError::kernel)?;
            coord
                .subscribe_session_events(&sid)
                .await
                .map_err(GuiError::kernel)?
        }
    };

    state.stop_event_task(&session_id).await;

    let sid_cleanup = session_id.clone();
    let tasks_cleanup = state.event_tasks.clone();
    let handle = tauri::async_runtime::spawn(async move {
        while let Some((_sid, event)) = rx.recv().await {
            let mut event_value = serde_json::to_value(&event).unwrap_or_default();

            if let Event::System(SystemEvent::Rewound { ref messages, .. }) = event {
                if let Some(msgs) = event_value
                    .pointer_mut("/system/rewound/messages")
                    .and_then(|v| v.as_array_mut())
                {
                    let converted: Vec<serde_json::Value> = messages
                        .iter()
                        .map(|m| serde_json::to_value(m).unwrap_or_default())
                        .collect();
                    *msgs = converted;
                }
            }

            if let Event::User(UserEvent::Message { ref content, .. }) = event {
                if let Some(blocks) = event_value
                    .pointer_mut("/user/message/content")
                    .and_then(|v| v.as_array_mut())
                {
                    let converted: Vec<serde_json::Value> = content
                        .iter()
                        .map(|b| serde_json::to_value(b).unwrap_or_default())
                        .collect();
                    *blocks = converted;
                }
            }

            let payload = serde_json::json!({
                "session_id": sid,
                "event": event_value,
            });
            let _ = app_handle.emit("kernel:event", payload);
        }
        let mut map = tasks_cleanup.lock().await;
        map.remove(&sid_cleanup);
    });

    {
        let mut tasks = state.event_tasks.lock().await;
        tasks.insert(session_id.clone(), handle);
    }

    {
        let mut guard = state.active_session.lock().await;
        *guard = Some(session_id);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn unsubscribe(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    state.stop_event_task(&session_id).await;
    {
        let mut guard = state.active_session.lock().await;
        if guard.as_ref() == Some(&session_id) {
            *guard = None;
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let messages = coord
        .get_session_messages(&sid)
        .await
        .map_err(GuiError::kernel)?;
    let values: Vec<_> = messages
        .into_iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let status = coord.get_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(serde_json::to_value(status)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_todos(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    match coord.get_todos(&sid).await.map_err(GuiError::kernel)? {
        Some(json_str) => {
            let parsed: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| GuiError::unknown(format!("Invalid todo JSON: {e}")))?;
            Ok(parsed)
        }
        None => Ok(serde_json::json!({ "todos": [] })),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord.cancel(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn respond_permission(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    approved: bool,
    remember: bool,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .send_permission_response(&sid, &req_id, approved, remember)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn respond_ask_user(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    answers: Vec<(String, String)>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let response = AskUserResponse {
        answers: answers.into_iter().collect(),
    };
    coord
        .send_ask_user_response(&sid, &req_id, response)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn compact_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .compact_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_permission_level(
    state: State<'_, AppState>,
    session_id: String,
    level: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let level = parse_level(&level)?;
    coord
        .set_permission_level(&sid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_goal(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Option<GoalState>, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let goal = coord.get_goal(&sid).await.map_err(GuiError::kernel)?;
    Ok(goal)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn start_goal(
    state: State<'_, AppState>,
    session_id: String,
    description: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    let goal_state = GoalState::new(description);
    coord
        .start_goal(&sid, goal_state)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pause_goal(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord.pause_goal(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn resume_goal(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord.resume_goal(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn edit_goal(
    state: State<'_, AppState>,
    session_id: String,
    description: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .update_goal(&sid, description)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn stop_goal(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord.stop_goal(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .rename_session(&sid, title)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_steer(
    state: State<'_, AppState>,
    session_id: String,
    blocks: Vec<ContentBlock>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord
        .send_steer(&sid, blocks)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn continue_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId::from(session_id);
    coord.send_continue(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}
