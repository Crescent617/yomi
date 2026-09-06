use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::agent::AgentShared;
use crate::agent::{Agent, AgentConfig, AgentInput, AgentSpawnArgs, AgentState};
use crate::comms::{EventBus, InputBus, InputBusSubscriber, Mailbox};
use crate::event::{AgentEvent, AgentStatus, Event, InternalEvent, StopReason};
use crate::kernel::persist_pool;
use crate::notification::{AgentActivity, Notification, NotificationBus};
use crate::types::SessionId;

/// 唯一管理 Agent 生命周期的地方。
/// `InputBus` 的唯一消费者，负责 Mailbox 管理、Agent lazy spawn、Cancel 分发。
pub struct Conductor {
    agent_shared: Arc<AgentShared>,
    agent_config: AgentConfig,
    active: DashMap<SessionId, ActiveAgent>,
    mailboxes: DashMap<SessionId, Arc<Mailbox>>,
    rx: std::sync::Mutex<Option<InputBusSubscriber>>,
    event_bus: Arc<EventBus>,
    input_bus: Arc<InputBus>,
    base_prompt: String,
    data_dir: std::path::PathBuf,
    /// Per-session spawn lock to prevent duplicate agent creation races.
    spawn_locks: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    notification_bus: Arc<NotificationBus>,
}

pub struct ActiveSessionSnapshot {
    pub session_id: SessionId,
    pub state: AgentState,
}

struct ActiveAgent {
    handle: JoinHandle<()>,
    cancel_token: crate::agent::CancelToken,
    state: Mutex<AgentState>,
    permission_state: Option<crate::permission::PermissionState>,
}

