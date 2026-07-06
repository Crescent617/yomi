use super::level::{exceeds_threshold, Level};
use crate::agent::AgentInput;
use crate::comms::EventBusHandle;
use crate::event::{AgentEvent, Event};
use crate::tools::{EDIT_TOOL_NAME, READ_TOOL_NAME, SHELL_TOOL_NAME};
use crate::types::{KernelError, Result, SessionId, ToolCall};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Result of checking permissions for a batch of tool calls
pub struct PermissionCheckResult {
    /// Tool calls that are approved for execution
    pub approved: Vec<ToolCall>,
    /// Tool calls that were denied, with (`tool_call_id`, `error_message`)
    pub denied: Vec<(String, String)>,
}

/// Check permissions for a batch of tool calls.
///
/// This function partitions tool calls into approved and denied lists.
/// If no permission checker is provided (YOLO mode), all calls are approved.
///
/// # Arguments
/// * `tool_calls` - The tool calls to check
/// * `permission_checker` - Optional permission checker (None = YOLO mode)
/// * `agent_id` - For logging purposes
///
/// # Returns
/// A `PermissionCheckResult` containing approved calls and denied call error messages.
pub async fn check_tool_permissions(
    tool_calls: &[ToolCall],
    permission_checker: Option<&Checker>,
) -> PermissionCheckResult {
    use super::resolver::ToolLevelResolver;

    let mut approved = Vec::new();
    let mut denied = Vec::new();

    for call in tool_calls {
        let level = ToolLevelResolver::resolve(&call.name, &call.arguments);

        // Check if permission is needed
        if let Some(checker) = permission_checker {
            match checker.check_permission(call, level).await {
                Ok(true) => {
                    approved.push(call.clone());
                }
                Ok(false) => {
                    tracing::warn!(
                        "Tool call {} denied: {} exceeds threshold",
                        call.id,
                        call.name
                    );
                    let error_msg = format!(
                        "Permission denied: {} tool (level: {:?}) was not approved by user",
                        call.name, level
                    );
                    denied.push((call.id.clone(), error_msg));
                }
                Err(e) => {
                    tracing::error!("Permission check failed for {}: {}", call.name, e);
                    let error_msg = format!("Permission check failed: {e}");
                    denied.push((call.id.clone(), error_msg));
                }
            }
        } else {
            // No permission checker (YOLO mode), approve all
            approved.push(call.clone());
        }
    }

    PermissionCheckResult { approved, denied }
}

/// Response from user for a permission request
#[derive(Debug, Clone, Copy)]
pub struct Response {
    /// Whether the tool execution is approved
    pub approved: bool,
    /// If true, remember this choice and auto-approve this level for the session
    pub remember: bool,
}

impl Response {
    /// Create a simple approve/deny response without remembering
    pub const fn once(approved: bool) -> Self {
        Self {
            approved,
            remember: false,
        }
    }

    /// Create an approved response with remember flag
    pub const fn approve(remember: bool) -> Self {
        Self {
            approved: true,
            remember,
        }
    }

    /// Create a denied response
    pub const fn deny() -> Self {
        Self {
            approved: false,
            remember: false,
        }
    }
}

/// Shared permission state across agents in a session
///
/// This is shared between all agents (main agent and subagents) so that:
/// - "Remember this approval" works across all agents
/// - Auto-approve level is consistent across the session
#[derive(Clone)]
pub struct PermissionState {
    // 使用 RwLock 允许运行时动态更新（用户选择 "always approve"）
    auto_approve_level: Arc<tokio::sync::RwLock<Level>>,
    /// Per-tool approval levels - tools that have been remembered as approved
    /// Key: tool name, Value: max approved level for this tool
    tool_approvals: Arc<tokio::sync::RwLock<HashMap<String, Level>>>,
}

