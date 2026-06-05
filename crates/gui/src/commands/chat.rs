use kernel::event::{Event, SystemEvent, UserEvent};
use kernel::permissions::Level;
use kernel::types::{ContentBlock, SessionId};
use tauri::{AppHandle, Emitter, State};

use crate::error::GuiError;
use crate::state::AppState;

/// GUI-layer camelCase wrappers for Message / `ContentBlock`

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInfo {
    pub id: String,
    pub role: String,
    pub content: Vec<ContentBlockInfo>,
    pub tool_calls: Option<Vec<kernel::types::ToolCall>>,
    pub tool_call_id: Option<String>,
    pub created_at: String,
    pub token_usage: Option<TokenUsageInfo>,
    pub response_id: Option<String>,
    pub finish_reason: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContentBlockInfo {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    RedactedThinking { data: String },
    ImageUrl { image_url: ImageUrlInfo },
    Audio { audio: AudioDataInfo },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUrlInfo {
    pub url: String,
    pub detail: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDataInfo {
    pub data: String,
    pub format: String,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ContentBlockInput {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    RedactedThinking { data: String },
    ImageUrl { image_url: ImageUrlInput },
    Audio { audio: AudioDataInput },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUrlInput {
    pub url: String,
    pub detail: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDataInput {
    pub data: String,
    pub format: String,
}

fn content_block_info(cb: &ContentBlock) -> ContentBlockInfo {
    match cb {
        ContentBlock::Text { text } => ContentBlockInfo::Text { text: text.clone() },
        ContentBlock::Thinking { thinking, signature } => ContentBlockInfo::Thinking {
            thinking: thinking.clone(),
            signature: signature.clone(),
        },
        ContentBlock::RedactedThinking { data } => {
            ContentBlockInfo::RedactedThinking { data: data.clone() }
        }
        ContentBlock::ImageUrl { image_url } => ContentBlockInfo::ImageUrl {
            image_url: ImageUrlInfo {
                url: image_url.url.clone(),
                detail: image_url.detail.clone(),
            },
        },
        ContentBlock::Audio { audio } => ContentBlockInfo::Audio {
            audio: AudioDataInfo {
                data: audio.data.clone(),
                format: audio.format.clone(),
            },
        },
    }
}

fn content_block_input(cb: ContentBlockInput) -> ContentBlock {
    match cb {
        ContentBlockInput::Text { text } => ContentBlock::Text { text },
        ContentBlockInput::Thinking { thinking, signature } => {
            ContentBlock::Thinking { thinking, signature }
        }
        ContentBlockInput::RedactedThinking { data } => ContentBlock::RedactedThinking { data },
        ContentBlockInput::ImageUrl { image_url } => ContentBlock::ImageUrl {
            image_url: kernel::types::ImageUrl {
                url: image_url.url,
                detail: image_url.detail,
            },
        },
        ContentBlockInput::Audio { audio } => ContentBlock::Audio {
            audio: kernel::types::AudioData {
                data: audio.data,
                format: audio.format,
            },
        },
    }
}

fn message_info(msg: &kernel::types::Message) -> MessageInfo {
    MessageInfo {
        id: msg.id.as_str().to_string(),
        role: match msg.role {
            kernel::types::Role::System => "system".to_string(),
            kernel::types::Role::User => "user".to_string(),
            kernel::types::Role::Assistant => "assistant".to_string(),
            kernel::types::Role::Tool => "tool".to_string(),
        },
        content: msg.content.iter().map(content_block_info).collect(),
        tool_calls: msg.tool_calls.clone(),
        tool_call_id: msg.tool_call_id.clone(),
        created_at: msg.created_at.to_rfc3339(),
        token_usage: msg.token_usage.as_ref().map(|tu| TokenUsageInfo {
            prompt_tokens: tu.prompt_tokens,
            completion_tokens: tu.completion_tokens,
            total_tokens: tu.total_tokens,
        }),
        response_id: msg.response_id.clone(),
        finish_reason: msg.finish_reason.map(|fr| match fr {
            kernel::types::FinishReason::Stop => "stop".to_string(),
            kernel::types::FinishReason::MaxTokens => "maxTokens".to_string(),
            kernel::types::FinishReason::ContentFilter => "contentFilter".to_string(),
            kernel::types::FinishReason::ToolCalls => "toolCalls".to_string(),
        }),
    }
}

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let block = ContentBlock::Text { text: content };
    coord
        .send_message(&sid, vec![block])
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn send_message_blocks(
    state: State<'_, AppState>,
    session_id: String,
    blocks: Vec<ContentBlockInput>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let blocks: Vec<ContentBlock> = blocks.into_iter().map(content_block_input).collect();
    coord
        .send_message(&sid, blocks)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn subscribe(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id.clone());
    let mut rx = match coord.subscribe_session_events(&sid).await {
        Ok(rx) => rx,
        Err(_) => {
            coord
                .restore_session(&sid)
                .await
                .map_err(GuiError::kernel)?;
            coord
                .subscribe_session_events(&sid)
                .await
                .map_err(GuiError::kernel)?
        }
    };

    // Stop any existing event task BEFORE spawning the new one to avoid
    // stale tasks racing with the insert.
    state.stop_event_task(&session_id).await;

    let sid_cleanup = session_id.clone();
    let tasks_cleanup = state.event_tasks.clone();
    let handle = tauri::async_runtime::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let mut event_value = serde_json::to_value(&event).unwrap_or_default();

            // SystemEvent::Rewound contains raw Message objects which are still snake_case.
            // Convert them to MessageInfo so the frontend receives camelCase.
            if let Event::System(SystemEvent::Rewound { ref messages, .. }) = event {
                if let Some(msgs) = event_value.pointer_mut("/system/rewound/messages").and_then(|v| v.as_array_mut()) {
                    let converted: Vec<serde_json::Value> = messages
                        .iter()
                        .map(|m| serde_json::to_value(message_info(m)).unwrap_or_default())
                        .collect();
                    *msgs = converted;
                }
            }

            // UserEvent::Message contains raw ContentBlock objects which are still snake_case.
            // Convert them to ContentBlockInfo so the frontend receives camelCase.
            if let Event::User(UserEvent::Message { ref content, .. }) = event {
                if let Some(blocks) = event_value.pointer_mut("/user/message/content").and_then(|v| v.as_array_mut()) {
                    let converted: Vec<serde_json::Value> = content
                        .iter()
                        .map(|b| serde_json::to_value(content_block_info(b)).unwrap_or_default())
                        .collect();
                    *blocks = converted;
                }
            }

            let payload = serde_json::json!({
                "sessionId": sid,
                "event": event_value,
            });
            let _ = app_handle.emit("kernel:event", payload);
        }
        // Remove our handle from the map when the channel closes so we don't leak.
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

#[tauri::command]
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

#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let messages = coord
        .get_session_messages(&sid)
        .await
        .map_err(GuiError::kernel)?;
    let values: Vec<_> = messages
        .into_iter()
        .map(|m| serde_json::to_value(message_info(&m)).unwrap_or_default())
        .collect();
    Ok(values)
}

#[tauri::command]
pub async fn get_session_status(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let status = coord
        .get_session_status(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(serde_json::to_value(status).unwrap_or_default())
}

#[tauri::command]
pub async fn get_todos(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    match coord.get_todos(&sid).await.map_err(GuiError::kernel)? {
        Some(json_str) => {
            let parsed: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| GuiError::unknown(format!("Invalid todo JSON: {e}")))?;
            Ok(parsed)
        }
        None => Ok(serde_json::json!({ "todos": [] })),
    }
}

// ── Cancel ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn cancel_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord.cancel(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

// ── Permission response ────────────────────────────────────────────────────

#[tauri::command]
pub async fn respond_permission(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    approved: bool,
    remember: bool,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .send_permission_response(&sid, &req_id, approved, remember)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Ask user response ────────────────────────────────────────────────────

#[tauri::command]
pub async fn respond_ask_user(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    answers: Vec<(String, String)>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let response = kernel::tools::AskUserResponse {
        answers: answers.into_iter().collect(),
    };
    coord
        .send_ask_user_response(&sid, &req_id, response)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Compact session ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn compact_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .compact_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Set permission level ─────────────────────────────────────────────────

#[tauri::command]
pub async fn set_permission_level(
    state: State<'_, AppState>,
    session_id: String,
    level: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let level = parse_level(&level)?;
    coord
        .set_permission_level(&sid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Goal mode ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_goal(
    state: State<'_, AppState>,
    session_id: String,
    description: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let goal_state = kernel::goal::GoalState::new(description);
    coord
        .start_goal(&sid, goal_state)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn stop_goal(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord.stop_goal(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    state: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .rename_session(&sid, title)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Steer message ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn send_steer(
    state: State<'_, AppState>,
    session_id: String,
    blocks: Vec<ContentBlockInput>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let blocks: Vec<ContentBlock> = blocks.into_iter().map(content_block_input).collect();
    coord
        .send_steer(&sid, blocks)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