impl Conductor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_shared: Arc<AgentShared>,
        agent_config: AgentConfig,
        rx: InputBusSubscriber,
        event_bus: Arc<EventBus>,
        input_bus: Arc<InputBus>,
        base_prompt: String,
        data_dir: std::path::PathBuf,
        notification_bus: Arc<NotificationBus>,
    ) -> Self {
        Self {
            agent_shared,
            agent_config,
            active: DashMap::new(),
            mailboxes: DashMap::new(),
            rx: std::sync::Mutex::new(Some(rx)),
            event_bus,
            input_bus,
            base_prompt,
            data_dir,
            spawn_locks: DashMap::new(),
            notification_bus,
        }
    }

    pub async fn run(self: Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let mut rx = self
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .expect("Conductor::run already called");
        let mut subscriber = self.event_bus.subscribe_all();
        let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_mins(10));

        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    tracing::info!("Conductor shutting down, cancelling all active agents");
                    for agent in &self.active {
                        agent.cancel_token.cancel_for_shutdown();
                    }
                    break;
                }
                Some((sid, input)) = rx.recv() => {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        this.handle_input(sid, input).await;
                    });
                }
                Some((sid, envelope)) = subscriber.recv() => {
                    let event_id = envelope.event_id.to_string();
                    if let Event::Agent(event) = &envelope.event {
                        if let Some(activity) = pet_activity(event) {
                            let _ = self.notification_bus.send(Notification::AgentActivity {
                                session_id: sid.clone(),
                                event_id,
                                activity,
                            });
                        }
                    }
                    match envelope.event {
                        Event::Agent(AgentEvent::MailboxChanged { steer, queued }) => {
                            let _ = self.notification_bus.send(Notification::MailboxChanged {
                                session_id: sid.clone(),
                                steer,
                                queued,
                            });
                        }
                        Event::Agent(AgentEvent::StateChanged { state }) => {
                            if let Some(agent) = self.active.get(&sid) {
                                *agent.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
                            }
                            let _ = self.notification_bus.send(Notification::StateChanged {
                                session_id: sid.clone(),
                                status: state,
                            });
                        }
                        Event::Internal(InternalEvent::MessageAdded { message }) => {
                            // 落盘走持久化池（2026-08-22 洪峰丢回复根
                            // 治）：dispatch 同步入队即返，慢 IO 写不
                            // 再堵事件循环（旧 inline append 在 91 会
                            // 话洪峰下把 256 深 bus 队列打爆丢件）。顺
                            // 序不变式 = 单循环顺序 dispatch + 池 per-
                            // key FIFO + `Stopped` 臂 wait_idle。
                            if let Some(ref pool) = self.agent_shared.persist_pool {
                                if message.role != crate::types::Role::System {
                                    pool.dispatch(
                                        &sid,
                                        persist_pool::PersistJob::Append((*message).clone()),
                                    );
                                }
                            }
                        }
                        Event::Internal(InternalEvent::MessageReplaced { messages }) => {
                            if let Some(ref pool) = self.agent_shared.persist_pool {
                                let to_persist: Vec<crate::types::Message> = messages
                                    .iter()
                                    .map(|m| (**m).clone())
                                    .filter(|m| m.role != crate::types::Role::System)
                                    .collect();
                                pool.dispatch(&sid, persist_pool::PersistJob::Replace(to_persist));
                            }
                            let _ = self.event_bus.publish(
                                sid.clone(),
                                crate::event::Envelope::new(
                                    sid.clone(),
                                    crate::event::Event::Agent(crate::event::AgentEvent::MessageReplaced { session_id: sid.clone() }),
                                ),
                            );
                        }
                        Event::Tool(crate::event::ToolEvent::Metadata { message_id, tool_id, metadata }) => {
                            if let Some(ref pool) = self.agent_shared.persist_pool {
                                let placeholder = Arc::new(crate::types::Message {
                                    id: message_id.clone(),
                                    role: crate::types::Role::Internal,
                                    content: vec![],
                                    tool_call_id: Some(tool_id),
                                    metadata: Some(metadata),
                                    ..Default::default()
                                });
                                // Persist tool metadata through the same MessageAdded path as
                                // other messages. Internal metadata is not a conversation
                                // boundary, so the server keeps its replay buffer intact.
                                let envelope = crate::event::Envelope::new(
                                    sid.clone(),
                                    crate::event::Event::Internal(
                                        crate::event::InternalEvent::MessageAdded {
                                            message: Arc::clone(&placeholder),
                                        },
                                    ),
                                );
                                if let Err(e) = self.event_bus.publish(sid.clone(), envelope) {
                                    tracing::warn!("Failed to publish metadata MessageAdded for session={sid}: {e}, falling back to direct persist");
                                    pool.dispatch(
                                        &sid,
                                        persist_pool::PersistJob::Append((*placeholder).clone()),
                                    );
                                }
                            }
                        }
                        Event::Agent(AgentEvent::Lifecycle {
                            state: AgentStatus::Stopped { reason },
                        }) => {
                            // 转运/收尾前显式排空该 session 的落盘队
                            // 列——"最终答案必落盘"从旧的循环时序隐含
                            // 保证变成断言式保证（上界与降级见
                            // `wait_drained`）。阻塞上界 30s/次的权
                            // 衡（fresh-eyes 复审）：store 病态 + 连
                            // 续 Stopped 会队头阻塞——宁慢不丢（病态
                            // 下 warn 可见；比慢盘丢件更可控）。
                            if let Some(ref pool) = self.agent_shared.persist_pool {
                                persist_pool::wait_drained(pool, &sid, "orphan forwarding")
                                    .await;
                            }
                            self.maybe_forward_orphan(&sid, reason);
                        }
                        _ => {}
                    }
                }
                _ = cleanup_interval.tick() => {
                    self.active.retain(|_sid, agent| !agent.handle.is_finished());
                    // 清理没有活跃 agent 且 mailbox 为空的 session，防止内存泄漏
                    self.mailboxes.retain(|sid, mb| {
                        self.active.contains_key(sid) || !mb.is_empty()
                    });
                    // 清理已完成 spawn 或不会再被唤醒的 session 的锁。
                    // 保留既没有活跃 agent 但 mailbox 仍在的 session（未来还会 spawn）。
                    self.spawn_locks.retain(|sid, _| {
                        !self.active.contains_key(sid) && self.mailboxes.contains_key(sid)
                    });
                }
                else => break,
            }
        }
    }

    pub fn get_state(&self, sid: &SessionId) -> Option<AgentState> {
        self.active
            .get(sid)
            .map(|a| *a.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Whether the session's agent task is live and in an active (non-idle)
    /// run. Used by the channel observability watchdog: a long tool call
    /// keeps the agent in `ExecutingTool` (alive), so liveness — unlike an
    /// event-gap timeout — never false-positives on slow tools.
    pub fn is_running(&self, sid: &SessionId) -> bool {
        self.active.get(sid).is_some_and(|agent| {
            !agent.handle.is_finished()
                && *agent.state.lock().unwrap_or_else(|e| e.into_inner()) != AgentState::Idle
        })
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// IDs of sessions with a live in-memory agent, in any state (including
    /// idle agents within their unload grace period). Auto gc excludes these
    /// so a live agent never has its session data pulled out from under it —
    /// a surviving agent would recreate data files the next orphan sweep
    /// would then delete.
    pub fn loaded_session_ids(&self) -> Vec<SessionId> {
        self.active
            .iter()
            .map(|agent| agent.key().clone())
            .collect()
    }

    /// Snapshot sessions whose agent task is still live and not idle.
    pub fn running_sessions(&self) -> Vec<ActiveSessionSnapshot> {
        self.active
            .iter()
            .filter_map(|agent| {
                if agent.handle.is_finished() {
                    return None;
                }
                let state = *agent.state.lock().unwrap_or_else(|e| e.into_inner());
                (state != AgentState::Idle).then(|| ActiveSessionSnapshot {
                    session_id: agent.key().clone(),
                    state,
                })
            })
            .collect()
    }

    /// Update the permission level for a live session (real-time).
    /// Returns `true` if the session is currently active and the level was updated.
    pub fn set_permission_level(&self, sid: &SessionId, level: crate::permission::Level) -> bool {
        if let Some(agent) = self.active.get(sid) {
            if let Some(ref ps) = agent.permission_state {
                tokio::spawn({
                    let ps = ps.clone();
                    let sid = sid.clone();
                    async move {
                        ps.set_auto_approve_level(level).await;
                        tracing::info!("Permission level updated in-memory for {}", sid.0);
                    }
                });
                return true;
            }
        }
        false
    }

    async fn handle_input(&self, sid: SessionId, input: AgentInput) {
        match input {
            input @ (AgentInput::Cancel | AgentInput::Shutdown) => {
                let mailbox = self.mailboxes.get(&sid).map(|mb| Arc::clone(&mb));
                if let Some(agent) = self.active.get(&sid) {
                    // Shutdown（kernel 关停）与 /stop 同路径，但取消令牌
                    // 带 shutdown 归因——终态与上下文标记可区分两者。
                    if matches!(input, AgentInput::Shutdown) {
                        agent.cancel_token.cancel_for_shutdown();
                    } else {
                        agent.cancel_token.cancel();
                    }
                }
                if let Some(ref mb) = mailbox {
                    mb.clear().await;
                    self.emit_mailbox_changed(&sid, mb).await;
                }
                // A cancelled agent exits its loop instead of resetting. Wait
                // for the task to finish; if new input arrived while it was
                // winding down (its own wake_agent saw the entry above and
                // bailed), respawn so that input is not stranded in the
                // mailbox until yet another message arrives.
                if let Some((_, agent)) = self.active.remove(&sid) {
                    if tokio::time::timeout(std::time::Duration::from_secs(5), agent.handle)
                        .await
                        .is_err()
                    {
                        // Stuck task (e.g. a tool ignoring cancellation). It
                        // never touches the mailbox again — a cancelled agent
                        // always breaks at the Idle check — so a later spawn
                        // cannot race it for input.
                        tracing::warn!(
                            "cancel: agent for session={} did not exit within 5s; detaching",
                            sid.0
                        );
                    }
                    if let Some(mb) = mailbox {
                        if !mb.is_empty() {
                            self.wake_agent(&sid, mb).await;
                        }
                    }
                }
            }
            // PermissionResponse and AskUserResponse are consumed directly by
            // Checker / AskUserTool via input_bus subscription; do not queue them.
            AgentInput::PermissionResponse { .. } | AgentInput::AskUserResponse { .. } => {}
            input => {
                // 图片落盘 + 绝对路径标注（当前轮进模型即可 Read/文件
                // 操作；历史读回经 inline_assets_in_message 有同款标注）。
                let input = match input {
                    AgentInput::User { content } => AgentInput::User {
                        content: crate::utils::asset::process_image_blocks(content, &self.data_dir)
                            .await,
                    },
                    AgentInput::Steer(content) => AgentInput::Steer(
                        crate::utils::asset::process_image_blocks(content, &self.data_dir).await,
                    ),
                    other => other,
                };
                let mb = self
                    .mailboxes
                    .entry(sid.clone())
                    .or_insert_with(|| Arc::new(Mailbox::new()))
                    .clone();

                Self::push_to_mailbox(&mb, input).await;
                self.emit_mailbox_changed(&sid, &mb).await;
                // User activity (message/steer/queue): refresh the session's
                // recency so session lists order by real use, not creation.
                if let Some(store) = &self.agent_shared.session_store {
                    if let Err(e) = store.touch(&sid).await {
                        tracing::warn!("failed to touch session={sid}: {e}");
                    }
                }
                self.wake_agent(&sid, mb).await;
            }
        }
    }

    async fn push_to_mailbox(mailbox: &Mailbox, input: AgentInput) {
        match input {
            AgentInput::Steer(content) => mailbox.push_steer(content).await,
            other => mailbox.push(other).await,
        }
    }

    /// Mailbox access for the management surface (snapshot / remove /
    /// clear). `None` = no mailbox (nothing pending).
    pub fn mailbox(&self, sid: &SessionId) -> Option<Arc<Mailbox>> {
        self.mailboxes.get(sid).map(|mb| Arc::clone(&mb))
    }

    /// Emit the mailbox-change event frontends refresh on.
    pub(crate) async fn emit_mailbox_changed(&self, sid: &SessionId, mailbox: &Mailbox) {
        let (steer, queued) = mailbox.lens().await;
        let _ = self
            .event_bus
            .handle(sid.clone())
            .try_send(crate::event::Envelope::new(
                sid.clone(),
                Event::Agent(AgentEvent::MailboxChanged { steer, queued }),
            ));
    }

    /// subagent 无主完成的回信转运入口（详见 `forward_orphan_reply`）：
    /// claim 命中的归既有 sync/async 路径。claim 消费保持 inline（与
    /// 事件处理同点原子）；转运体挪出循环——jsonl 整读+两次 store
    /// 读不挡其他 session 的事件分发（复审 should-fix）。落盘完整性
    /// 已由调用点的 `wait_idle` 显式排空保证，spawn 不破坏正确性。
    fn maybe_forward_orphan(self: &Arc<Self>, sid: &SessionId, reason: StopReason) {
        if !self.should_forward_orphan(sid) {
            return;
        }
        let this = Arc::clone(self);
        let sid = sid.clone();
        tokio::spawn(async move {
            this.forward_orphan_reply(&sid, &reason).await;
        });
    }

    /// 该 `Stopped` 是否是"无主"的 subagent 完成（claim 原子消费判
    /// 定）：`run_subagent` 声明过的归既有回信路径（sync `ToolOutput`
    /// / `async` 完成 steer），跳过；未声明的（post-completion
    /// follow-up、daemon 重启恢复）由 conductor 转运。claim 完整语
    /// 义见 `AgentShared::subagent_claims`。
    fn should_forward_orphan(&self, sid: &SessionId) -> bool {
        sid.starts_with(crate::types::SUB_PREFIX)
            && self.agent_shared.subagent_claims.remove(sid).is_none()
    }

    /// subagent 无主完成的回信转运：把最终答案以 `[From Agent: ...]`
    /// 格式 steer 给 parent（2026-08-21 `post_message` follow-up 回信
    /// 蒸发事故的根治——彼时回信路径都在首次完成后退出，跟进答案
    /// 蒸发在 subagent 自己的 transcript 里）。
    ///
    /// **无落盘竞态**：落盘走持久化池（per-session FIFO），`Stopped`
    /// 臂在调用本函数前先 `wait_idle` 显式排空该 session 的落盘队
    /// 列——agent 先发 `MessageAdded`（最终答案）再发 `Stopped`，
    /// 处理到 `Stopped` 时最终答案必然已落盘。这就是本函数可以直接
    /// 读 store 而不需要任何轮询/基线的原因（断言式保证，不靠时序
    /// 猜）。残余缺口（既有持久化路径同型假设，非本函数引入）：bus
    /// 主通道打满时 `EventSink` 的 `try_send().ok()` 会静默丢
    /// `MessageAdded`/`Stopped`，此时可能读到上一轮旧答案。
    async fn forward_orphan_reply(&self, sid: &SessionId, reason: &StopReason) {
        // Cancelled 多半是 parent 自己 /stop 的；Shutdown 是 daemon
        // 关停打断——半成品答案都不转运。
        if matches!(reason, StopReason::Cancelled { .. } | StopReason::Shutdown) {
            return;
        }
        let (Some(messages), Some(sessions)) = (
            &self.agent_shared.message_store,
            &self.agent_shared.session_store,
        ) else {
            return;
        };
        let reply = messages
            .get(&sid.0)
            .await
            .ok()
            .and_then(|msgs| {
                msgs.iter()
                    .rev()
                    .find(|m| m.role == crate::types::Role::Assistant)
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
            })
            .unwrap_or_default();
        // 无可转运文本（Failed 且无残留、纯 thinking 终答案等病态边
        // 缘）——留 warn。
        if reply.is_empty() {
            tracing::warn!(
                session_id = %sid.0,
                "orphan subagent completion has no text reply to forward"
            );
            return;
        }
        let parent = sessions
            .get(sid)
            .await
            .ok()
            .flatten()
            .and_then(|info| info.parent_id);
        let Some(parent) = parent else {
            tracing::warn!(
                session_id = %sid.0,
                "orphan subagent reply has nowhere to go (no parent)"
            );
            return;
        };
        let body = match reason {
            StopReason::Completed { .. } => reply,
            StopReason::Failed { error } => format!("⚠ run failed: {error}\n{reply}"),
            StopReason::MaxIterations { reached } => {
                format!("⚠ run hit max iterations ({reached})\n{reply}")
            }
            StopReason::Cancelled { .. } | StopReason::Shutdown => {
                unreachable!("cancelled/shutdown filtered above")
            }
        };
        let steer =
            crate::tools::format_agent_message(&sid.0, format_args!("Follow-up reply\n{body}"));
        if let Err(error) = self.input_bus.publish(
            parent,
            AgentInput::Steer(vec![crate::types::ContentBlock::Text { text: steer }]),
        ) {
            tracing::warn!("failed to forward orphan subagent reply: {error}");
        }
    }

    /// Check whether the session or any ancestor is routed from an external
    /// channel (i.e. has no interactive UI attached).
    ///
    /// Subagent sessions don't carry their own channel mapping, so the parent
    /// chain is walked to find a channel-routed ancestor.
    async fn is_channel_routed(
        &self,
        sid: &SessionId,
        session_info: Option<&crate::storage::SessionInfo>,
    ) -> bool {
        let Some(hub) = &self.agent_shared.channel_hub else {
            return false;
        };
        if hub.is_channel_session(sid).await {
            return true;
        }
        let Some(store) = &self.agent_shared.session_store else {
            return false;
        };
        let mut cursor = session_info.and_then(|i| i.parent_id.clone());
        while let Some(pid) = cursor {
            if hub.is_channel_session(&pid).await {
                return true;
            }
            cursor = match store.get(&pid).await {
                Ok(Some(info)) => info.parent_id,
                _ => None,
            };
        }
        false
    }

    async fn wake_agent(&self, sid: &SessionId, mailbox: Arc<Mailbox>) {
        let lock = self
            .spawn_locks
            .entry(sid.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .downgrade()
            .clone();
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                // Another task is already spawning for this session.
                // Messages have already been pushed to the mailbox; once that
                // spawn completes and the agent starts, it will consume them.
                return;
            }
        };

        if self
            .active
            .get(sid)
            .is_some_and(|v| !v.handle.is_finished())
        {
            return;
        }

        let history = match &self.agent_shared.message_store {
            Some(store) => match store.get_inlined(&sid.0).await {
                Ok(msgs) => msgs.into_iter().map(Arc::new).collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };

        let session_info = match self.agent_shared.session_store.as_ref() {
            Some(s) => s.get(sid).await.ok().flatten(),
            None => None,
        };

        let cancel_token = session_info
            .as_ref()
            .and_then(|i| {
                i.parent_id
                    .clone()
                    .and_then(|p| self.active.get(&p))
                    .map(|a| a.cancel_token.child_token())
            })
            .unwrap_or_else(crate::agent::CancelToken::new);

        // Resolve working directory from session info or fallback to data_dir/workspace
        let working_dir = session_info
            .as_ref()
            .and_then(|i| i.working_dir.clone())
            .map(std::path::PathBuf::from);

        let cwd = Some(crate::utils::path::session_workspace_dir(
            &self.data_dir,
            working_dir,
        ));

        // Create file state store
        let file_state_store = match Self::create_file_state_store(&sid.0, &self.data_dir).await {
            Ok(store) => store,
            Err(e) => {
                tracing::error!("Failed to create file state store: {}", e);
                Arc::new(crate::tools::helper::FileStateStore::new())
            }
        };

        // Create permission state from session's auto_approve_level
        let auto_approve_level = session_info
            .as_ref()
            .and_then(|i| i.auto_approve_level.as_ref())
            .and_then(|s| s.parse::<crate::permission::Level>().ok())
            .unwrap_or_default();
        let permission_state = Some(crate::permission::PermissionState::new(auto_approve_level));

        // Resolve workspace skill directory
        let workspace_skill_dir = match cwd.as_ref() {
            Some(dir) => crate::skill::workspace_skill_dir(dir).await,
            None => None,
        };

        // 磁盘即真相：spawn 现场分层扫描（同目录并发单飞），工作区目录优先级最高
        let skill_folders = crate::skill::session_skill_folders(
            &self.agent_shared.skill_folders,
            workspace_skill_dir.clone(),
        );
        let skills = self.agent_shared.skill_loader.load(skill_folders).await;

        // Clone AgentShared so we can mutate skill_folders per-session
        // （追加 workspace 供 skill_load 工具按名解析；与 loader 同一目录
        // 列表（重复留最后），工具 rev 扫描与分层合并同胜者）
        let mut base_clone: AgentShared = (*self.agent_shared).clone();
        base_clone.skill_folders = crate::skill::session_skill_folders(
            &self.agent_shared.skill_folders,
            workspace_skill_dir.clone(),
        );
        let checkpoint_store = base_clone.checkpoint_store.clone();
        let shared = Arc::new(base_clone.with_per_session(
            permission_state,
            Some(Arc::clone(&file_state_store)),
            checkpoint_store,
        ));

        let working_dir = cwd.unwrap_or_default();

        let is_sub_agent = sid.starts_with(crate::types::SUB_PREFIX);

        // ask_user needs an interactive surface to answer on: sub-agents
        // never talk to the user directly (they report to the parent), and
        // channel sessions get the tool only when their channel renders
        // the question card (Feishu); on plain platforms it would time out
        // after 2 minutes, so it stays blocked there.
        // NB (2026-08): ask_user 已整体下线（tools/mod.rs 不再注册），
        // 本 heuristic 随之惰性（blocklist 匹配不到任何已注册工具），
        // 保留仅以便工具回归时恢复。
        // Channel routing shapes three decisions below (watch contract,
        // chat-scoped rules, ask_user blocklist) — resolve it once.
        // Subagent sessions carry no mapping of their own; their rules
        // scope stays None (a sub-agent is a tool, not a chat voice).
        let routing = match &self.agent_shared.channel_hub {
            Some(hub) if !is_sub_agent => hub
                .store()
                .find_routing_by_session(sid)
                .await
                .ok()
                .flatten(),
            _ => None,
        };
        // Watch observers (a watched chat's mirror session, `/watch`)
        // learn their contract from the routing row's kind.
        let watch_routing = routing.as_ref().filter(|r| r.is_watch());
        // The chat id rides every routing row (thread rows denormalize
        // it into actual_chat_id), so the chat-scoped rules file needs
        // no thread→chat lookup — and no chat session to exist.
        let rules_chat = routing.as_ref().map(|r| r.external_chat_id.as_str());
        let channel_routed = self.is_channel_routed(sid, session_info.as_ref()).await;
        let ask_card_capable = match &self.agent_shared.channel_hub {
            Some(hub) if !is_sub_agent => hub.session_channel_supports_cards(sid).await,
            _ => false,
        };
        let mut tool_blocklist = self.agent_config.tool_blocklist.clone();
        if !tool_blocklist
            .iter()
            .any(|p| p == crate::tools::ask_user::ASK_USER_TOOL_NAME)
            && (is_sub_agent || (channel_routed && !ask_card_capable))
        {
            tool_blocklist.push(crate::tools::ask_user::ASK_USER_TOOL_NAME.to_string());
        }

        // 模板化 subagent：session 记录里的模板名在 spawn 时实时 resolve，
        // base prompt 换成角色定义（model/skills/工具集全继承父 agent）。
        let template = if is_sub_agent {
            match session_info.as_ref().and_then(|i| i.template.as_deref()) {
                Some(name) => {
                    let agents_dir = crate::agent_tmpl::global_dir(&self.data_dir);
                    match crate::agent_tmpl::resolve(name, &agents_dir, Some(working_dir.as_path()))
                        .await
                    {
                        Some(t) if t.body.trim().is_empty() => {
                            tracing::warn!(
                                "template '{name}' has empty body; using default base prompt"
                            );
                            None
                        }
                        Some(t) => Some(t),
                        None => {
                            tracing::warn!(
                                "template '{name}' not found at spawn; using default base prompt"
                            );
                            None
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        };

        // Prompt assembly (capability contracts, watch-observer section,
        // channel rules) lives in one testable function — the
        // conductor only gathers the inputs. Sub-agents get no contract
        // sections: their output never leaves the parent. Note the
        // narrower predicate than the ask_user blocklist above, which
        // covers every sub-agent outright.
        let base_prompt = crate::prompt::compose_system_prompt(crate::prompt::SystemPromptParts {
            base_prompt: self.base_prompt.clone(),
            template_body: template.as_ref().map(|t| t.body.clone()),
            is_sub_agent,
            enable_attachments: self.agent_config.enable_attachments,
            channel_routed,
            watch: watch_routing.map(|routing| {
                (
                    routing.channel_name.as_str(),
                    routing.external_chat_id.as_str(),
                )
            }),
            rules_chat,
            // Session rules are the only rules layer local/GUI sessions
            // have; sub-agents get none (a sub-agent is a tool, not a
            // chat voice).
            rules_session: if is_sub_agent {
                None
            } else {
                Some(sid.0.as_str())
            },
            data_dir: &self.data_dir,
        })
        .await;

        // Resolve tool flags here — session-level policy lives in the
        // conductor, not the agent: sub-agent sessions must not spawn
        // further sub-agents, nor manage cron jobs.
        let tool_flags =
            crate::tools::ToolFlags::new(self.agent_config.enable_subagent && !is_sub_agent)
                .with_cron(self.agent_config.enable_cron_tool && !is_sub_agent)
                .with_todo(self.agent_config.enable_todo_tool);

        // tools/ 目录外挂（spawn 时扫描快照）：代理工具的收口与内建一致
        // （Agent::new 合并处做 blocklist 与撞名让位）。
        let ext_tools = crate::tools::ext::scan(&self.data_dir).await;

        let args = AgentSpawnArgs::new(base_prompt, sid.0.clone(), mailbox, working_dir)
            .with_skills(skills)
            .with_arc_history(history)
            .with_max_iterations(self.agent_config.max_iterations)
            .with_tool_flags(tool_flags)
            .with_file_state_store(Arc::clone(&file_state_store))
            .with_tool_blocklist(tool_blocklist)
            .with_max_tool_output_length(self.agent_config.max_tool_output_length)
            .with_cancel_token(cancel_token.clone())
            .with_input_bus(self.input_bus.clone())
            .with_ext_tools(ext_tools);

        let agent = Agent::new(&shared, args).await;

        let session_id = sid.0.clone();
        let loop_span = tracing::info_span!("agent_loop", session_id = %session_id);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(
            async move {
                if start_rx.await.is_err() {
                    return;
                }
                tracing::info!("agent loop started");
                let _ = agent.start_loop().await;
                tracing::info!("agent loop ended");
            }
            .instrument(loop_span),
        );

        self.active.insert(
            sid.to_owned(),
            ActiveAgent {
                handle,
                cancel_token,
                state: Mutex::new(AgentState::Idle),
                permission_state: shared.permission_state.clone(),
            },
        );
        let _ = start_tx.send(());
    }

    /// Create and populate the file state store for this session
    async fn create_file_state_store(
        session_id: &str,
        data_dir: &std::path::Path,
    ) -> crate::types::Result<Arc<crate::tools::helper::FileStateStore>> {
        let jsonl_store =
            crate::storage::file_state::JsonlFileStateStore::new(session_id, data_dir);
        jsonl_store.maybe_vacuum().await?;
        let persistent_store: Arc<dyn crate::storage::FileStateStore> = Arc::new(jsonl_store);

        let states = persistent_store.read_all().await?;

        let file_state_store = crate::tools::helper::FileStateStore::new()
            .with_persistent(persistent_store)
            .with_states(states.into_iter().map(|fs| (fs.path, fs.mtime)));

        Ok(Arc::new(file_state_store))
    }
}

fn pet_activity(event: &AgentEvent) -> Option<AgentActivity> {
    match event {
        AgentEvent::PermissionRequest {
            req_id, session_id, ..
        } => Some(AgentActivity::PermissionRequested {
            req_id: req_id.clone(),
            target_session_id: session_id.clone(),
        }),
        AgentEvent::AskUserQuestion {
            req_id, session_id, ..
        } => Some(AgentActivity::AskUserRequested {
            req_id: req_id.clone(),
            target_session_id: session_id.clone(),
        }),
        AgentEvent::PermissionAck { req_id } | AgentEvent::AskUserAck { req_id } => {
            Some(AgentActivity::RequestResolved {
                req_id: req_id.clone(),
            })
        }
        AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        } => Some(AgentActivity::Started),
        AgentEvent::Lifecycle {
            state: AgentStatus::Stopped { reason },
        } => Some(AgentActivity::Stopped {
            reason: reason.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "conductor_test.rs"]
mod tests;
