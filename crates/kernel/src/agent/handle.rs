use crate::agent::{AgentError, AgentInput, AgentState, CancelToken};
use crate::permissions::Responder;
use crate::types::{AgentId, ContentBlock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

/// 外部控制运行中 Agent 的句柄
#[derive(Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    pub(super) input_tx: mpsc::Sender<AgentInput>,
    pub(super) state_rx: tokio::sync::watch::Receiver<AgentState>,
    cancel_token: CancelToken,
    pub(super) permission_responder: Option<Responder>,
    /// Generation counter: inputs with lower generation are stale (cancelled before send)
    input_stale_since: Arc<AtomicU64>,
}

impl std::fmt::Debug for AgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHandle")
            .field("id", &self.id)
            .field("cancel_token", &self.cancel_token)
            .field("permission_responder", &self.permission_responder.is_some())
            .field(
                "input_generation",
                &self.input_stale_since.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl AgentHandle {
    pub fn new(
        id: AgentId,
        input_tx: mpsc::Sender<AgentInput>,
        state_rx: tokio::sync::watch::Receiver<AgentState>,
        cancel_token: CancelToken,
        permission_responder: Option<Responder>,
        input_stale_since: Arc<AtomicU64>,
    ) -> Self {
        Self {
            id,
            input_tx,
            state_rx,
            cancel_token,
            permission_responder,
            input_stale_since,
        }
    }

    /// 发送用户消息给 Agent（支持多模态内容）
    pub async fn send_message(&self, content: Vec<ContentBlock>) -> Result<(), AgentError> {
        let generation = self.input_stale_since.load(Ordering::Relaxed);
        let input = AgentInput::User {
            content,
            generation,
        };
        self.input_tx
            .send(input)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// 发送用户文本消息给 Agent（便捷方法）
    pub async fn send_text(&self, text: String) -> Result<(), AgentError> {
        self.send_message(vec![ContentBlock::Text { text }]).await
    }

    /// 发送权限响应给 Agent
    pub async fn send_permission_response(
        &self,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<(), AgentError> {
        if let Some(ref responder) = self.permission_responder {
            responder.respond(req_id, approved, remember).await;
            Ok(())
        } else {
            Err(AgentError::NoPermissionChecker)
        }
    }

    /// 获取当前状态
    pub fn state(&self) -> AgentState {
        *self.state_rx.borrow()
    }

    /// 等待状态变化
    pub async fn wait_for_state_change(&mut self) -> AgentState {
        let _ = self.state_rx.changed().await;
        *self.state_rx.borrow()
    }

    /// 请求取消，同时递增 generation 使此前发送的输入变为 stale
    pub fn cancel(&self) {
        self.input_stale_since.fetch_add(1, Ordering::SeqCst);
        self.cancel_token.cancel();
    }

    /// 优雅地关闭 Agent（发送 Close 信号，区别于 Cancel）
    pub async fn close(&self) -> Result<(), AgentError> {
        self.input_tx
            .send(super::AgentInput::Shutdown)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }

    /// 请求强制压缩消息缓冲区
    pub async fn force_compact(&self) -> Result<(), AgentError> {
        self.input_tx
            .send(AgentInput::Compact)
            .await
            .map_err(|_| AgentError::ChannelClosed)
    }
}
