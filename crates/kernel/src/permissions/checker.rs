use super::level::{exceeds_threshold, Level};
use crate::event::{AgentEvent, Event};
use crate::tools::{BASH_TOOL_NAME, EDIT_TOOL_NAME, READ_TOOL_NAME};
use crate::types::{AgentId, ToolCall};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

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
/// - Permission responses can be routed to any agent
/// - "Remember this approval" works across all agents
/// - Auto-approve level is consistent across the session
#[derive(Clone)]
pub struct PermissionState {
    // 使用 RwLock 允许运行时动态更新（用户选择 "always approve"）
    auto_approve_level: Arc<tokio::sync::RwLock<Level>>,
    /// Per-tool approval levels - tools that have been remembered as approved
    /// Key: tool name, Value: max approved level for this tool
    tool_approvals: Arc<tokio::sync::RwLock<HashMap<String, Level>>>,
    pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
}

impl PermissionState {
    /// Create new shared permission state
    pub fn new(auto_approve_level: Level) -> (Self, Responder) {
        let pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let responder = Responder {
            pending_reqs: Arc::clone(&pending_permissions),
        };
        let state = Self {
            auto_approve_level: Arc::new(tokio::sync::RwLock::new(auto_approve_level)),
            tool_approvals: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            pending_permissions,
        };
        (state, responder)
    }

