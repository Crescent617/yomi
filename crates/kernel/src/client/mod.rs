use crate::checkpoint::RewindTarget;
use crate::event::Command;
use crate::goal::GoalState;
use crate::kernel::CreateSessionInput;
use crate::kernel::Kernel;
use crate::notification::Notification;
use crate::permission::Level;
use crate::storage::session::SessionListScope;
use crate::transport::{recv_frame, send_frame, ReadHalf, SocketAddr, Stream, WriteHalf};
use crate::types::{
    ContentBlock, KernelError, MessageId, Project, ProjectId, Result, SessionError, SessionId,
};
use crate::wire::{Envelope, ReqMethod, RequestIdGenerator, RespBody, RpcError, WireMsg};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::{broadcast, Mutex};

/// How long to retry connecting to the daemon on first use.
/// Daemon initialisation (storage, provider, skills) can take several
/// seconds, so we allow a generous timeout.
const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_secs(10);
/// Interval between connection retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
/// RPC request timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
/// Heartbeat interval in seconds.
const HEARTBEAT_INTERVAL_SECS: u64 = 2;
/// Heartbeat timeout in seconds (3 missed heartbeats).
const HEARTBEAT_TIMEOUT_SECS: u64 = 6;

type PendingMap = dashmap::DashMap<
    u64,
    tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, RpcError>>,
>;
type EventRouterMap = dashmap::DashMap<String, broadcast::Sender<Envelope>>;

/// Router key collecting events from **all** sessions (used by
/// `subscribe_all_events`; session IDs are ULIDs, so "*" never collides).
const ALL_EVENTS_ROUTER_KEY: &str = "*";

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
    /// Gracefully stop the kernel and all background tasks.
    fn stop(&self);

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
    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()>;
    async fn pause_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn resume_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>>;
    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()>;
    async fn stop_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
    async fn clear_session(&self, session_id: &SessionId) -> Result<()>;
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

    // ── Model ──────────────────────────────────────────────────────────────
    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>>;
    async fn get_session_model(&self, session_id: &SessionId) -> Result<String>;
    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()>;

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

// ── LocalKernel (existing Kernel wrapped) ──────────────────────

