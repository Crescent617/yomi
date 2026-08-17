use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{ContentBlock, FinishReason, Message, MessageId, MessageTokenUsage, Role};

/// 会话消息列表 API 的清晰结构（与 storage Message 解耦）。
/// 从类型层面区分 user / assistant / tool，消灭旧 Message 里大量 Option 的模糊性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionMessage {
    User(UserMsg),
    Steer(UserMsg),
    /// 中断标记（`Agent::mark_interrupted`）：GUI 渲染为分割线而非用户气泡
    Interrupted(UserMsg),
    Assistant(AssistantMsg),
    Tool(ToolMsg),
}

/// 用户消息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserMsg {
    pub id: MessageId,
    pub content: Vec<ContentBlock>,
    pub created_at: DateTime<Utc>,
}

/// 助手消息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AssistantMsg {
    pub id: MessageId,
    pub content: Vec<ContentBlock>,
    pub token_usage: Option<MessageTokenUsage>,
    pub response_id: Option<String>,
    pub model_id: Option<String>,
    pub finish_reason: Option<FinishReason>,
    pub created_at: DateTime<Utc>,
}

/// 工具结果消息 —— 自包含，无需前端再查 `tool_call_id` 配对
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolMsg {
    pub id: MessageId,
    pub tool_call_id: String,
    pub name: String,
    pub args: String,
    pub result: Vec<ContentBlock>,
    pub meta: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

impl SessionMessage {
    /// 从 storage 层 `Message` 转换。
    /// - 先扫描一遍建立两个索引：
    ///   1. `tool_call_id → (name, args)` 从 assistant 的 `tool_calls` 提取
    ///   2. `tool_call_id → metadata` 从 internal 消息的 metadata 提取（如 `subagent_session_id`）
    /// - system / internal 消息本身被过滤，但 internal 的 metadata 会被合并到对应的 tool 消息
    pub fn from_storage(messages: Vec<Message>) -> Vec<Self> {
        // 1. 建立 tool_call_id 索引
        let mut tool_call_index: HashMap<String, (String, String)> = HashMap::new();
        // 2. 建立 `tool_call_id → metadata` 索引（从 Internal 消息收集）
        let mut tool_meta_index: HashMap<String, HashMap<String, String>> = HashMap::new();

        for msg in &messages {
            if msg.role == Role::Assistant {
                if let Some(ref calls) = msg.tool_calls {
                    for call in calls {
                        let args = serde_json::to_string(&call.arguments).unwrap_or_default();
                        tool_call_index.insert(call.id.clone(), (call.name.clone(), args));
                    }
                }
            } else if msg.role == Role::Internal {
                let (Some(ref tool_call_id), Some(ref metadata)) =
                    (&msg.tool_call_id, &msg.metadata)
                else {
                    continue;
                };
                tool_meta_index.insert(tool_call_id.clone(), metadata.clone());
            }
        }

        let mut result = Vec::with_capacity(messages.len());
        for msg in messages {
            match msg.role {
                Role::User => {
                    let user_msg = UserMsg {
                        id: msg.id,
                        content: msg.content,
                        created_at: msg.created_at,
                    };
                    let is_steer = msg
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.get(crate::types::IS_STEER_META_KEY))
                        .is_some_and(|value| value == "true");
                    let interrupted = msg
                        .metadata
                        .as_ref()
                        .and_then(|meta| meta.get(crate::types::INTERRUPTED_META_KEY))
                        .is_some_and(|value| value == "true");
                    result.push(if interrupted {
                        SessionMessage::Interrupted(user_msg)
                    } else if is_steer {
                        SessionMessage::Steer(user_msg)
                    } else {
                        SessionMessage::User(user_msg)
                    });
                }
                Role::Assistant => {
                    result.push(SessionMessage::Assistant(AssistantMsg {
                        id: msg.id,
                        content: msg.content,
                        token_usage: msg.token_usage,
                        response_id: msg.response_id,
                        model_id: msg.model_id,
                        finish_reason: msg.finish_reason,
                        created_at: msg.created_at,
                    }));
                }
                Role::Tool => {
                    let Some(tool_call_id) = msg.tool_call_id else {
                        // tool_call_id 缺失，数据损坏，丢弃
                        tracing::warn!(msg_id = ?msg.id, "Tool message missing tool_call_id, skipping");
                        continue;
                    };
                    // 找不到对应的 assistant tool_call 说明历史被截断或数据不一致，同样跳过
                    let Some((name, args)) = tool_call_index.get(&tool_call_id).cloned() else {
                        tracing::warn!(
                            tool_call_id,
                            msg_id = ?msg.id,
                            "Tool message has no matching assistant tool_call (history truncated?), skipping"
                        );
                        continue;
                    };
                    // 合并 metadata：Internal 消息的 metadata 优先于 Tool 消息自身的 metadata
                    let mut meta = msg.metadata.unwrap_or_default();
                    if let Some(internal_meta) = tool_meta_index.get(&tool_call_id) {
                        for (k, v) in internal_meta {
                            meta.insert(k.clone(), v.clone());
                        }
                    }
                    result.push(SessionMessage::Tool(ToolMsg {
                        id: msg.id,
                        tool_call_id,
                        name,
                        args,
                        result: msg.content,
                        meta,
                        created_at: msg.created_at,
                    }));
                }
                Role::System | Role::Internal => {
                    // system 消息过滤；internal 消息的 metadata 已在第一遍扫描中收集
                }
            }
        }
        result
    }
}

impl SessionMessage {
    /// Concatenate all text content from the message blocks.
    pub fn text_content(&self) -> String {
        match self {
            SessionMessage::User(msg) | SessionMessage::Steer(msg) => msg.text_content(),
            SessionMessage::Interrupted(msg) => msg.text_content(),
            SessionMessage::Assistant(msg) => msg.text_content(),
            SessionMessage::Tool(msg) => msg.text_content(),
        }
    }

    /// Get token usage if this is an assistant message that recorded it.
    pub fn token_usage(&self) -> Option<&MessageTokenUsage> {
        match self {
            SessionMessage::Assistant(msg) => msg.token_usage.as_ref(),
            _ => None,
        }
    }
}

/// Extract plain text from a slice of content blocks.
fn extract_text_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

impl UserMsg {
    pub fn text_content(&self) -> String {
        extract_text_from_blocks(&self.content)
    }
}

impl AssistantMsg {
    pub fn text_content(&self) -> String {
        extract_text_from_blocks(&self.content)
    }

    pub fn thinking_content(&self) -> Option<String> {
        let thinking: Vec<&str> = self
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                _ => None,
            })
            .collect();
        if thinking.is_empty() {
            None
        } else {
            Some(thinking.join(""))
        }
    }
}

impl ToolMsg {
    pub fn text_content(&self) -> String {
        extract_text_from_blocks(&self.result)
    }
}
