//! 工具调用循环哨兵（loop guard）——L1 警告 / L2 熔断。
//!
//! 纯函数检测器，悬在工具批收尾处（`tool_exec::finish_tool_batch`）：
//! 同一调用——工具名 + canonical 参数 + **完全相同的结果**——在
//! **连续**的工具批中重复出现，即判定模型陷入死循环：
//!
//! - L1 `Warn`（达 `warn_at` 次）：注入一条 user 提醒，给模型一次
//!   自纠机会（与 auto-continue 的 `"continue"` 同一注入路径）；
//! - L2 `Break`（达 `break_at` 次）：记 `StopReason::ToolLoop`，
//!   走 `WindingDown` 统一收尾，与 `max_iterations` 同路径。
//!
//! 两条防误伤原则：
//!
//! - **连续**才计数：轮询等待（A,B,A,B 交错）形不成连续 streak，
//!   「等构建完成再查一次」这类合法节奏不会被掐；
//! - **结果相同**才计数：改完文件重读这类合理重复，结果已变、
//!   指纹不同，天然豁免。
//!
//! 检测只读 message buffer 尾部、无内部状态：不按 turn 重置、不怕
//! respawn、不怕压缩重写——看到的永远是消息流本身（the stream is
//! reality），无状态可漂移。

use crate::types::{ContentBlock, Message, Role};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// 哨兵阈值（由 `AgentConfig::tool_loop_guard` 装配，随 agent 生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopGuard {
    /// 同一调用连续重复达到该次数 → `Warn`（0 = 不发警告）。
    pub warn_at: usize,
    /// 同一调用连续重复达到该次数 → `Break`（0 = 关闭哨兵）。
    pub break_at: usize,
}

impl Default for LoopGuard {
    fn default() -> Self {
        Self {
            warn_at: 2,
            break_at: 3,
        }
    }
}

/// 对刚执行完的工具批的处置信号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopSignal {
    None,
    /// 同一调用已连续重复 `streak` 次——提醒模型换方法。
    Warn {
        tool: String,
        streak: usize,
    },
    /// 同一调用已连续重复 `streak` 次——结束 turn。
    Break {
        tool: String,
        streak: usize,
    },
}

impl LoopSignal {
    fn severity(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::Warn { .. } => 1,
            Self::Break { .. } => 2,
        }
    }
}

/// 检测 buffer 尾部是否出现工具调用死循环。
pub fn detect(messages: &[Arc<Message>], guard: LoopGuard) -> LoopSignal {
    if guard.break_at == 0 {
        return LoopSignal::None;
    }
    let batches = recent_batches(messages, guard.break_at);
    let Some(current) = batches.last() else {
        return LoopSignal::None;
    };
    let mut signal = LoopSignal::None;
    for call in current {
        // streak = 当前批 + 紧邻之前连续含有同指纹调用的批数。
        let streak = 1 + batches[..batches.len() - 1]
            .iter()
            .rev()
            .take_while(|batch| batch.iter().any(|prev| prev.print == call.print))
            .count();
        let hit = if streak >= guard.break_at {
            LoopSignal::Break {
                tool: call.tool.clone(),
                streak,
            }
        } else if guard.warn_at > 0 && streak >= guard.warn_at {
            LoopSignal::Warn {
                tool: call.tool.clone(),
                streak,
            }
        } else {
            continue;
        };
        // 同批多个调用命中时取最严重者（Break > Warn）。
        if hit.severity() > signal.severity() {
            signal = hit;
        }
    }
    signal
}

/// 一次调用的指纹：结果不同的重复调用（如改后重读）指纹不同，
/// 不计入循环。
struct CallPrint {
    tool: String,
    print: u64,
}

/// 取尾部最近 `depth` 个工具批（含刚完成的批），按时间序返回。
/// 扫描深度即熔断阈值：streak 超过阈值与等于阈值不可分，无需回看更多。
fn recent_batches(messages: &[Arc<Message>], depth: usize) -> Vec<Vec<CallPrint>> {
    // 倒扫时工具结果先于其 assistant 批出现——批构建时结果已在图中。
    let mut results: HashMap<&str, &Message> = HashMap::new();
    let mut batches: Vec<Vec<CallPrint>> = Vec::new();
    for msg in messages.iter().rev() {
        match msg.role {
            Role::Tool => {
                if let Some(id) = msg.tool_call_id.as_deref() {
                    results.entry(id).or_insert(msg);
                }
            }
            Role::Assistant => {
                let Some(calls) = msg.tool_calls.as_ref().filter(|c| !c.is_empty()) else {
                    continue;
                };
                // 无结果的悬挂调用（mid-batch kill）不参与判定。
                let batch = calls
                    .iter()
                    .filter_map(|call| {
                        let result = results.get(call.id.as_str())?;
                        Some(CallPrint {
                            tool: call.name.clone(),
                            print: fingerprint(&call.name, &call.arguments, result),
                        })
                    })
                    .collect();
                batches.push(batch);
                if batches.len() == depth {
                    break;
                }
            }
            _ => {}
        }
    }
    batches.reverse();
    batches
}

fn fingerprint(name: &str, args: &serde_json::Value, result: &Message) -> u64 {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    canonical_args(args).hash(&mut hasher);
    for block in &result.content {
        match block {
            ContentBlock::Text { text } => {
                0u8.hash(&mut hasher);
                text.hash(&mut hasher);
            }
            ContentBlock::Thinking { thinking, .. } => {
                1u8.hash(&mut hasher);
                thinking.hash(&mut hasher);
            }
            ContentBlock::RedactedThinking { data } => {
                2u8.hash(&mut hasher);
                data.hash(&mut hasher);
            }
            ContentBlock::ImageUrl { image_url } => {
                3u8.hash(&mut hasher);
                image_url.url.hash(&mut hasher);
            }
            ContentBlock::Audio { audio } => {
                4u8.hash(&mut hasher);
                audio.format.hash(&mut hasher);
                audio.data.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// 参数 canonical 化：对象 key 排序序列化——不同轮次/provider 重排
/// key 序不逃脱指纹。（数值 1 与 1.0 序列化不同，视为不同参数：
/// 宁可漏检一次重试，不误伤合法调用。）
fn canonical_args(args: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(args, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        _ => out.push_str(&serde_json::to_string(value).unwrap_or_default()),
    }
}

#[cfg(test)]
#[path = "loop_detect_test.rs"]
mod tests;