#[async_trait]
impl KernelApi for Kernel {
    fn stop(&self) {
        Self::stop(self);
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn get_config(&self) -> Result<crate::config::KernelConfig> {
        crate::config::Config::get_kernel_config()
    }

    async fn set_config(&self, content: String) -> Result<()> {
        crate::config::Config::set_kernel_config(&content)
    }

    async fn restart(&self) -> Result<()> {
        Err(KernelError::config(
            "restart is only available through a daemon server",
        ))
    }

    async fn read_file(
        &self,
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<crate::utils::file_read::FileBytes> {
        crate::utils::file_read::read_file(&source, &self.data_dir().await, offset, limit).await
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        Self::list_projects(self).await
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        Self::create_project(self, dir, name).await
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        Self::get_project(self, id).await
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        Self::rename_project(self, id, name).await
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport> {
        Self::delete_project(self, id).await
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        Self::create_session(self, input).await
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        Self::restore_session(self, id).await
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        Self::fork_session(self, parent, auto_approve_level).await
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        Self::send_message(self, session_id, blocks).await
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        Self::cancel(self, session_id);
        Ok(())
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        _remember: bool,
    ) -> Result<()> {
        Self::send_permission_response(self, session_id, req_id, approved, _remember)
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        Self::set_permission_level(self, session_id, level).await
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        Self::compact_session(self, session_id);
        Ok(())
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        Self::rewind_session(self, session_id, message_id, target).await
    }

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        Self::rename_session(self, session_id, title).await
    }

    async fn pin_session(&self, session_id: &SessionId, emoji: Option<String>) -> Result<()> {
        Self::pin_session(self, session_id, emoji).await
    }

    async fn unpin_session(&self, session_id: &SessionId) -> Result<()> {
        Self::unpin_session(self, session_id).await
    }

    async fn set_pinned_session_emoji(
        &self,
        session_id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        Self::set_pinned_session_emoji(self, session_id, emoji).await
    }

    async fn list_pinned_sessions(
        &self,
    ) -> Result<Vec<crate::storage::pinned_session::PinnedSessionDetail>> {
        Self::list_pinned_sessions(self).await
    }

    async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer> {
        Self::add_favorite(self, input).await
    }

    async fn remove_favorite(&self, id: &str) -> Result<()> {
        Self::remove_favorite(self, id).await
    }

    async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()> {
        Self::remove_favorite_by_message(self, session_id, message_id).await
    }

    async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>> {
        Self::list_favorites(self, query, limit, offset).await
    }

    async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()> {
        Self::update_favorite_note(self, id, note).await
    }

    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()> {
        Self::start_goal(self, session_id, state).await
    }

    async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::pause_goal(self, session_id).await
    }

    async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::resume_goal(self, session_id).await
    }

    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        Self::get_goal(self, session_id).await
    }

    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()> {
        Self::update_goal(self, session_id, description).await
    }

    async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::stop_goal(self, session_id).await
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        Self::delete_session(self, session_id).await
    }

    async fn clear_session(&self, session_id: &SessionId) -> Result<()> {
        Self::clear_session(self, session_id)
    }

    async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        Self::list_messages(self, session_id).await
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<crate::types::SessionResponse> {
        Ok(Self::get_session(self, session_id).await?)
    }

    async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<SessionJsonlChunk> {
        Self::read_session_jsonl(self, session_id, before_offset, after_offset).await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
        _after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        Ok(Self::subscribe_session_events(self, session_id))
    }

    async fn subscribe_all_events(&self) -> Result<crate::comms::EventBusSubscriber> {
        // Same Internal filter as `subscribe_session_events` — internal
        // events never leave the kernel.
        Ok(Self::event_bus(self)
            .expect("event_bus must be configured")
            .subscribe_all_filtered(|envelope| {
                !matches!(envelope.event, crate::event::Event::Internal(_))
            }))
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        Self::list_sessions(self, project_id, scope, before, limit).await
    }

    async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>> {
        Self::list_running_sessions(self).await
    }

    async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>> {
        Self::list_subagents(self, parent_session_id).await
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        Self::get_checkpoints(self, session_id).await
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        Self::get_todos(self, session_id).await
    }

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        Self::send_ask_user_response(self, session_id, req_id, response)
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        Self::send_steer(self, session_id, content).await;
        Ok(())
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        Self::send_continue(self, session_id);
        Ok(())
    }

    async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        Self::list_session_skills(self, session_id).await
    }

    async fn subscribe_notifications(&self) -> Result<mpsc::Receiver<Notification>> {
        let mut rx = self.notification_bus().subscribe();
        let (tx, mpsc_rx) = mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(noti) => {
                        if tx.send(noti).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Notification subscriber lagged, dropped {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(mpsc_rx)
    }

    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary> {
        Self::get_usage_summary(self, days).await
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        Self::get_daily_usage(self, days).await
    }

    async fn get_model_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        Self::get_model_usage(self, days).await
    }

    async fn get_model_usage_since(
        &self,
        start: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        Self::get_model_usage_since(self, start).await
    }

    async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::usage::UsageRecord>> {
        Self::get_usage_records(self, before_id, limit).await
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        Self::create_cron_job(self, input).await
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        Self::list_cron_jobs(self, status, limit).await
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        Self::get_cron_job(self, id).await
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        Self::update_cron_job(self, id, input).await
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        Self::delete_cron_job(self, id).await
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        Self::trigger_cron_job(self, id).await
    }

    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>> {
        Self::list_models(self).await
    }

    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        Ok(Self::get_session_model(self, session_id).await)
    }

    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        Self::set_session_model(self, session_id, key).await
    }

    async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>> {
        Self::list_agent_templates(self, session_id).await
    }

    async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()> {
        Self::save_agent_template(self, session_id, scope, name, body).await
    }

    async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()> {
        Self::delete_agent_template(self, session_id, scope, name).await
    }
}

// ── RemoteKernel (IPC client with lazy connect) ─────────────────────

struct Connection {
    write_half: Arc<Mutex<WriteHalf>>,
    pending: Arc<PendingMap>,
    _reader: tokio::task::JoinHandle<()>,
    _heartbeat: tokio::task::JoinHandle<()>,
    /// Cancelled when the connection is dead (reader or heartbeat
    /// detected an error, or the caller explicitly killed the old
    /// connection).  `ensure_connected()` checks this to decide
    /// whether a reconnect is needed.
    cancel: tokio_util::sync::CancellationToken,
}

/// Client-side kernel proxy that talks to a kernel daemon over IPC.
/// Uses lazy connect: the connection is established on the first API call.
pub struct RemoteKernel {
    addr: SocketAddr,
    req_id: RequestIdGenerator,
    connection: Arc<Mutex<Option<Connection>>>,
    /// Persistent local event routers: `session_id` -> broadcast sender.
    /// Lifetime is independent of individual connections so that receivers
    /// survive reconnects.
    event_routers: Arc<EventRouterMap>,
    /// Local broadcast channel for notifications received from the wire.
    notification_tx: broadcast::Sender<Notification>,
}

impl RemoteKernel {
    /// Create a lazy kernel that connects on first use.
    pub fn new(addr: SocketAddr) -> Self {
        let (notification_tx, _) = broadcast::channel(256);
        Self {
            addr,
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers: Arc::new(EventRouterMap::new()),
            notification_tx,
        }
    }

