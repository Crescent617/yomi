//! `/btw` 旁问：conductor 侧的只读快照旁路。
//!
//! 快照的四样原料全部来自 agent 之外的公共源——storage 里的会话历史、
//! 事件流里的半截 assistant 文本、与主 loop 同源的 tools/model 解析
//! （均由 conductor 准备，见 `conductor::start_btw`）。agent 对旁问零
//! 感知：没有泳道、没有发布义务、没有生命周期耦合。
//!
//! 请求与主 loop 同源构建（同函数组装 tools/system/messages 前缀，
//! prompt cache 友好；日期与项目 memory 现读，跨日或编辑后内容自然
//! 漂移），差异只有末尾追加的一条包裹 user 消息。不执行工具、不落盘、
//! 不进 agent 状态机；事件流为 `Start → Delta* → Done`（Done 恰好一次，
//! 经 `BtwTracker::complete/abort` 抢删收口），每条会话同时只跑一条
//! （新旁问到达即 `Replaced` 旧的）。

use std::sync::Arc;

use dashmap::DashMap;
use futures::TryStreamExt;

use crate::agent::AgentState;
use crate::event::{
    AgentEvent, BtwEndReason, BtwEvent, ContentChunk, Envelope, Event, InternalEvent, ModelEvent,
};
use crate::provider::{ModelConfig, ModelStreamItem, Provider};
use crate::types::{BtwId, ContentBlock, Message, MessageId, Role, SessionId, ToolDefinition};

/// 包裹指令：作为末尾 user 消息追加在快照之后（前缀绝不动，保缓存）。
/// 工具定义随请求发送（逐字节一致是缓存命中的前提），这里把"别调"
/// 的理由说透——单 step 下 `tool_use` 不会被执行，调了也是白调。
const BTW_WRAPPER: &str = "\
[btw · 旁问，不进会话历史]
你正在回答一个关于当前会话的临时旁问。规则：
- 直接基于上面的会话上下文，用纯文本回答。**不要调用任何工具**——本请求只跑一轮，工具调用不会被执行，调了也是白调。
- 回答简短直接，默认不超过 10 行；使用提问所用的语言。
- 如果问题必须查新信息或动手操作才能回答，用一句话说明“这需要正式提问”并简述原因，不要硬答，也不要尝试调工具。
- 不要声称自己执行了任何操作。

问题：";

/// 旁问历史组装：同源重建的 system prompt 前置，历史里的旧 System
/// 剔除（与 `Agent::new` 的构造段同构——上下文里只保留一份 system）。
pub(crate) fn assemble_btw_history(
    system_prompt: String,
    history: &[Arc<Message>],
) -> Vec<Arc<Message>> {
    let mut messages = vec![Arc::new(Message::system(system_prompt))];
    messages.extend(history.iter().filter(|m| m.role != Role::System).cloned());
    messages
}

/// 构造旁问快照：与 provider 视角同口径（`sanitized_model_messages` 剔除
/// Internal、收编悬空 tool 批），尾部可带半截 assistant 文本，包裹指令
/// 永远作为最后一条 user 消息（前缀不动，保缓存）。快照即冻结——之后
/// 主 run 怎么推进（含 compact / rewind）都与本次旁问无关。
pub(crate) fn build_btw_snapshot(
    history: &[Arc<Message>],
    partial_text: Option<String>,
    question: &str,
) -> Vec<Arc<Message>> {
    let mut messages = crate::agent::MessageBuffer::sanitized_model_messages(history);
    if let Some(text) = partial_text {
        // 尾部本就是 assistant 时（/continue 触发的 turn）新起一条会产生
        // 相邻同角色——Anthropic 要求角色交替，会 400；并进尾部。
        match messages.last_mut() {
            Some(last) if last.role == Role::Assistant => {
                Arc::make_mut(last)
                    .content
                    .push(ContentBlock::Text { text });
            }
            _ => messages.push(Arc::new(Message::with_blocks(
                Role::Assistant,
                vec![ContentBlock::Text { text }],
            ))),
        }
    }
    messages.push(Arc::new(Message::with_blocks(
        Role::User,
        vec![ContentBlock::Text {
            text: format!("{BTW_WRAPPER}{question}"),
        }],
    )));
    messages
}