impl PermissionState {
    /// Create new shared permission state
    pub fn new(auto_approve_level: Level) -> Self {
        Self {
            auto_approve_level: Arc::new(tokio::sync::RwLock::new(auto_approve_level)),
            tool_approvals: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Update the auto-approve level at runtime
    pub async fn set_auto_approve_level(&self, level: Level) {
        let mut current = self.auto_approve_level.write().await;
        *current = level;
    }

    /// Get the current auto-approve level
    pub async fn get_auto_approve_level(&self) -> Level {
        *self.auto_approve_level.read().await
    }
}

/// 权限检查器
///
/// 职责：
/// 1. 检查工具级别是否超过阈值
/// 2. 如果超过阈值，发送 `PermissionRequest` 事件并等待响应
/// 3. 上层（TUI/Session）负责显示确认对话框或自动响应
pub struct Checker {
    state: PermissionState,
    event_tx: EventBusHandle,
    input_bus: Arc<crate::comms::InputBus>,
    session_id: SessionId,
}

impl Checker {
    /// 创建新的权限检查器
    pub fn new(
        state: PermissionState,
        event_tx: EventBusHandle,
        input_bus: Arc<crate::comms::InputBus>,
        session_id: SessionId,
    ) -> Self {
        Self {
            state,
            event_tx,
            input_bus,
            session_id,
        }
    }

    /// 获取当前自动批准级别
    pub async fn auto_approve_level(&self) -> Level {
        *self.state.auto_approve_level.read().await
    }

    /// 检查工具是否需要权限确认
    ///
    /// 返回：
    /// - Ok(true): 允许执行（未超过阈值或用户批准）
    /// - Ok(false): 拒绝执行（用户拒绝或超时）
    /// - Err: 检查过程中发生错误
    pub async fn check_permission(&self, tool_call: &ToolCall, level: Level) -> Result<bool> {
        // 检查是否超过全局阈值
        let current_level = *self.state.auto_approve_level.read().await;
        if !exceeds_threshold(level, current_level) {
            return Ok(true);
        }

        // 检查该工具是否已被记住批准（per-tool approval）
        let tool_approvals = self.state.tool_approvals.read().await;
        if let Some(&approved_level) = tool_approvals.get(&tool_call.name) {
            if level <= approved_level {
                tracing::info!(
                    "Tool {} auto-approved (remembered approval up to {})",
                    tool_call.name,
                    approved_level
                );
                return Ok(true);
            }
        }
        drop(tool_approvals);

        // 超过阈值，需要用户确认
        let req_id = ulid::Ulid::new().to_string();
        let mut subscriber = self.input_bus.subscribe(self.session_id.clone());

        // 提取工具参数用于显示
        let tool_args = match tool_call.name.as_str() {
            SHELL_TOOL_NAME => tool_call
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            EDIT_TOOL_NAME | READ_TOOL_NAME => tool_call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            _ => serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
        };

        // 发送权限请求事件
        self.event_tx
            .send(crate::event::Envelope::new(
                self.session_id.clone(),
                Event::Agent(AgentEvent::PermissionRequest {
                    req_id: req_id.clone(),
                    session_id: self.session_id.0.to_string(),
                    tool_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    tool_args,
                    tool_level: format!("{level}"),
                    reason: format!(
                        "{} tool exceeds {:?} auto-approve threshold",
                        tool_call.name, current_level
                    ),
                }),
            ))
            .await
            .map_err(|e| KernelError::io(format!("Failed to send permission request: {e}")))?;
        tracing::info!(
            "Permission request sent with req_id={req_id} for tool {}",
            tool_call.name
        );

        // 等待响应（2 分钟 timeout）
        let result = tokio::time::timeout(Duration::from_mins(2), async {
            while let Some((_, input)) = subscriber.recv().await {
                if let AgentInput::PermissionResponse {
                    req_id: id,
                    approved,
                    remember,
                } = input
                {
                    if id == req_id {
                        return Response { approved, remember };
                    }
                }
            }
            Response::deny()
        })
        .await;

        let approved = match result {
            Ok(response) => {
                tracing::info!(
                    "Permission check received response: approved={}, remember={}",
                    response.approved,
                    response.remember
                );

                // 如果用户选择 "remember"，记录该工具的批准级别
                if response.approved && response.remember {
                    let mut approvals = self.state.tool_approvals.write().await;
                    let tool_name = tool_call.name.clone();
                    match approvals.get(&tool_name) {
                        Some(&current) if current >= level => {}
                        _ => {
                            tracing::info!(
                                "Remembering approval for tool '{}' up to {:?} level",
                                tool_name,
                                level
                            );
                            approvals.insert(tool_name, level);
                        }
                    }
                }

                response.approved
            }
            Err(_) => {
                tracing::warn!("Permission request timeout for tool {}", tool_call.name);
                false
            }
        };

        let _ = self
            .event_tx
            .send(crate::event::Envelope::new(
                self.session_id.clone(),
                Event::Agent(AgentEvent::PermissionAck {
                    req_id: req_id.clone(),
                }),
            ))
            .await;

        Ok(approved)
    }
}

#[cfg(test)]
#[path = "checker_test.rs"]
mod tests;