    /// Connect immediately and return a ready kernel.
    pub async fn connect(addr: &SocketAddr) -> Result<Self> {
        let stream = crate::transport::connect(addr).await?;
        let this = Self::from_stream(stream, addr).await?;
        this.validate_wire_protocol().await?;
        Ok(this)
    }

    /// Wrap an already-connected stream.
    pub async fn from_stream(stream: Stream, addr: &SocketAddr) -> Result<Self> {
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let event_routers: Arc<EventRouterMap> = Arc::new(EventRouterMap::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let last_pong = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
        let (notification_tx, _) = broadcast::channel(256);

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&event_routers),
            notification_tx.clone(),
            Arc::clone(&last_pong),
            cancel.clone(),
        );
        let heartbeat = Self::spawn_heartbeat(Arc::clone(&write_half), last_pong, cancel.clone());

        let this = Self {
            addr: addr.clone(),
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers,
            notification_tx,
        };
        *this.connection.lock().await = Some(Connection {
            write_half,
            pending,
            _reader: reader,
            _heartbeat: heartbeat,
            cancel,
        });
        Ok(this)
    }

    fn spawn_reader(
        mut read_half: ReadHalf,
        write_half: Arc<Mutex<WriteHalf>>,
        pending: Arc<PendingMap>,
        event_routers: Arc<EventRouterMap>,
        notification_tx: broadcast::Sender<Notification>,
        last_pong: Arc<std::sync::Mutex<tokio::time::Instant>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    result = recv_frame(&mut read_half) => {
                        let msg = match result {
                            Ok(m) => m,
                            Err(e) => {
                                if e.kind() == std::io::ErrorKind::InvalidData {
                                    // Daemon sent an oversized or malformed frame.
                                    tracing::warn!("Inbound frame rejected: {e}");
                                } else {
                                    tracing::warn!("Remote reader error: {e}");
                                }
                                break;
                            }
                        };

                        match msg {
                            WireMsg::Response { id, body } => {
                                let result = match body {
                                    RespBody::Ok { result } => Ok(result),
                                    RespBody::Err { error } => Err(error),
                                };
                                if let Some((_, tx)) = pending.remove(&id) {
                                    let _ = tx.send(result);
                                }
                            }
                            WireMsg::Event(envelope) => {
                                if let Some(entry) = event_routers.get(envelope.session_id.as_str()) {
                                    let _ = entry.value().send(envelope.clone());
                                }
                                if let Some(entry) = event_routers.get(ALL_EVENTS_ROUTER_KEY) {
                                    let _ = entry.value().send(envelope);
                                }
                            }
                            WireMsg::Noti(noti) => {
                                let _ = notification_tx.send(noti);
                            }
                            WireMsg::Ping => {
                                let mut guard = write_half.lock().await;
                                let _ = send_frame(&mut *guard, &WireMsg::Pong).await;
                            }
                            WireMsg::Pong => {
                                let mut guard = last_pong
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                *guard = tokio::time::Instant::now();
                            }
                            WireMsg::Request { .. } => {
                                tracing::warn!("Unexpected message from server: {:?}", msg);
                            }
                        }
                    }
                }
            }

            cancel.cancel();
            // Notify pending RPCs.
            let keys: Vec<u64> = pending.iter().map(|e| *e.key()).collect();
            for key in keys {
                if let Some((_, tx)) = pending.remove(&key) {
                    let _ = tx.send(Err(RpcError {
                        code: "connection_closed".to_string(),
                        message: "Connection to kernel daemon closed".to_string(),
                        detail: None,
                    }));
                }
            }
            // Keep persistent routers alive across reconnects. Existing receivers
            // continue consuming from the same broadcast senders after the new
            // connection re-subscribes server-side.
            for entry in event_routers.iter() {
                let _ = notification_tx.send(Notification::ConnectionLost {
                    session_id: SessionId::from(entry.key().clone()),
                });
            }
        })
    }

    fn spawn_heartbeat(
        write_half: Arc<Mutex<WriteHalf>>,
        last_pong: Arc<std::sync::Mutex<tokio::time::Instant>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if cancel.is_cancelled() {
                    break;
                }
                let elapsed = last_pong
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed();
                if elapsed > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                    tracing::warn!(
                        "Heartbeat timeout (no pong for {:?}), disconnecting",
                        elapsed
                    );
                    cancel.cancel();
                    break;
                }
                let mut w = write_half.lock().await;
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    send_frame(&mut *w, &WireMsg::Ping),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!("Heartbeat send_frame failed: {e}");
                        cancel.cancel();
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("Heartbeat send_frame timed out (3s)");
                        cancel.cancel();
                        break;
                    }
                }
            }
        })
    }

    pub async fn check_ready(&self) -> Result<()> {
        self.ensure_connected().await
    }

    async fn server_instance_id(&self) -> Result<String> {
        self.ensure_connected().await?;
        let value = self.call_raw(ReqMethod::Hello).await?;
        value
            .get("instance_id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| SessionError::WireProtocolMismatch.into())
    }

    /// Retries for up to 10 s to allow the daemon to finish spawning.
    /// On reconnect, re-subscribes all sessions in the persistent router.
    async fn ensure_connected(&self) -> Result<()> {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = *guard {
            if !conn.cancel.is_cancelled() {
                return Ok(());
            }
        }
        if let Some(old) = guard.take() {
            // Cancel the old connection so tasks exit naturally and run
            // cleanup (notify pending RPCs, send Shutdown events, drop
            // local event router senders so receivers become Closed).
            old.cancel.cancel();
            // We do NOT abort here: abort() skips the cleanup code at
            // the end of the reader task, which means TUI receivers
            // never learn the connection is dead.
        }
        let start = tokio::time::Instant::now();
        let stream = loop {
            match crate::transport::connect(&self.addr).await {
                Ok(s) => break s,
                Err(_) if start.elapsed() < CONNECT_RETRY_TIMEOUT => {
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
                Err(e) => {
                    return Err(
                        SessionError::Other(format!("Failed to connect to daemon: {e}")).into(),
                    );
                }
            }
        };
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let last_pong = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&self.event_routers),
            self.notification_tx.clone(),
            Arc::clone(&last_pong),
            cancel.clone(),
        );
        let heartbeat = Self::spawn_heartbeat(Arc::clone(&write_half), last_pong, cancel.clone());

        *guard = Some(Connection {
            write_half: Arc::clone(&write_half),
            pending,
            _reader: reader,
            _heartbeat: heartbeat,
            cancel,
        });

        // Collect sessions that still have active local receivers.
        // We drop the lock here so that `call()` (which also calls
        // `ensure_connected`) can acquire it.
        let sessions_to_resub: Vec<String> = self
            .event_routers
            .iter()
            .filter(|e| e.value().receiver_count() > 0)
            .map(|e| e.key().clone())
            .collect();
        drop(guard);

        // Re-subscribe sessions that still have active local receivers.
        // We do NOT remove stale routers here: doing so would drop the
        // `broadcast::Sender`, causing the UI's `event_rx` to become
        // `Closed` and the TUI to exit immediately.  Instead we leave
        // the router in place; the UI will learn that the session is
        // gone when subsequent `send_message` calls return
        // `session_not_found`.
        for sid in sessions_to_resub {
            if let Err(e) = Box::pin(self.call(ReqMethod::Subscribe {
                session_id: sid,
                after_event_id: None,
            }))
            .await
            {
                tracing::warn!("Re-subscribe failed: {e}");
            }
        }

        // Wire protocol version handshake.
        self.validate_wire_protocol().await?;

        Ok(())
    }

    async fn validate_wire_protocol(&self) -> Result<()> {
        // Wire protocol version handshake.
        match self.call_raw(ReqMethod::Hello).await {
            Ok(val) => {
                let server_proto = val
                    .get("proto")
                    .and_then(|v| v.as_u64())
                    .map_or(0, |n| n as u32);
                let client_proto = crate::wire::WIRE_PROTOCOL_VERSION;
                if server_proto != client_proto {
                    tracing::error!(
                        "Wire protocol version mismatch: server v{}, client v{}",
                        server_proto,
                        client_proto,
                    );
                    self.invalidate_connection().await;
                    return Err(SessionError::WireProtocolMismatch.into());
                }
            }
            Err(e) => {
                // Old daemon that doesn't recognise `Hello` will close the
                // connection (serde unknown variant). Treat this as a fatal
                // mismatch rather than silently degrading.
                tracing::error!("Hello handshake failed (old daemon?): {e}");
                self.invalidate_connection().await;
                return Err(SessionError::WireProtocolMismatch.into());
            }
        }

        Ok(())
    }

    async fn invalidate_connection(&self) {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = guard.take() {
            conn.cancel.cancel();
        }
    }

    async fn call_raw(&self, method: ReqMethod) -> Result<serde_json::Value> {
        let id = self.req_id.next();

        // Grab write_half and install pending oneshot, then drop the
        // connection lock so we don't hold it across the network await.
        let (write_half, rx) = {
            let guard = self.connection.lock().await;
            let conn = guard
                .as_ref()
                .ok_or_else(|| KernelError::from(SessionError::ConnectionLost))?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            conn.pending.insert(id, tx);
            (Arc::clone(&conn.write_half), rx)
        };

        let msg = WireMsg::Request { id, method };
        {
            let mut w = write_half.lock().await;
            match tokio::time::timeout(Duration::from_secs(5), send_frame(&mut *w, &msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    drop(w);
                    self.invalidate_connection().await;
                    return Err(SessionError::SendFailed(e.to_string()).into());
                }
                Err(_) => {
                    drop(w);
                    self.invalidate_connection().await;
                    return Err(SessionError::SendFailed("write timeout (5s)".to_string()).into());
                }
            }
        }

        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(Ok(val))) => Ok(val),
            Ok(Ok(Err(e))) => {
                // If the server sent a structured session error, try to
                // reconstruct it exactly instead of losing the variant.
                if e.code == "session_error" {
                    if let Some(ref d) = e.detail {
                        if let Ok(se) = serde_json::from_value::<SessionError>(d.clone()) {
                            return Err(KernelError::from(se));
                        }
                    }
                    return Err(SessionError::Other(format!(
                        "RPC session error [{}]: {}",
                        e.code, e.message
                    ))
                    .into());
                }
                Err(SessionError::Other(format!("RPC error [{}]: {}", e.code, e.message)).into())
            }
            Ok(Err(_)) => Err(SessionError::Cancelled.into()),
            Err(_) => {
                // RPC timeout usually means the reader task is stuck or
                // the server is dead.  Force a reconnect on the next
                // call by dropping the connection.
                self.invalidate_connection().await;
                Err(SessionError::RequestTimeout.into())
            }
        }
    }

    /// Send a raw wire request and return the untyped result value.
    ///
    /// Escape hatch for tooling (e.g. `yomi rpc`) that talks the wire
    /// protocol directly without a typed `KernelApi` wrapper. Prefer the
    /// typed trait methods for anything permanent. Streaming methods
    /// (`Subscribe`/`SubscribeAll`) only return an ack here — use
    /// `subscribe_session_events`/`subscribe_all_events` to follow events.
    pub async fn call(&self, method: ReqMethod) -> Result<serde_json::Value> {
        self.ensure_connected().await?;
        self.call_raw(method).await
    }

    async fn subscribe_events_internal(
        &self,
        session_id: &SessionId,
        after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        use dashmap::mapref::entry::Entry;

        let tx = match self.event_routers.entry(session_id.0.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(256);
                entry.insert(tx.clone());
                tx
            }
        };

        let result = self
            .call(ReqMethod::Subscribe {
                session_id: session_id.0.to_string(),
                after_event_id,
            })
            .await;
        if let Err(ref e) = result {
            // Only remove the local router when the server explicitly
            // says the session is gone.  Transient errors (timeout, write
            // failure) should leave the router in place so that a later
            // re-subscribe can reuse the same sender.
            if e.is_session_not_found() {
                self.event_routers.remove(session_id.0.as_str());
            }
            return Err(result.unwrap_err());
        }

        let mut broadcast_rx = tx.subscribe();
        let (mpsc_tx, mpsc_rx) = mpsc::channel::<(SessionId, crate::wire::Envelope)>(256);
        let sid = session_id.clone();
        tokio::spawn(async move {
            while let Ok(ev) = broadcast_rx.recv().await {
                if mpsc_tx.send((sid.clone(), ev)).await.is_err() {
                    break;
                }
            }
        });

        Ok(crate::comms::EventBusSubscriber::from_receiver(mpsc_rx))
    }
}

