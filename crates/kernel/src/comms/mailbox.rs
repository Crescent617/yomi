use std::collections::VecDeque;
use std::fmt;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use crate::agent::AgentInput;
use crate::types::{ContentBlock, MailboxItemId};

/// 与 Agent 1:1 绑定的双队列缓冲。
/// steer 高优先级，在 Streaming 前批量消费；normal 普通消息，Idle 时逐条消费。
pub struct Mailbox {
    steer: Mutex<VecDeque<MailboxEntry>>,
    normal: Mutex<VecDeque<MailboxEntry>>,
    notify: Notify,
}

/// 一条 pending 条目：push 时发放 id（ULID，天然不重用），供管理面寻址。
pub struct MailboxEntry {
    pub id: MailboxItemId,
    pub enqueued_at: chrono::DateTime<chrono::Utc>,
    pub input: AgentInput,
}

impl MailboxEntry {
    fn new(input: AgentInput) -> Self {
        Self {
            id: MailboxItemId::new(),
            enqueued_at: chrono::Utc::now(),
            input,
        }
    }
}

/// 管理面暴露的条目类别：steer（优先队列）/ queue（normal 里的用户消息）。
/// 控制输入（Compact/Rewind/Clear/Continue/Shutdown）留在内部，不出现在快照里。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxItemKind {
    Steer,
    Queue,
}

/// 快照里的单条 pending 项（wire payload；预览在入队内容里截取）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MailboxItem {
    pub id: MailboxItemId,
    pub kind: MailboxItemKind,
    /// 展示用预览（拍平 + ≤80 字符）。
    pub preview: String,
    /// 首个文本块的全文（供前端"编辑后重发"）；非文本消息为 None。
    pub text: Option<String>,
    pub blocks_len: usize,
    pub enqueued_at: chrono::DateTime<chrono::Utc>,
}

/// `clear_mailbox` 的范围。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MailboxScope {
    Steer,
    Queue,
    All,
}

/// 双队列快照：steer + queue（用户消息），均按 FIFO。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MailboxSnapshot {
    pub steer: Vec<MailboxItem>,
    pub queue: Vec<MailboxItem>,
}

/// 从内容块里取预览与全文：首个文本块；预览拍平截断。
fn texts_of(blocks: &[ContentBlock]) -> (String, Option<String>, usize) {
    let text = blocks.iter().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.clone()),
        _ => None,
    });
    let flat = text
        .as_deref()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
    let preview = match flat {
        Some(f) if f.chars().count() > 80 => format!("{}…", f.chars().take(79).collect::<String>()),
        Some(f) => f,
        None if blocks.is_empty() => String::new(),
        None => "[non-text content]".to_string(),
    };
    (preview, text, blocks.len())
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
        self.normal.lock().await.push_back(MailboxEntry::new(input));
        self.notify.notify_one();
    }

    pub async fn push_steer(&self, content: Vec<ContentBlock>) {
        self.steer
            .lock()
            .await
            .push_back(MailboxEntry::new(AgentInput::Steer(content)));
        self.notify.notify_one();
    }

    /// Consume up to `count` queued steer messages and separate each message with a blank line.
    pub async fn try_pull_steer(&self, count: usize) -> Vec<ContentBlock> {
        let mut q = self.steer.lock().await;
        let n = count.min(q.len());
        let messages: Vec<_> = q.drain(..n).collect();
        let mut merged = Vec::new();

        for (index, entry) in messages.into_iter().enumerate() {
            let AgentInput::Steer(message) = entry.input else {
                debug_assert!(false, "steer queue holds only Steer inputs");
                continue;
            };
            if index > 0 {
                merged.push(ContentBlock::Text {
                    text: "\n\n".to_string(),
                });
            }
            merged.extend(message);
        }

        merged
    }

    /// 批量消费 normal，最多 `count` 条 `AgentInput`
    pub async fn try_pull(&self, count: usize) -> Vec<AgentInput> {
        let mut q = self.normal.lock().await;
        let n = count.min(q.len());
        q.drain(..n).map(|entry| entry.input).collect()
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

    /// 双队列长度（steer, normal）——`MailboxChanged` 事件的载荷。
    pub async fn lens(&self) -> (usize, usize) {
        (
            self.steer.lock().await.len(),
            self.normal.lock().await.len(),
        )
    }

    /// 管理面快照：steer + normal 里的用户消息（控制输入不暴露）。
    pub async fn snapshot(&self) -> MailboxSnapshot {
        let item = |kind: MailboxItemKind, entry: &MailboxEntry| -> Option<MailboxItem> {
            let blocks: &[ContentBlock] = match &entry.input {
                AgentInput::Steer(blocks) | AgentInput::User { content: blocks } => blocks,
                _ => return None,
            };
            let (preview, text, blocks_len) = texts_of(blocks);
            Some(MailboxItem {
                id: entry.id.clone(),
                kind,
                preview,
                text,
                blocks_len,
                enqueued_at: entry.enqueued_at,
            })
        };
        let steer = self
            .steer
            .lock()
            .await
            .iter()
            .filter_map(|e| item(MailboxItemKind::Steer, e))
            .collect();
        let queue = self
            .normal
            .lock()
            .await
            .iter()
            .filter_map(|e| item(MailboxItemKind::Queue, e))
            .collect();
        MailboxSnapshot { steer, queue }
    }

    /// 撤回一条 pending（best-effort：已被消费则 false）。
    pub async fn remove(&self, id: &MailboxItemId) -> bool {
        {
            let mut q = self.steer.lock().await;
            if let Some(index) = q.iter().position(|e| &e.id == id) {
                q.remove(index);
                return true;
            }
        }
        self.take(id).await.is_some()
    }

    /// 从 normal 队列取出一条（移动语义；steer 队列不参与——steer 已
    /// 是最高优先，没有"再提升"的对象）。
    pub async fn take(&self, id: &MailboxItemId) -> Option<AgentInput> {
        let mut q = self.normal.lock().await;
        let index = q.iter().position(|e| &e.id == id)?;
        Some(q.remove(index).expect("index from position").input)
    }

    /// 按范围清空（管理面操作；不同于 cancel 的全清，不影响 agent 运行）。
    /// normal 队列只清用户消息：控制输入（Compact/Rewind/…）是内部瞬态项
    /// （且 Rewind 带挂起的 result_tx），不动它们。
    pub async fn clear_scope(&self, scope: MailboxScope) -> usize {
        let mut removed = 0;
        if matches!(scope, MailboxScope::Steer | MailboxScope::All) {
            removed += self.steer.lock().await.drain(..).count();
        }
        if matches!(scope, MailboxScope::Queue | MailboxScope::All) {
            let mut q = self.normal.lock().await;
            let before = q.len();
            q.retain(|e| !matches!(e.input, AgentInput::User { .. }));
            removed += before - q.len();
        }
        removed
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