/// 跑一次旁问补全：转发文本增量（Delta），返回终态原因（Done 由
/// 调用方经 `BtwTracker::complete` 收口——与 abort 抢删句柄，恰好
/// 一次是结构保证）。只转发文本增量：thinking 属内部推理不转发；
/// `tool_use` 只记录不执行——纯 `tool_use` 无文本时返回 `ToolUse`，
/// 由客户端显示"这需要正式提问"的兜底文案。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_btw_completion(
    sink: Arc<dyn crate::comms::EventSink>,
    session_id: SessionId,
    request_id: BtwId,
    provider: Arc<dyn Provider>,
    messages: Vec<Arc<Message>>,
    tools: Vec<Arc<ToolDefinition>>,
    config: ModelConfig,
) -> BtwEndReason {
    let emit = |event: BtwEvent| {
        sink.emit(Envelope::new(session_id.clone(), Event::Btw(event)));
    };

    let mut stream = match provider.stream(&messages, &tools, &config).await {
        Ok(stream) => stream,
        Err(e) => {
            return BtwEndReason::Error(e.to_string());
        }
    };

    let mut saw_text = false;
    let mut saw_tool_use = false;
    let error = loop {
        match stream.try_next().await {
            Ok(Some(ModelStreamItem::Chunk(ContentChunk::Text(text)))) => {
                saw_text = true;
                emit(BtwEvent::Delta {
                    request_id: request_id.clone(),
                    text,
                });
            }
            Ok(Some(ModelStreamItem::ToolCall(_) | ModelStreamItem::ToolCallDelta { .. })) => {
                saw_tool_use = true;
            }
            Ok(Some(ModelStreamItem::Complete) | None) => break None,
            // thinking / fallback / usage / response meta 与旁问无关。
            Ok(Some(_)) => {}
            Err(e) => break Some(BtwEndReason::Error(e.to_string())),
        }
    };
    error.unwrap_or({
        if saw_tool_use && !saw_text {
            BtwEndReason::ToolUse
        } else {
            BtwEndReason::Stop
        }
    })
}

/// conductor 持有的旁问跟踪器：每会话一条在飞句柄 + 事件流半截文本镜像。
/// 镜像由 conductor 主循环既有的事件订阅臂逐条喂入（`observe`），无独立
/// task、无额外订阅。
pub(crate) struct BtwTracker {
    handles: DashMap<SessionId, (BtwId, tokio::task::AbortHandle)>,
    /// 当前在飞 assistant 消息的半截文本（按 `message_id` 归账）。
    partials: DashMap<SessionId, (MessageId, String)>,
    /// start 全程串行化（conductor 的 `handle_input` 按输入并发 spawn，
    /// 无锁时两条连发旁问的 abort/register 会交错出孤儿流）。
    start_locks: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    /// /stop・shutdown 代际：cancel 臂与 start 持同一把 per-session 锁
    /// 串行，代际在锁内不变；start 完成 prepare 后发现代际已变 =
    /// cancel 落在 prepare 窗口——放弃 spawn 补 `Done{Cancelled}`
    /// （否则旁问会在 /stop 之后才开跑）。
    cancel_gens: DashMap<SessionId, u64>,
    event_bus: Arc<crate::comms::EventBus>,
}

impl BtwTracker {
    pub(crate) fn new(event_bus: Arc<crate::comms::EventBus>) -> Self {
        Self {
            handles: DashMap::new(),
            partials: DashMap::new(),
            start_locks: DashMap::new(),
            cancel_gens: DashMap::new(),
            event_bus,
        }
    }

