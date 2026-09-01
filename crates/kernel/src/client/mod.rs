//! Unified client API for both local (in-process) and remote (IPC) kernels:
//! the [`KernelApi`] trait, its shared response types, and the two
//! implementations —
//!
//! - [`local`]: `impl KernelApi for Kernel` (in-process).
//! - [`remote`]: [`RemoteKernel`], an IPC client proxy with lazy connect.

mod local;
mod remote;

pub use remote::RemoteKernel;

/// Error message returned by [`KernelApi::restart`] when the daemon came
/// back but the saved config could not be applied. Callers (e.g. the CLI)
/// match on it to tell "restart succeeded but config is broken" apart from
/// a genuinely failed restart — only the latter may fall back to killing.
pub const RESTART_CONFIG_NOT_APPLIED: &str =
    "daemon restarted but the saved config could not be applied";

use crate::checkpoint::RewindTarget;
use crate::kernel::CreateSessionInput;
use crate::notification::Notification;
use crate::permission::Level;
use crate::types::{ContentBlock, KernelError, MessageId, Project, ProjectId, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaginatedSessions {
    pub sessions: Vec<crate::storage::session::SessionInfo>,
    pub next_cursor: Option<String>,
}

/// Raw, unformatted byte range from a session JSONL file.
pub type SessionJsonlChunk = crate::utils::file_chunk::FileChunk;

/// Read `source` in full by looping chunked [`KernelApi::read_file`] calls.
///
/// Fails before transferring anything when the daemon reports a file size
/// above `max_bytes`. Returns the raw bytes and the daemon-guessed mime.
pub async fn read_file_bytes(
    api: &dyn KernelApi,
    source: crate::utils::file_read::FileSource,
    max_bytes: u64,
) -> Result<(Vec<u8>, String)> {
    use base64::Engine as _;

    let mut offset = 0u64;
    let mut mime = String::new();
    let mut bytes = Vec::new();
    loop {
        let chunk = api.read_file(source.clone(), Some(offset), None).await?;
        if offset == 0 {
            if chunk.file_size > max_bytes {
                return Err(KernelError::io(format!(
                    "file too large: {} bytes (max {max_bytes})",
                    chunk.file_size
                )));
            }
            mime.clone_from(&chunk.mime);
            bytes.reserve(chunk.file_size as usize);
        }
        let advanced = chunk.end_offset > offset;
        if advanced {
            let data = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .map_err(|e| KernelError::io(format!("decode file chunk: {e}")))?;
            bytes.extend_from_slice(&data);
            offset = chunk.end_offset;
        }
        if offset >= chunk.file_size {
            return Ok((bytes, mime));
        }
        if !advanced {
            return Err(KernelError::io(format!(
                "file read stalled at {offset} bytes"
            )));
        }
    }
}

/// Unified API for both local (in-process) and remote (IPC) kernels.
#[async_trait]
pub trait KernelApi: Send + Sync {
    /// Gracefully stop the kernel and all background tasks：本地
    /// kernel 先 cancel 再等持久化排空（10s 上界）最后关 bus；
    /// 远程 kernel 的生命周期在服务端——`stop` 仅断开本地连接
    /// （runtime 内 fire-and-forget；无 runtime 时同步等锁断开，
    /// 见 `swap_kernel` 的 `block_on` fallback）。**唯一的关停入口**。
    async fn stop(&self);

    /// Whether the kernel is currently reachable.
    ///
    /// Local kernels are always connected. The remote client reports
    /// `false` once the daemon connection has been invalidated (heartbeat
    /// loss, send/RPC failure) and not yet re-established — callers can use
    /// this to skip RPCs that would only stall until their timeout.
    async fn is_connected(&self) -> bool;

    // ── Config ─────────────────────────────────────────────────────────────
    async fn get_config(&self) -> Result<crate::config::KernelConfig>;
    async fn set_config(&self, content: String) -> Result<()>;
    async fn restart(&self) -> Result<()>;

    // ── Files ────────────────────────────────────────────────────────────
    /// Read a byte range of a daemon-side file (see
    /// `crate::utils::file_read`). Resolution happens on the daemon's host,
    /// so this works identically against local and remote daemons.
    async fn read_file(
        &self,
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<crate::utils::file_read::FileBytes>;

    // ── Project ──────────────────────────────────────────────────────────
    async fn list_projects(&self) -> Result<Vec<Project>>;
    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project>;
    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>>;
    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()>;
    /// Delete a project and all its sessions (incl. subagents) with their
    /// resources. Returns a report of what was removed.
    async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport>;

    // ── Session ──────────────────────────────────────────────────────────
    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId>;
    async fn restore_session(&self, id: &SessionId) -> Result<SessionId>;
    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId>;
    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()>;
    async fn cancel(&self, session_id: &SessionId) -> Result<()>;
    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()>;
    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()>;
    async fn compact_session(&self, session_id: &SessionId) -> Result<()>;
    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()>;
    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()>;
    async fn pin_session(&self, session_id: &SessionId, emoji: Option<String>) -> Result<()>;
    async fn unpin_session(&self, session_id: &SessionId) -> Result<()>;
    async fn set_pinned_session_emoji(
        &self,
        session_id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()>;
    async fn list_pinned_sessions(
        &self,
    ) -> Result<Vec<crate::storage::pinned_session::PinnedSessionDetail>>;
    async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer>;
    async fn remove_favorite(&self, id: &str) -> Result<()>;
    async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()>;
    async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>>;
    async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()>;
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
    async fn clear_session(&self, session_id: &SessionId) -> Result<()>;
    /// Pending mailbox contents (steer + queued user messages), FIFO.
    async fn mailbox_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::comms::MailboxSnapshot>;
    /// Retract one pending mailbox item (best-effort: already consumed
    /// → false).
    async fn remove_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool>;
    /// Promote a queued user message to a steer (atomic server-side move).
    async fn steer_mailbox_item(&self, session_id: &SessionId, item_id: &str) -> Result<bool>;
    /// Clear pending mailbox items by scope without cancelling the run.
    /// Returns the number removed.
    async fn clear_mailbox(
        &self,
        session_id: &SessionId,
        scope: crate::comms::MailboxScope,
    ) -> Result<usize>;
    async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>>;
    async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<SessionJsonlChunk>;
    async fn get_session(&self, session_id: &SessionId) -> Result<crate::types::SessionResponse>;
    /// Rules in effect for a session: channel rules of its chat (when
    /// channel-routed) + the session's own rules.
    async fn get_session_rules(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionRulesResponse>;
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
        after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber>;
    /// Subscribe to the live event stream of **all** sessions (real-time
    /// only, no replay). Overlapping per-session subscriptions are not
    /// deduplicated — mix both and dedupe by `event_id` if needed.
    async fn subscribe_all_events(&self) -> Result<crate::comms::EventBusSubscriber>;
    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: crate::storage::session::SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions>;
    async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>>;
    async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>>;
    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>>;
    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()>;
    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>>;
    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()>;
    async fn send_continue(&self, session_id: &SessionId) -> Result<()>;
    async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>>;
    async fn subscribe_notifications(&self) -> Result<mpsc::Receiver<Notification>>;

    // ── Usage ──────────────────────────────────────────────────────────
    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary>;
    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>>;
    async fn get_model_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::ModelUsage>>;
    async fn get_model_usage_since(
        &self,
        start: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::storage::usage::ModelUsage>>;
    async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::usage::UsageRecord>>;

    // ── Cron Job ─────────────────────────────────────────────────────────
    //
    // DESIGN PRINCIPLE: All cron operations MUST go through `KernelApi`.
    // Clients (GUI, TUI, CLI) must never hold a `CronStore` directly, because
    // that would only work in local/in-process mode and break remote IPC mode.
    // By routing every cron call through the kernel, both `LocalKernel`
    // and `RemoteKernel` can serve the same interface.
    // ──────────────────────────────────────────────────────────────────────

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId>;
    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>>;
    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>>;
    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool>;
    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool>;
    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()>;

    // ── Channels ───────────────────────────────────────────────────────
    /// Open a platform thread in `chat_id` and run `text` in a fresh
    /// session bound to it. Returns the session id, anchor message id
    /// and thread jump link.
    async fn channel_new_thread(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        title: Option<String>,
        text: String,
    ) -> Result<serde_json::Value>;

    /// Query (`on` absent) or switch a chat's watch mode. Result:
    /// `{on, session_id}`.
    async fn set_channel_watch(
        &self,
        channel: Option<String>,
        platform: Option<String>,
        chat_id: String,
        on: Option<bool>,
    ) -> Result<serde_json::Value>;

    // ── Model ──────────────────────────────────────────────────────────────
    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>>;
    async fn get_session_model(&self, session_id: &SessionId) -> Result<String>;
    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()>;
    async fn get_session_context_window(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::kernel::ContextWindowInfo>;
    /// `None` 清除覆盖（跟随模型配置）。
    async fn set_session_context_window(
        &self,
        session_id: &SessionId,
        tokens: Option<u32>,
    ) -> Result<()>;

    // ── Agent Template ─────────────────────────────────────────────────────
    async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>>;
    async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()>;
    async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()>;
}