    /// Create a responder for this permission state
    pub fn create_responder(&self) -> Responder {
        Responder {
            pending_reqs: Arc::clone(&self.pending_permissions),
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
///
/// 对于 `Subagent` 场景，上层可以直接发送 PermissionResponse(false) 来拒绝
/// 不需要在 kernel 层区分 `SubAgent` 和主 Agent
pub struct Checker {
    state: PermissionState,
    agent_id: AgentId,
    event_tx: mpsc::Sender<Event>,
}

impl Checker {
    /// 创建新的权限检查器
    ///
    /// Uses shared `PermissionState` so all agents in a session share
    /// the same permission configuration and pending requests.
    pub fn new(state: PermissionState, agent_id: AgentId, event_tx: mpsc::Sender<Event>) -> Self {
        Self {
            state,
            agent_id,
            event_tx,
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
        let req_id = Uuid::now_v7().to_string();
        let (tx, rx) = oneshot::channel::<Response>();

        // 存储 oneshot sender 到 pending_permissions map
        {
            let mut pending = self.state.pending_permissions.lock().await;
            pending.insert(req_id.clone(), tx);
        }

        // 提取工具参数用于显示
        let tool_args = match tool_call.name.as_str() {
            BASH_TOOL_NAME => tool_call
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
            .send(Event::Agent(AgentEvent::PermissionRequest {
                agent_id: self.agent_id.clone(),
                req_id: req_id.clone(),
                tool_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                tool_args,
                tool_level: format!("{level}"),
                reason: format!(
                    "{} tool exceeds {:?} auto-approve threshold",
                    tool_call.name, current_level
                ),
            }))
            .await?;
        tracing::info!(
            "Permission request sent with req_id={req_id} for tool {}",
            tool_call.name
        );

        // 等待响应（使用 timeout 防止无限等待）
        match tokio::time::timeout(std::time::Duration::from_secs(300), rx).await {
            Ok(Ok(response)) => {
                tracing::info!(
                    "Permission check received response: approved={}, remember={}",
                    response.approved,
                    response.remember
                );

                // 如果用户选择 "remember"，记录该工具的批准级别（per-tool approval）
                if response.approved && response.remember {
                    let mut approvals = self.state.tool_approvals.write().await;
                    let tool_name = tool_call.name.clone();
                    // 只升级（不降级）该工具的批准级别
                    match approvals.get(&tool_name) {
                        Some(&current) if current >= level => {
                            // 已有相同或更高级别的批准，无需更新
                        }
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

                Ok(response.approved)
            }
            Ok(Err(_)) => {
                // oneshot sender dropped，清理并默认拒绝
                self.state.pending_permissions.lock().await.remove(&req_id);
                Ok(false)
            }
            Err(_) => {
                // 超时，清理并默认拒绝
                self.state.pending_permissions.lock().await.remove(&req_id);
                tracing::warn!("Permission request timeout for tool {}", tool_call.name);
                Ok(false)
            }
        }
    }
}

/// 权限响应器 - 用于外部发送权限响应
#[derive(Clone, Debug)]
pub struct Responder {
    pending_reqs: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
}

impl Responder {
    /// 响应权限请求
    ///
    /// 返回 true 表示成功发送响应，false 表示请求不存在或已超时
    pub async fn respond(&self, req_id: &str, approved: bool, remember: bool) -> bool {
        let response = Response { approved, remember };
        let mut pending = self.pending_reqs.lock().await;
        pending.remove(req_id).map_or_else(
            || {
                tracing::warn!(
                    "Permission response for unknown/timed out req_id: {}",
                    req_id
                );
                false
            },
            |sender| {
                if sender.send(response).is_err() {
                    tracing::warn!("Permission response receiver dropped for req_id={}", req_id);
                }
                tracing::info!(
                    "Permission response sent for req_id={}: approved={}, remember={}",
                    req_id,
                    approved,
                    remember
                );
                true
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;
    use serde_json::json;

    #[tokio::test]
    async fn test_permission_checker_auto_approve() {
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Caution 阈值 - Safe 工具应该自动通过
        let (state, _responder) = PermissionState::new(Level::Caution);
        let checker = Checker::new(state, AgentId::new(), event_tx.clone());

        let safe_tool = ToolCall {
            id: "test1".to_string(),
            name: "Read".to_string(),
            arguments: json!({}),
        };

        // Safe 工具应该自动批准（不需要发送事件）
        assert!(checker
            .check_permission(&safe_tool, Level::Safe)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_permission_responder() {
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let (state, responder) = PermissionState::new(Level::Safe); // Safe 级别，Caution 工具需要确认
        let checker = Checker::new(state, AgentId::new(), event_tx.clone());

        // 创建一个 Caution 级别的工具调用
        let caution_tool = ToolCall {
            id: "test1".to_string(),
            name: "Edit".to_string(),
            arguments: json!({"file_path": "test.txt"}),
        };

        // 在后台运行权限检查
        let checker_task = tokio::spawn(async move {
            checker
                .check_permission(&caution_tool, Level::Caution)
                .await
        });

        // 接收权限请求事件
        let event = event_rx.recv().await.unwrap();
        let req_id = match event {
            Event::Agent(AgentEvent::PermissionRequest { req_id, .. }) => req_id,
            _ => panic!("Expected PermissionRequest event"),
        };

        // 发送响应（不带 remember）
        let result = responder.respond(&req_id, true, false).await;
        assert!(result);

        // 验证权限检查返回批准
        let check_result = checker_task.await.unwrap().unwrap();
        assert!(check_result);
    }

    #[tokio::test]
    async fn test_permission_remember_per_tool() {
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let (state, responder) = PermissionState::new(Level::Safe); // Safe 级别，Caution 和 Dangerous 需要确认
        let checker = Checker::new(state, AgentId::new(), event_tx.clone());

        // Wrap checker in Arc so we can use it after spawning
        let checker = Arc::new(checker);

        // 第一步：请求 Edit 工具权限，选择 remember
        let edit_tool = ToolCall {
            id: "test1".to_string(),
            name: "Edit".to_string(),
            arguments: json!({"file_path": "test.txt"}),
        };

        let checker_clone = Arc::clone(&checker);
        let checker_task = tokio::spawn(async move {
            checker_clone
                .check_permission(&edit_tool, Level::Caution)
                .await
        });

        let event = event_rx.recv().await.unwrap();
        let req_id = match event {
            Event::Agent(AgentEvent::PermissionRequest { req_id, .. }) => req_id,
            _ => panic!("Expected PermissionRequest event"),
        };

        // 发送响应，选择 remember
        responder.respond(&req_id, true, true).await;
        let result = checker_task.await.unwrap().unwrap();
        assert!(result);

        // 第二步：再次请求 Edit 工具，应该自动通过（不需要事件）
        let edit_tool2 = ToolCall {
            id: "test2".to_string(),
            name: "Edit".to_string(),
            arguments: json!({"file_path": "test2.txt"}),
        };

        // 这次应该直接返回 true，不发事件（因为 Edit 已被记住批准）
        let result = checker
            .check_permission(&edit_tool2, Level::Caution)
            .await
            .unwrap();
        assert!(result);

        // 没有事件发送（因为 Edit 已自动批准）
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv()).await;
        assert!(
            timeout_result.is_err(),
            "Should not receive event for auto-approved Edit tool"
        );

        // 第三步：请求另一个 Caution 级别的工具（如 Write），应该需要确认
        // 因为 remember 只针对 Edit 工具，不影响其他工具
        let write_tool = ToolCall {
            id: "test3".to_string(),
            name: "Write".to_string(),
            arguments: json!({"file_path": "test3.txt"}),
        };

        let checker_clone = Arc::clone(&checker);
        let checker_task = tokio::spawn(async move {
            checker_clone
                .check_permission(&write_tool, Level::Caution)
                .await
        });

        // Write 工具应该触发权限请求事件（因为只记住了 Edit）
        let event = event_rx.recv().await;
        assert!(
            event.is_some(),
            "Write tool should trigger permission request"
        );

        // 清理：响应并结束任务
        if let Some(Event::Agent(AgentEvent::PermissionRequest { req_id, .. })) = event {
            responder.respond(&req_id, true, false).await;
        }
        let _ = checker_task.await;
    }
}
