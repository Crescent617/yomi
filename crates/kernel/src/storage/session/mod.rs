//! Session management - session lifecycle and metadata storage

use crate::types::{Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SessionListScope {
    All,
    Assigned,
}

/// Session metadata for listing and display
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionInfo {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub message_count: i64,
    pub working_dir: Option<String>,
    pub project_id: Option<crate::types::ProjectId>,
    pub auto_approve_level: Option<String>,
    pub model_key: Option<String>,
    /// subagent spawn 时使用的角色模板名；普通 session 为 None
    pub template: Option<String>,
    /// session 级覆盖袋（`settings` 列 JSON 的 typed 视图）；`None` =
    /// 无任何覆盖（列 NULL 或空袋）
    pub settings: Option<SessionOverrides>,
}

/// Per-session 覆盖袋的 typed 视图（`sessions.settings` JSON object）。
/// 只读已知 key；写走 SQL `json_set`/`json_remove` 原子按键更新（不经
/// 读-改-写序列化），所以未来新增 key 不会被旧 daemon 吃掉。
/// 设计见 docs/design/session-context-window.md。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionOverrides {
    /// 覆盖模型的 context_window（压缩触发点、provider 输入自检、ctx%
    /// 展示）；`None` = 跟随模型配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}

impl SessionOverrides {
    /// 袋里是否一个 key 都没有（序列化为 `{}` 的等价物，存储层归一为
    /// NULL 前的判空）。
    pub fn is_empty(&self) -> bool {
        self.context_window.is_none()
    }

    /// 序列化为存储字符串；空袋归一为 `None`（列存 NULL）。
    pub fn to_storage(&self) -> Option<String> {
        if self.is_empty() {
            None
        } else {
            serde_json::to_string(self).ok()
        }
    }

    /// 从存储字符串解析；NULL/非法 JSON 一律视为无覆盖。非法值只可能
    /// 来自手工改库——写侧（json_valid 兜底）下次写入即自愈，这里只
    /// debug 记录（resolve_model 每 turn 读，warn 会刷屏）。
    pub fn from_storage(raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        match serde_json::from_str::<Self>(raw) {
            Ok(v) if !v.is_empty() => Some(v),
            Ok(_) => None,
            Err(e) => {
                tracing::debug!(error = %e, raw, "ignoring malformed session settings JSON");
                None
            }
        }
    }
}

/// Input for [`SessionStore::create`]. Only `id` is required; the rest
/// defaults to absent. Built via [`NewSession::new`] + struct update syntax:
///
/// ```rust,ignore
/// store.create(NewSession {
///     working_dir: Some("/repo".into()),
///     ..NewSession::new(id)
/// }).await?;
/// ```
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: SessionId,
    pub project_id: Option<crate::types::ProjectId>,
    pub working_dir: Option<String>,
    pub auto_approve_level: Option<String>,
    pub parent_id: Option<SessionId>,
    pub model_key: Option<String>,
    pub template: Option<String>,
    /// 建行即写入的覆盖袋（thread 继承等场景）；`None` = 无覆盖。
    pub settings: Option<SessionOverrides>,
}

impl NewSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            project_id: None,
            working_dir: None,
            auto_approve_level: None,
            parent_id: None,
            model_key: None,
            template: None,
            settings: None,
        }
    }
}

impl SessionInfo {
    /// Format the age of the session as a human-readable string
    pub fn format_age(&self) -> String {
        format_age(self.updated_at)
    }
}

/// Format a timestamp as a relative age ("2d ago", "3h ago", "5m ago",
/// "just now").
pub fn format_age(ts: DateTime<Utc>) -> String {
    let age = Utc::now() - ts;
    if age.num_days() > 0 {
        format!("{}d ago", age.num_days())
    } else if age.num_hours() > 0 {
        format!("{}h ago", age.num_hours())
    } else if age.num_minutes() > 0 {
        format!("{}m ago", age.num_minutes())
    } else {
        "just now".to_string()
    }
}

/// Storage for session lifecycle and metadata
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new session from the given input.
    async fn create(&self, input: NewSession) -> Result<()>;

    /// Fork a session, copying its metadata (including `auto_approve_level` and `model_key`)
    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId>;

    /// Update session `model_key`
    async fn update_model_key(&self, id: &SessionId, key: &str) -> Result<u64>;

    /// Clear the session's model override (`model_key = NULL` — back to
    /// following the configured default).
    async fn clear_model_key(&self, id: &SessionId) -> Result<u64>;

    /// Set one key in the session's settings bag (atomic `json_set` —
    /// concurrent writers of DIFFERENT keys never clobber each other).
    /// `key` 须为 kernel 侧白名单常量（当前仅 `context_window`）。
    async fn set_setting(&self, id: &SessionId, key: &str, value: serde_json::Value)
        -> Result<u64>;

    /// Remove one key from the settings bag (atomic `json_remove`)；袋空
    /// 归一为 NULL（与 `SessionOverrides::to_storage` 的约定一致）。
    async fn remove_setting(&self, id: &SessionId, key: &str) -> Result<u64>;

    /// Get session metadata by ID
    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<()>;

    /// List sessions with cursor-based pagination.
    /// `project_id` = None returns all sessions (including independent ones).
    /// Returns `(sessions, next_cursor)` where `next_cursor` is the `updated_at` of the last
    /// session if there are more pages, or None if this is the last page.
    async fn list(
        &self,
        project_id: Option<&crate::types::ProjectId>,
        scope: SessionListScope,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> Result<(Vec<SessionInfo>, Option<String>)>;

    /// List direct subagent children of a parent session, newest first.
    async fn list_subagents(&self, parent_id: &SessionId) -> Result<Vec<SessionInfo>>;

    /// Update message count for a session
    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()>;

    /// Touch a session (refresh `updated_at` to now) — called on user
    /// activity so session lists order by real recency.
    async fn touch(&self, id: &SessionId) -> Result<()>;

    /// Update session title
    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()>;

    /// Update session `auto_approve_level`
    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<u64>;

    /// List expired session IDs: `updated_at` older than `cutoff`.
    ///
    /// The returned set includes:
    /// - regular (non-subagent) expired sessions
    /// - child subagent sessions of those expired parents (regardless of own age)
    /// - orphaned subagent sessions (`parent_id IS NULL`) that are themselves expired
    ///
    /// Subagent sessions whose parent is still alive are never returned.
    /// When `keep_pinned` is true, pinned sessions (and their children) are excluded.
    async fn list_expired(
        &self,
        cutoff: DateTime<Utc>,
        keep_pinned: bool,
    ) -> Result<Vec<SessionId>>;

    /// Delete sessions by ID in batches. Returns the number of rows deleted.
    async fn delete_batch(&self, ids: &[SessionId]) -> Result<u64>;

    /// List all session IDs belonging to a project, including subagent
    /// children of those sessions (used by project cascade deletion).
    async fn list_ids_by_project(
        &self,
        project_id: &crate::types::ProjectId,
    ) -> Result<Vec<SessionId>>;
}

pub(crate) use crate::storage::storage_err;

pub mod sqlite;
pub use sqlite::SqliteSessionStore;
