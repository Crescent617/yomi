use std::collections::VecDeque;
use std::fmt;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use crate::agent::AgentInput;
use crate::types::ContentBlock;

/// 与 Agent 1:1 绑定的双队列缓冲。
/// steer 高优先级，在 Streaming 前批量消费；normal 普通消息，Idle 时逐条消费。
pub struct Mailbox {
    steer: Mutex<VecDeque<ContentBlock>>,
    normal: Mutex<VecDeque<AgentInput>>,
    notify: Notify,
}

impl fmt::Debug for Mailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mailbox")
            .field("steer_len", &self.steer.try_lock().map_or(0, |m| m.len()))
            .field("normal_len", &self.normal.try_lock().map_or(0, |m| m.len()))
            .finish_non_exhaustive()
    }
}

impl Mailbox {
    pub fn new() -> Self {
        Self {
            steer: Mutex::new(VecDeque::new()),
            normal: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        }
    }

    pub async fn push(&self, input: AgentInput) {
        self.normal.lock().await.push_back(input);
        self.notify.notify_one();
    }

    pub async fn push_steer(&self, content: Vec<ContentBlock>) {
        self.steer.lock().await.extend(content);
        self.notify.notify_one();
    }

    /// 批量消费 steer，最多 `count` 条 ContentBlock（flat）
    pub async fn try_pull_steer(&self, count: usize) -> Vec<ContentBlock> {
        let mut q = self.steer.lock().await;
        let n = count.min(q.len());
        q.drain(..n).collect()
    }

    /// 批量消费 normal，最多 `count` 条 `AgentInput`
    pub async fn try_pull(&self, count: usize) -> Vec<AgentInput> {
        let mut q = self.normal.lock().await;
        let n = count.min(q.len());
        q.drain(..n).collect()
    }

    /// 只读检查 steer 是否为空（Idle 分支插队判断）
    pub fn is_steer_empty(&self) -> bool {
        self.steer.try_lock().is_ok_and(|m| m.is_empty())
    }

    /// 检查双队列是否都为空
    pub fn is_empty(&self) -> bool {
        self.steer.try_lock().is_ok_and(|m| m.is_empty())
            && self.normal.try_lock().is_ok_and(|m| m.is_empty())
    }

    /// 清空双队列（cancel 时使用）
    pub async fn clear(&self) {
        self.steer.lock().await.clear();
        self.normal.lock().await.clear();
    }

    /// 等待有新消息到达（可配合 select! 使用）
    pub async fn wait_for_mail(&self) {
        // Fast path: if there's already mail, don't wait
        if !self.is_empty() {
            return;
        }
        self.notify.notified().await;
    }
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}
