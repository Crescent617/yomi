use crate::agent::{AgentInput, AgentState, CancellationToken};
use crate::types::AgentId;
use tokio::sync::mpsc;

/// 外部控制运行中 Agent 的句柄
#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    pub(super) input_tx: mpsc::Sender<AgentInput>,
    pub(super) state_rx: tokio::sync::watch::Receiver<AgentState>,
    cancel_token: CancellationToken,
}

impl AgentHandle {
    pub const fn new(
        id: AgentId,
        input_tx: mpsc::Sender<AgentInput>,
        state_rx: tokio::sync::watch::Receiver<AgentState>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            id,
            input_tx,
            state_rx,
            cancel_token,
        }
    }

    /// 发送用户消息给 Agent
    pub async fn send_message(&self, content: String) -> anyhow::Result<()> {
        self.input_tx
            .send(AgentInput::User(content))
            .await
            .map_err(|_| anyhow::anyhow!("Agent {} input channel closed", self.id.0))
    }

    /// 发送工具结果给 Agent
    pub async fn send_tool_result(&self, tool_id: String, output: String) -> anyhow::Result<()> {
        self.input_tx
            .send(AgentInput::ToolResult { tool_id, output })
            .await
            .map_err(|_| anyhow::anyhow!("Agent {} input channel closed", self.id.0))
    }

    /// 发送取消信号给 Agent
    pub async fn send_cancel(&self) -> anyhow::Result<()> {
        self.input_tx
            .send(AgentInput::Cancel)
            .await
            .map_err(|_| anyhow::anyhow!("Agent {} input channel closed", self.id.0))
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

    /// 请求取消
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// 检查是否已请求取消
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }
}
