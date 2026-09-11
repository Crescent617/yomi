use crate::types::{Message, MessageId};
use std::sync::Arc;

/// Simple message buffer for agent conversation history
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Arc<Message>>,
}

impl Default for MessageBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl MessageBuffer {
    /// Create an empty buffer
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Create from existing messages (for recovery)
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages: messages.into_iter().map(Arc::new).collect(),
        }
    }

    /// Create from existing Arc messages (internal use)
    pub fn from_arc_messages(messages: &[Arc<Message>]) -> Self {
        Self {
            messages: messages.to_vec(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(Arc::new(message));
    }

    /// Push an already-arc-wrapped message
    pub fn push_arc(&mut self, message: Arc<Message>) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Arc<Message>] {
        &self.messages
    }

    /// Get mutable access to the underlying vector (use with caution)
    pub fn messages_mut(&mut self) -> &mut Vec<Arc<Message>> {
        &mut self.messages
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Update a message using Copy-on-Write pattern
    /// If the Arc is shared, it will be cloned before modification
    pub fn update_message<F>(&mut self, idx: usize, f: F)
    where
        F: FnOnce(&mut Message),
    {
        if let Some(arc) = self.messages.get_mut(idx) {
            // Arc::make_mut will clone the inner data if it's shared
            let message = Arc::make_mut(arc);
            f(message);
        }
    }

    /// Get a clone of the messages as a new Vec<Arc<Message>>
    pub fn clone_messages(&self) -> Vec<Arc<Message>> {
        self.messages.clone()
    }

    /// Return the provider-facing message view: internal metadata removed and
    /// incomplete assistant/tool groups sanitized without mutating stored history.
    pub fn sanitized_model_messages(messages: &[Arc<Message>]) -> Vec<Arc<Message>> {
        let mut buffer = Self {
            messages: messages
                .iter()
                .filter(|message| message.role != crate::types::Role::Internal)
                .cloned()
                .collect(),
        };
        buffer.sanitize();
        buffer.messages
    }

    /// Close out dangling tool batches in a loaded history: every assistant
    /// `tool_calls` entry without a matching tool result gets a synthesized
    /// cancelled result inserted at the close-out point (before the next
    /// chain-breaking message, or at the end).
    ///
    /// 统一兜底所有缺口成因：cancel 路径的合成 cancelled 结果与
    /// interruption marker 同走可丢总线（饱和时双丢）、进程崩溃、
    /// shutdown 拆除竞速。补齐后 assistant→tool 链完整，`sanitize`
    /// 不再整组剔除中断痕迹，模型对「这批调用已取消」知情——重发与否
    /// 是模型的知情决策，而非痕迹被抹后的无意识重跑。与 `sanitize`
    /// 同口径：`Role::Internal` 对链透明（不打断开放批、不参与核销）。
    /// 迁移优先于合成：jsonl 落盘是 append-only，上轮 respawn 合成的
    /// 结果、总线乱序的真实结果都可能写在批的关账点之后（甚至文件
    /// 尾）。关账点发现该 call 已有结果（位置乱序）时把它迁移回批
    /// 内，而不是再合成一条——这是跨 respawn 幂等的关键：只补缺不
    /// 迁移时，尾部已有的上轮合成结果无法核销，每轮 respawn 重复
    /// 合成，文件无限 +N（2026-09-11 对抗 review 实测）。乱序布局
    /// 不 rewrite 文件，只在每次读入时内存修复。
    ///
    /// 幂等：已关账/可迁移的批不产生新消息。
    ///
    /// Returns the closed-out history plus the synthesized messages, in
    /// the same relative order (the caller persists the latter).
    pub fn close_dangling_tool_batches(
        messages: &[Arc<Message>],
        max_tool_output_length: usize,
    ) -> (Vec<Arc<Message>>, Vec<Message>) {
        use crate::types::Role;
        use std::collections::{HashMap, HashSet};

        // 趟 1：索引——全部批的 call_id 集合，及属于某批的 tool 结果
        // 的首条位置（同 id 病态重复的多余条在趟 2 按孤儿原位保留）。
        let mut batch_ids: HashSet<&str> = HashSet::new();
        for msg in messages {
            if msg.role == Role::Assistant {
                if let Some(calls) = msg.tool_calls.as_ref() {
                    batch_ids.extend(calls.iter().map(|c| c.id.as_str()));
                }
            }
        }
        let mut results: HashMap<&str, (usize, &Arc<Message>)> = HashMap::new();
        for (seq, msg) in messages.iter().enumerate() {
            if msg.role != Role::Tool {
                continue;
            }
            let Some(id) = msg.tool_call_id.as_deref() else {
                continue;
            };
            if batch_ids.contains(id) {
                results.entry(id).or_insert((seq, msg));
            }
        }

        // 趟 2：重建——原位配对直接保留；错位结果留待关账点迁移；
        // 真缺口合成 cancelled。
        let mut out: Vec<Arc<Message>> = Vec::with_capacity(messages.len());
        let mut synthesized: Vec<Message> = Vec::new();
        let mut consumed: HashSet<usize> = HashSet::new();
        // 当前未关账批的剩余调用（最近一个带 tool_calls 的 assistant）。
        let mut open: Vec<crate::types::ToolCall> = Vec::new();

        for (seq, msg) in messages.iter().enumerate() {
            match msg.role {
                // 透明：不打断开放批（与 sanitize 同口径）。
                Role::Internal => out.push(msg.clone()),
                Role::Tool => {
                    let id = msg.tool_call_id.as_deref();
                    if consumed.contains(&seq) {
                        // 已在关账点被迁移输出过。
                        continue;
                    }
                    if let Some(pos) = id.and_then(|id| open.iter().position(|c| c.id == id)) {
                        // 原位配对当前开放批。
                        consumed.insert(seq);
                        open.remove(pos);
                        out.push(msg.clone());
                        continue;
                    }
                    match id.and_then(|i| results.get(i)) {
                        Some(&(result_seq, _)) if result_seq == seq => {
                            // 属于某批的配对结果但不在原位（迟到/错位）：
                            // 留待该批关账点迁移输出，此处跳过。
                        }
                        _ => {
                            // 真孤儿（不属于任何批，或同 id 病态重复的
                            // 多余条）：原位保留，交 sanitize 裁决。
                            out.push(msg.clone());
                        }
                    }
                }
                // 其他任何角色打断链：先把开放批关账（结果插在该消息
                // 之前——abort 时结果本就先于 marker 落盘，时序一致），
                // 再开新批（若新 assistant 带调用）。
                _ => {
                    close_open_batch(
                        &mut open,
                        &results,
                        &mut consumed,
                        max_tool_output_length,
                        &mut out,
                        &mut synthesized,
                    );
                    if msg.role == Role::Assistant {
                        if let Some(calls) = msg.tool_calls.as_ref().filter(|tc| !tc.is_empty()) {
                            open = calls.clone();
                        }
                    }
                    out.push(msg.clone());
                }
            }
        }
        // 历史末尾的 dangling 批：尾部关账。
        close_open_batch(
            &mut open,
            &results,
            &mut consumed,
            max_tool_output_length,
            &mut out,
            &mut synthesized,
        );
        (out, synthesized)
    }

    /// Sanitize the message buffer by removing inconsistent tool call/response pairs.
    /// Removes assistant messages with `tool_calls` that don't have corresponding tool responses,
    /// and removes tool responses that are not immediately after their corresponding assistant.
    /// `Role::Internal` messages are transparent to chain validation: they
    /// carry UI metadata (e.g. subagent placeholders persisted at tool
    /// start) and never reach the provider, so they neither join a chain
    /// nor break one — otherwise every respawn would strip subagent tool
    /// chains whose jsonl interleaves them (2026-09-11 对抗 review 发现).
    /// Also removes empty assistant messages (no content, no tool calls) — poison
    /// persisted by a model hiccup (empty completion); replaying them makes strict
    /// gateways 400 every request. Dropping them here lets already-poisoned
    /// sessions self-heal on the next turn.
    /// Time: O(n), Space: O(k) where k = number of pending tool calls
    pub fn sanitize(&mut self) {
        use crate::types::Role;
        use std::collections::HashSet;

        // First pass: find all valid (assistant -> tool chain) groups
        // A tool response is valid only if it follows its assistant with
        // only Internal messages (transparent) in between.
        let mut to_remove = HashSet::new();
        let n = self.messages.len();
        let mut i = 0;
        let mut expected_tool_ids = HashSet::new();
        let mut tool_msg_indices = Vec::new();

        while i < n {
            let msg = &self.messages[i];

            // Non-assistant: Tool gets marked, others skipped
            let Role::Assistant = msg.role else {
                if msg.role == Role::Tool {
                    to_remove.insert(i);
                }
                i += 1;
                continue;
            };

            // Assistant without tool_calls: keep it unless it carries no
            // content at all — that shape is empty-completion poison.
            let Some(calls) = msg.tool_calls.as_ref() else {
                if msg.content.is_empty() {
                    to_remove.insert(i);
                }
                i += 1;
                continue;
            };

            expected_tool_ids.clear();
            tool_msg_indices.clear();

            for call in calls {
                expected_tool_ids.insert(call.id.clone());
            }

            let tool_call_count = calls.len();
            let mut valid_chain = true;
            let mut cursor = i + 1;

            while tool_msg_indices.len() < tool_call_count {
                let Some(next) = self.messages.get(cursor) else {
                    valid_chain = false;
                    break;
                };
                match next.role {
                    // 透明：不参与链、不断链，也不计入移除集。
                    Role::Internal => cursor += 1,
                    Role::Tool => {
                        tool_msg_indices.push(cursor);
                        let Some(ref tool_call_id) = next.tool_call_id else {
                            valid_chain = false;
                            break;
                        };
                        if !expected_tool_ids.remove(tool_call_id) {
                            valid_chain = false;
                            break;
                        }
                        cursor += 1;
                    }
                    _ => {
                        valid_chain = false;
                        break;
                    }
                }
            }

            // Check if all expected tool calls have responses
            if valid_chain && !expected_tool_ids.is_empty() {
                valid_chain = false;
            }

            if !valid_chain {
                to_remove.insert(i);
                to_remove.extend(tool_msg_indices.iter());
                // cursor 停在断点（未消费）；从断点重扫，Internal 会
                // 被快速跳过、Tool 作为孤儿标记——每条消息摊销 O(1)。
                i = cursor;
                continue;
            }

            // Valid chain - skip past the whole scanned span
            i = cursor;
        }

        if to_remove.is_empty() {
            return;
        }

        let mut i = 0;
        self.messages.retain(|_| {
            let keep = !to_remove.contains(&i);
            i += 1;
            keep
        });
    }
}

/// Drain the open batch: append a synthesized cancelled result per
/// Drain the open batch at a close-out point. A call whose result already
/// exists out of place (late persist, previous respawn's append at the
/// file tail) gets that message **migrated** into the batch — only a true
/// gap gets a newly synthesized cancelled result (appended to both the
/// closed-out history and the persist list). Migration is what makes the
/// close-out idempotent across respawns on the append-only jsonl layout.
fn close_open_batch(
    open: &mut Vec<crate::types::ToolCall>,
    results: &std::collections::HashMap<&str, (usize, &Arc<Message>)>,
    consumed: &mut std::collections::HashSet<usize>,
    max_tool_output_length: usize,
    out: &mut Vec<Arc<Message>>,
    synthesized: &mut Vec<Message>,
) {
    for call in open.drain(..) {
        match results.get(call.id.as_str()) {
            Some(&(seq, msg)) if !consumed.contains(&seq) => {
                consumed.insert(seq);
                out.push(msg.clone());
            }
            _ => {
                let (_, message) = crate::tools::executor::build_cancelled_result(
                    &call.id,
                    &call.name,
                    MessageId::new(),
                    max_tool_output_length,
                );
                out.push(Arc::new(message.clone()));
                synthesized.push(message);
            }
        }
    }
}

#[cfg(test)]
#[path = "message_buffer_test.rs"]
mod tests;