    /// start 的 per-session 串行闸。
    pub(crate) async fn lock(&self, sid: &SessionId) -> tokio::sync::OwnedMutexGuard<()> {
        self.start_locks
            .entry(sid.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await
    }

    /// 事件镜像：conductor 主循环每个 envelope 喂一次。只跟踪三种事件，
    /// 其余一律忽略。
    pub(crate) fn observe(&self, sid: &SessionId, event: &Event) {
        match event {
            Event::Model(ModelEvent::Chunk {
                message_id,
                content: ContentChunk::Text(text),
            }) => {
                let mut entry = self
                    .partials
                    .entry(sid.clone())
                    .or_insert_with(|| (message_id.clone(), String::new()));
                if entry.0 != *message_id {
                    *entry = (message_id.clone(), String::new());
                }
                entry.1.push_str(text);
            }
            // 完整 assistant 消息落盘 = 该 message 的半截使命结束——
            // 去重窗口在事件源关闭（快照不会把同一文本拼两次）。
            Event::Internal(InternalEvent::MessageAdded { message })
                if message.role == Role::Assistant
                    && self.partials.get(sid).is_some_and(|p| p.0 == message.id) =>
            {
                self.partials.remove(sid);
            }
            // turn 彻底结束（含被掐断、半截从未落盘的病态路径）——
            // 残留半截不带给下一条旁问。
            Event::Agent(AgentEvent::StateChanged { state }) if *state == AgentState::Idle => {
                self.partials.remove(sid);
            }
            _ => {}
        }
    }

    /// 当前半截文本（空/纯空白视为无）。
    pub(crate) fn partial_of(&self, sid: &SessionId) -> Option<String> {
        self.partials.get(sid).and_then(|p| {
            let text = p.1.trim();
            (!text.is_empty()).then(|| p.1.clone())
        })
    }

    /// 当前 cancel 代际（须在 start 锁内读取；cancel 臂的 bump 经
    /// 同一把锁串行，构成 happens-before）。
    pub(crate) fn cancel_gen(&self, sid: &SessionId) -> u64 {
        self.cancel_gens.get(sid).map_or(0, |g| *g)
    }

    /// /stop・shutdown 臂调用（须持 start 锁）：代际 +1，随后 abort。
    pub(crate) fn bump_cancel_gen(&self, sid: &SessionId) {
        *self.cancel_gens.entry(sid.clone()).or_insert(0) += 1;
    }

    /// 旁路流自然结束（runner task 出口调用）：与 abort 抢删句柄——
    /// 双方的句柄删除都是 DashMap 单条原子操作，一条句柄只会被一
    /// 个路径删掉，Done 恰好一次是结构保证而非时序运气。
    pub(crate) fn complete(&self, sid: &SessionId, request_id: &BtwId, reason: BtwEndReason) {
        if self
            .handles
            .remove_if(sid, |_, (rid, _)| rid == request_id)
            .is_some()
        {
            self.emit(
                sid,
                BtwEvent::Done {
                    request_id: request_id.clone(),
                    reason,
                },
            );
        }
    }

    /// 终止在飞旁问（若有）：抢到句柄即掐断并发终态；自然结束的流
    /// 已被 `complete` 抢先删句柄，这里落空——Done 恰好一次。
    pub(crate) fn abort(&self, sid: &SessionId, reason: BtwEndReason) {
        if let Some((_, (request_id, handle))) = self.handles.remove(sid) {
            handle.abort();
            self.emit(sid, BtwEvent::Done { request_id, reason });
        }
    }

    /// daemon 关停：全部在飞旁问统一补终态掐断。
    pub(crate) fn abort_all(&self, reason: &BtwEndReason) {
        let sids: Vec<SessionId> = self.handles.iter().map(|e| e.key().clone()).collect();
        for sid in sids {
            self.abort(&sid, reason.clone());
        }
    }

    /// 登记在飞句柄（调用方已持 start 锁、已 abort 旧流）。
    pub(crate) fn register(
        &self,
        sid: SessionId,
        request_id: BtwId,
        handle: tokio::task::AbortHandle,
    ) {
        self.handles.insert(sid, (request_id, handle));
    }

    pub(crate) fn emit(&self, sid: &SessionId, event: BtwEvent) {
        use crate::comms::EventSink;
        self.event_bus
            .handle(sid.clone())
            .emit(Envelope::new(sid.clone(), Event::Btw(event)));
    }

    /// `cleanup_interval` 用：清已完成句柄、无归属锁（`try_lock` 失败
    /// = start 进行中，保留）与残留 partial。`cancel_gens` 不清理：
    /// per-session 一个 u64 内存可忽略，而清理与锁外快照读取的竞态
    /// （误清 → 快照失配 → 误取消）代价比留着大（R4 复审）。
    pub(crate) fn retain(&self, keep: impl Fn(&SessionId) -> bool) {
        self.handles.retain(|_, (_, h)| !h.is_finished());
        self.start_locks.retain(|sid, lock| {
            self.handles.contains_key(sid) || keep(sid) || lock.try_lock().is_err()
        });
        self.partials.retain(|sid, _| keep(sid));
    }
}

#[cfg(test)]
#[path = "btw_test.rs"]
mod tests;
