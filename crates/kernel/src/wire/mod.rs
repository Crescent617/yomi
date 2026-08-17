use crate::event::Command;
pub use crate::notification::Notification;
use crate::permission::Level;
use crate::types::ContentBlock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Wire Protocol ────────────────────────────────────────────────────────

/// Wire protocol version. Bumped on any breaking change to the IPC schema.
pub const WIRE_PROTOCOL_VERSION: u32 = 25;

/// All operations a client can request from the daemon.
///
/// The derived `JsonSchema` powers `yomi rpc --help`: heavy payload types are
/// stubbed with `#[schemars(with = ...)]` so the derive doesn't cascade
/// across the codebase — schemas there are indicative, not exhaustive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReqMethod {
    /// Handshake: client checks daemon wire protocol version.
    Hello,

    // ── Config ───────────────────────────────────────────────────────────
    GetConfig,
    SetConfig {
        content: String,
    },
    Restart,

    // ── Files ────────────────────────────────────────────────────────────
    /// Read a byte range of a daemon-side file (asset or declared
    /// attachment; see `crate::utils::file_read`). `offset` defaults to 0;
    /// `limit` defaults to a server-side chunk size, and `Some(0)` returns
    /// metadata only.
    ReadFile {
        source: crate::utils::file_read::FileSource,
        offset: Option<u64>,
        limit: Option<u64>,
    },

    // ── Project ──────────────────────────────────────────────────────────
    ListProjects,
    CreateProject {
        dir: String,
        name: Option<String>,
    },
    GetProject {
        project_id: String,
    },
    RenameProject {
        project_id: String,
        name: String,
    },
    DeleteProject {
        project_id: String,
    },

    // ── Session ──────────────────────────────────────────────────────────
    CreateSession {
        project_id: Option<String>,
        working_dir: Option<String>,
        /// 缺省时走配置 `auto_approve`
        auto_approve_level: Option<Level>,
        model_key: Option<String>,
    },
    RestoreSession {
        session_id: String,
    },
    ForkSession {
        parent_id: String,
        auto_approve_level: Level,
    },
    SendMessage {
        session_id: String,
        #[schemars(with = "serde_json::Value")]
        blocks: Vec<ContentBlock>,
    },
    ListSessionSkills {
        session_id: String,
    },
    Command {
        session_id: String,
        #[schemars(with = "serde_json::Value")]
        cmd: Command,
    },
    Subscribe {
        session_id: String,
        /// If provided, the server will first replay all buffered events
        /// with an `event_id` strictly greater than this value, then switch
        /// to real-time push. If the id is not found in the buffer (e.g.
        /// it was cleared by a `MessageAdded` event), the server replays
        /// the entire current buffer.
        #[serde(default)]
        #[schemars(with = "Option<String>")]
        after_event_id: Option<crate::types::EventId>,
    },
    Unsubscribe {
        session_id: String,
    },
    /// Subscribe to the live event stream of **all** sessions (real-time
    /// only, no replay — cross-session history is not buffered globally).
    ///
    /// This stream is independent of per-session `Subscribe` streams: an
    /// event is delivered once per overlapping subscription, so a client
    /// mixing both must deduplicate by `event_id`.
    SubscribeAll,
    UnsubscribeAll,
    ListSessions {
        project_id: Option<String>,
        scope: crate::storage::session::SessionListScope,
        before: Option<DateTime<Utc>>,
        limit: usize,
    },
    ListRunningSessions,
    ListSubagents {
        parent_session_id: String,
    },
    ListMessages {
        session_id: String,
    },
    ReadSessionJsonl {
        session_id: String,
        before_offset: Option<u64>,
        after_offset: Option<u64>,
    },
    GetSession {
        session_id: String,
    },
    GetCheckpoints {
        session_id: String,
    },
    GetTodos {
        session_id: String,
    },
    RenameSession {
        session_id: String,
        title: String,
    },
    PinSession {
        session_id: String,
        icon_emoji: Option<String>,
    },
    UnpinSession {
        session_id: String,
    },
    SetPinnedSessionEmoji {
        session_id: String,
        icon_emoji: Option<String>,
    },
    ListPinnedSessions,
    ShutdownSession {
        session_id: String,
    },
    DeleteSession {
        session_id: String,
    },
    ClearSession {
        session_id: String,
    },

    // ── Favorites ────────────────────────────────────────────────────────
    AddFavorite {
        #[schemars(with = "serde_json::Value")]
        input: crate::storage::AddFavoriteInput,
    },
    RemoveFavorite {
        favorite_id: String,
    },
    RemoveFavoriteByMessage {
        session_id: String,
        message_id: String,
    },
    ListFavorites {
        query: Option<String>,
        limit: usize,
        offset: usize,
    },
    UpdateFavoriteNote {
        favorite_id: String,
        note: Option<String>,
    },

    // ── Cron Job ─────────────────────────────────────────────────────────
    CreateCronJob {
        name: String,
        schedule: String,
        #[schemars(with = "serde_json::Value")]
        action: crate::cron::CronAction,
        max_runs: Option<u32>,
        expires_at: Option<DateTime<Utc>>,
    },
    ListCronJobs {
        status: Option<String>,
        limit: usize,
    },
    GetCronJob {
        job_id: String,
    },
    UpdateCronJob {
        job_id: String,
        name: Option<String>,
        schedule: Option<String>,
        #[schemars(with = "Option<serde_json::Value>")]
        action: Option<crate::cron::CronAction>,
        status: Option<String>,
        /// `None` = 不变；`Some(0)` = 恢复不限次数
        max_runs: Option<u32>,
        /// `None` = 不变；`Some(NEVER_EXPIRES)` = 恢复永不过期
        expires_at: Option<DateTime<Utc>>,
    },
    /// Trigger a cron job manually (execute immediately, record result).
    TriggerCronJob {
        job_id: String,
    },

    DeleteCronJob {
        job_id: String,
    },

    // ── Usage ───────────────────────────────────────────────────────
    GetUsageSummary {
        days: Option<i64>,
    },
    GetDailyUsage {
        days: i64,
    },
    GetModelUsage {
        days: i64,
    },
    GetModelUsageSince {
        start: chrono::DateTime<chrono::Utc>,
    },
    GetUsageRecords {
        before_id: Option<String>,
        limit: usize,
    },

    // ── Channel ────────────────────────────────────────────────────
    ListChannels,

    // ── Model ────────────────────────────────────────────────────────
    ListModels,
    GetSessionModel {
        session_id: String,
    },
    SetSessionModel {
        session_id: String,
        key: String,
    },

    // ── Agent Template ───────────────────────────────────────────────
    ListAgentTemplates {
        session_id: Option<String>,
    },
    SaveAgentTemplate {
        session_id: Option<String>,
        scope: crate::agent_tmpl::TemplateScope,
        name: String,
        body: String,
    },
    DeleteAgentTemplate {
        session_id: Option<String>,
        scope: crate::agent_tmpl::TemplateScope,
        name: String,
    },
}

/// Response body — tagged union, no serde magic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RespBody {
    Ok { result: serde_json::Value },
    Err { error: RpcError },
}

/// Event envelope for wire transmission, includes session ID and event ID.
pub use crate::event::Envelope;

/// Wire-level message envelope for IPC between kernel daemon and clients.
///
/// Uses JSON over length-prefixed frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMsg {
    /// Client → Server: request with id.
    Request { id: u64, method: ReqMethod },

    /// Server → Client: response to a request.
    Response { id: u64, body: RespBody },

    /// Server → Client: event push from kernel.
    Event(Envelope),

    /// Server → Client: notification push from kernel.
    Noti(Notification),

    /// Heartbeat ping.
    Ping,

    /// Heartbeat pong.
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    /// Structured error detail (e.g. serialized `SessionError`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// Next request ID generator (thread-safe).
#[derive(Debug)]
pub struct RequestIdGenerator {
    next: std::sync::atomic::AtomicU64,
}

impl RequestIdGenerator {
    pub fn new() -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(1),
        }
    }

    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for RequestIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod mod_test;