#[async_trait]
impl KernelApi for RemoteKernel {
    fn stop(&self) {
        // Remote kernel lifecycle is managed server-side.
        // Just drop any local connection resources.
        let conn = self.connection.clone();
        tokio::spawn(async move {
            let mut guard = conn.lock().await;
            if let Some(c) = guard.take() {
                c.cancel.cancel();
            }
        });
    }

    async fn is_connected(&self) -> bool {
        // try_lock: ensure_connected holds the mutex across its reconnect
        // loop (up to 10s); a locked connection is unusable anyway, so
        // report not-connected instead of stalling the caller.
        match self.connection.try_lock() {
            Ok(guard) => matches!(guard.as_ref(), Some(conn) if !conn.cancel.is_cancelled()),
            Err(_) => false,
        }
    }

    async fn get_config(&self) -> Result<crate::config::KernelConfig> {
        let result = self.call(ReqMethod::GetConfig).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn set_config(&self, content: String) -> Result<()> {
        self.call(ReqMethod::SetConfig { content }).await?;
        Ok(())
    }

    async fn restart(&self) -> Result<()> {
        let old_instance_id = self.server_instance_id().await?;
        self.call(ReqMethod::Restart).await?;
        self.invalidate_connection().await;

        let start = tokio::time::Instant::now();
        loop {
            match self.server_instance_id().await {
                Ok(instance_id) if instance_id != old_instance_id => break,
                Ok(_) | Err(_) if start.elapsed() < CONNECT_RETRY_TIMEOUT => {
                    self.invalidate_connection().await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Ok(_) => {
                    return Err(SessionError::Other(
                        "daemon restart timed out waiting for a replacement instance".to_string(),
                    )
                    .into());
                }
                Err(error) => return Err(error),
            }
        }

        let config = self.get_config().await?;
        if config.full_config.is_empty() {
            return Err(KernelError::config(
                "daemon restarted but the saved config could not be applied",
            ));
        }
        Ok(())
    }

    async fn read_file(
        &self,
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> Result<crate::utils::file_read::FileBytes> {
        let result = self
            .call(ReqMethod::ReadFile {
                source,
                offset,
                limit,
            })
            .await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        let result = self.call(ReqMethod::ListProjects).await?;
        let projects: Vec<Project> = serde_json::from_value(result)?;
        Ok(projects)
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let result = self
            .call(ReqMethod::CreateProject {
                dir: dir.to_string_lossy().to_string(),
                name,
            })
            .await?;
        let project: Project = serde_json::from_value(result)?;
        Ok(project)
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let result = self
            .call(ReqMethod::GetProject {
                project_id: id.0.to_string(),
            })
            .await?;
        let project: Option<Project> = serde_json::from_value(result)?;
        Ok(project)
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.call(ReqMethod::RenameProject {
            project_id: id.0.to_string(),
            name,
        })
        .await?;
        Ok(())
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<crate::storage::GcReport> {
        let result = self
            .call(ReqMethod::DeleteProject {
                project_id: id.0.to_string(),
            })
            .await?;
        let report: crate::storage::GcReport = serde_json::from_value(result)?;
        Ok(report)
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let result = self
            .call(ReqMethod::CreateSession {
                project_id: input.project_id.map(|p| p.0.to_string()),
                working_dir: input.working_dir.map(|p| p.to_string_lossy().to_string()),
                auto_approve_level: input.auto_approve_level,
                model_key: input.model_key,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId::from(sid))
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        let result = self
            .call(ReqMethod::RestoreSession {
                session_id: id.0.to_string(),
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId::from(sid))
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let result = self
            .call(ReqMethod::ForkSession {
                parent_id: parent.0.to_string(),
                auto_approve_level,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId::from(sid))
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        self.call(ReqMethod::SendMessage {
            session_id: session_id.0.to_string(),
            blocks,
        })
        .await?;
        Ok(())
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Cancel,
        })
        .await?;
        Ok(())
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Response {
                req_id: req_id.to_string(),
                approved,
                remember,
            },
        })
        .await?;
        Ok(())
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::SetLevel(level),
        })
        .await?;
        Ok(())
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Compact,
        })
        .await?;
        Ok(())
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Rewind { message_id, target },
        })
        .await?;
        Ok(())
    }

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        self.call(ReqMethod::RenameSession {
            session_id: session_id.0.to_string(),
            title,
        })
        .await?;
        Ok(())
    }

    async fn pin_session(&self, session_id: &SessionId, emoji: Option<String>) -> Result<()> {
        self.call(ReqMethod::PinSession {
            session_id: session_id.0.to_string(),
            icon_emoji: emoji,
        })
        .await?;
        Ok(())
    }

    async fn unpin_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::UnpinSession {
            session_id: session_id.0.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn set_pinned_session_emoji(
        &self,
        session_id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        self.call(ReqMethod::SetPinnedSessionEmoji {
            session_id: session_id.0.to_string(),
            icon_emoji: emoji,
        })
        .await?;
        Ok(())
    }

    async fn list_pinned_sessions(
        &self,
    ) -> Result<Vec<crate::storage::pinned_session::PinnedSessionDetail>> {
        let result = self.call(ReqMethod::ListPinnedSessions).await?;
        let sessions = serde_json::from_value(result)?;
        Ok(sessions)
    }

    async fn add_favorite(
        &self,
        input: crate::storage::AddFavoriteInput,
    ) -> Result<crate::storage::FavoriteAnswer> {
        let result = self.call(ReqMethod::AddFavorite { input }).await?;
        let favorite = serde_json::from_value(result)?;
        Ok(favorite)
    }

    async fn remove_favorite(&self, id: &str) -> Result<()> {
        self.call(ReqMethod::RemoveFavorite {
            favorite_id: id.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn remove_favorite_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<()> {
        self.call(ReqMethod::RemoveFavoriteByMessage {
            session_id: session_id.0.to_string(),
            message_id: message_id.0.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn list_favorites(
        &self,
        query: Option<String>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::storage::FavoriteAnswer>> {
        let result = self
            .call(ReqMethod::ListFavorites {
                query,
                limit,
                offset,
            })
            .await?;
        let favorites = serde_json::from_value(result)?;
        Ok(favorites)
    }

    async fn update_favorite_note(&self, id: &str, note: Option<String>) -> Result<()> {
        self.call(ReqMethod::UpdateFavoriteNote {
            favorite_id: id.to_string(),
            note,
        })
        .await?;
        Ok(())
    }

    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::StartGoal(state),
        })
        .await?;
        Ok(())
    }

    async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::PauseGoal,
        })
        .await?;
        Ok(())
    }

    async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::ResumeGoal,
        })
        .await?;
        Ok(())
    }

    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        let result = self
            .call(ReqMethod::Command {
                session_id: session_id.0.to_string(),
                cmd: Command::GetGoal,
            })
            .await?;
        let goal: Option<crate::goal::GoalState> = serde_json::from_value(result)?;
        Ok(goal)
    }

    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::EditGoal { description },
        })
        .await?;
        Ok(())
    }

    async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::StopGoal,
        })
        .await?;
        Ok(())
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::DeleteSession {
            session_id: session_id.0.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn clear_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::ClearSession {
            session_id: session_id.0.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        let result = self
            .call(ReqMethod::ListMessages {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let messages: Vec<crate::types::SessionMessage> = serde_json::from_value(result)?;
        Ok(messages)
    }

    async fn get_session(&self, session_id: &SessionId) -> Result<crate::types::SessionResponse> {
        let result = self
            .call(ReqMethod::GetSession {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let session: crate::types::SessionResponse = serde_json::from_value(result)?;
        Ok(session)
    }

    async fn read_session_jsonl(
        &self,
        session_id: &SessionId,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    ) -> Result<SessionJsonlChunk> {
        let result = self
            .call(ReqMethod::ReadSessionJsonl {
                session_id: session_id.0.to_string(),
                before_offset,
                after_offset,
            })
            .await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
        after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        self.subscribe_events_internal(session_id, after_event_id)
            .await
    }

    async fn subscribe_all_events(&self) -> Result<crate::comms::EventBusSubscriber> {
        use dashmap::mapref::entry::Entry;

        let tx = match self.event_routers.entry(ALL_EVENTS_ROUTER_KEY.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(256);
                entry.insert(tx.clone());
                tx
            }
        };

        self.call(ReqMethod::SubscribeAll).await?;

        let mut broadcast_rx = tx.subscribe();
        let (mpsc_tx, mpsc_rx) = mpsc::channel::<(SessionId, crate::wire::Envelope)>(256);
        tokio::spawn(async move {
            while let Ok(ev) = broadcast_rx.recv().await {
                let sid = ev.session_id.clone();
                if mpsc_tx.send((sid, ev)).await.is_err() {
                    break;
                }
            }
        });

        Ok(crate::comms::EventBusSubscriber::from_receiver(mpsc_rx))
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        scope: SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        let result = self
            .call(ReqMethod::ListSessions {
                project_id: project_id.map(|p| p.0.to_string()),
                scope,
                before,
                limit,
            })
            .await?;
        let sessions: PaginatedSessions = serde_json::from_value(result)?;
        Ok(sessions)
    }

    async fn list_running_sessions(&self) -> Result<Vec<crate::types::RunningSessionResponse>> {
        let result = self.call(ReqMethod::ListRunningSessions).await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn list_subagents(
        &self,
        parent_session_id: &SessionId,
    ) -> Result<Vec<crate::types::SubagentResponse>> {
        let result = self
            .call(ReqMethod::ListSubagents {
                parent_session_id: parent_session_id.0.to_string(),
            })
            .await?;
        Ok(serde_json::from_value(result)?)
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        let result = self
            .call(ReqMethod::GetCheckpoints {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let checkpoints: Vec<crate::checkpoint::Checkpoint> = serde_json::from_value(result)?;
        Ok(checkpoints)
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        let result = self
            .call(ReqMethod::GetTodos {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let todos: Option<String> = serde_json::from_value(result)?;
        Ok(todos)
    }

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::AskUserResponse {
                req_id: req_id.to_string(),
                answers: response.answers.into_iter().collect(),
            },
        })
        .await?;
        Ok(())
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Steer { content },
        })
        .await?;
        Ok(())
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        self.call(ReqMethod::Command {
            session_id: session_id.0.to_string(),
            cmd: Command::Continue,
        })
        .await?;
        Ok(())
    }

    async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        let result = self
            .call(ReqMethod::ListSessionSkills {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let skills: Vec<Arc<crate::skill::Skill>> = serde_json::from_value(result)?;
        Ok(skills)
    }

    async fn subscribe_notifications(&self) -> Result<mpsc::Receiver<Notification>> {
        let mut rx = self.notification_tx.subscribe();
        let (tx, mpsc_rx) = mpsc::channel(256);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(noti) => {
                        if tx.send(noti).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Notification subscriber lagged, dropped {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(mpsc_rx)
    }

    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary> {
        let result = self
            .call(ReqMethod::GetUsageSummary { days: Some(days) })
            .await?;
        let summary: crate::storage::usage::UsageSummary = serde_json::from_value(result)?;
        Ok(summary)
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        let result = self.call(ReqMethod::GetDailyUsage { days }).await?;
        let daily: Vec<crate::storage::usage::DailyUsage> = serde_json::from_value(result)?;
        Ok(daily)
    }

    async fn get_model_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        let result = self.call(ReqMethod::GetModelUsage { days }).await?;
        let usage: Vec<crate::storage::usage::ModelUsage> = serde_json::from_value(result)?;
        Ok(usage)
    }

    async fn get_model_usage_since(
        &self,
        start: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::storage::usage::ModelUsage>> {
        let result = self.call(ReqMethod::GetModelUsageSince { start }).await?;
        let usage: Vec<crate::storage::usage::ModelUsage> = serde_json::from_value(result)?;
        Ok(usage)
    }

    async fn get_usage_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::storage::usage::UsageRecord>> {
        let result = self
            .call(ReqMethod::GetUsageRecords {
                before_id: before_id.map(String::from),
                limit,
            })
            .await?;
        let records: Vec<crate::storage::usage::UsageRecord> = serde_json::from_value(result)?;
        Ok(records)
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        #[derive(serde::Deserialize)]
        struct JobIdResponse {
            job_id: String,
        }
        let result = self
            .call(ReqMethod::CreateCronJob {
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                max_runs: input.max_runs,
                expires_at: input.expires_at,
            })
            .await?;
        let resp: JobIdResponse = serde_json::from_value(result)
            .map_err(|e| crate::types::KernelError::storage(format!("parse job_id: {e}")))?;
        Ok(crate::cron::CronJobId::from(resp.job_id))
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        let result = self
            .call(ReqMethod::ListCronJobs {
                status: status.map(|s| s.as_str().to_string()),
                limit,
            })
            .await?;
        let jobs: Vec<crate::cron::CronJob> = serde_json::from_value(result)?;
        Ok(jobs)
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        let result = self
            .call(ReqMethod::GetCronJob {
                job_id: id.0.to_string(),
            })
            .await?;
        let job: Option<crate::cron::CronJob> = serde_json::from_value(result)?;
        Ok(job)
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        let result = self
            .call(ReqMethod::UpdateCronJob {
                job_id: id.0.to_string(),
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                status: input.status.map(|s| s.as_str().to_string()),
                max_runs: input.max_runs,
                expires_at: input.expires_at,
            })
            .await?;
        let updated: bool = serde_json::from_value(result)?;
        Ok(updated)
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        let result = self
            .call(ReqMethod::DeleteCronJob {
                job_id: id.0.to_string(),
            })
            .await?;
        let deleted: bool = serde_json::from_value(result)?;
        Ok(deleted)
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        self.call(ReqMethod::TriggerCronJob {
            job_id: id.0.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn list_models(&self) -> Result<Vec<crate::kernel::ModelInfo>> {
        let result = self.call(ReqMethod::ListModels).await?;
        let models: Vec<crate::kernel::ModelInfo> = serde_json::from_value(result)?;
        Ok(models)
    }

    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        let result = self
            .call(ReqMethod::GetSessionModel {
                session_id: session_id.0.to_string(),
            })
            .await?;
        let key: String = serde_json::from_value(result)?;
        Ok(key)
    }

    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        self.call(ReqMethod::SetSessionModel {
            session_id: session_id.0.to_string(),
            key: key.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn list_agent_templates(
        &self,
        session_id: Option<&SessionId>,
    ) -> Result<Vec<crate::agent_tmpl::AgentTemplate>> {
        let result = self
            .call(ReqMethod::ListAgentTemplates {
                session_id: session_id.map(|s| s.0.to_string()),
            })
            .await?;
        let templates: Vec<crate::agent_tmpl::AgentTemplate> = serde_json::from_value(result)?;
        Ok(templates)
    }

    async fn save_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
        body: &str,
    ) -> Result<()> {
        self.call(ReqMethod::SaveAgentTemplate {
            session_id: session_id.map(|s| s.0.to_string()),
            scope,
            name: name.to_string(),
            body: body.to_string(),
        })
        .await?;
        Ok(())
    }

    async fn delete_agent_template(
        &self,
        session_id: Option<&SessionId>,
        scope: crate::agent_tmpl::TemplateScope,
        name: &str,
    ) -> Result<()> {
        self.call(ReqMethod::DeleteAgentTemplate {
            session_id: session_id.map(|s| s.0.to_string()),
            scope,
            name: name.to_string(),
        })
        .await?;
        Ok(())
    }
}
