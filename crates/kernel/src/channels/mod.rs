//! External channel adapters (Feishu/Telegram/doc comments) and the hub
//! that routes their messages into kernel sessions. Feishu card design
//! rules (button size, copy language, client rendering quirks) live in
//! the module docs of `platform/feishu.rs` — read them before touching
//! any card rendering code (`render/obs`, `render/reply`, `cards/mailbox`,
//! `cards/approval`, `hub/deliver`).

use crate::permission::Level;
use crate::types::{ContentBlock, Result as KernelResult, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) mod utils;
pub(crate) use utils::MAX_RETRY_DELAY;

pub(crate) mod attachments;

pub(crate) mod comment;

// 目录分组（2026-08-22）：源文件按职责落子目录，模块路径经再导出
// 与旧平铺完全一致（`crate::channels::X` 零变化）。
pub(crate) mod cards;
pub(crate) mod platform;
pub(crate) mod render;

pub mod hub;

pub(crate) use cards::{approval, ask, cron_card, mailbox, settings};
pub(crate) use platform::{feishu, feishu_events, feishu_text, telegram};
pub(crate) use render::{obs, reply};

/// Why a channel message was rejected by access control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDeniedReason {
    BlockedChat,
    BlockedUser,
    ChatNotAllowed,
    UserNotAllowed,
}

impl std::fmt::Display for AccessDeniedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::BlockedChat => "chat is blocked",
            Self::BlockedUser => "user is blocked",
            Self::ChatNotAllowed => "chat not in allowed_chats",
            Self::UserNotAllowed => "user not in allowed_users",
        };
        f.write_str(s)
    }
}

/// Channel-level error type
#[derive(Debug, thiserror::Error, Clone)]
pub enum ChannelError {
    #[error("Channel config error: {0}")]
    Config(String),
    #[error("Channel storage error: {0}")]
    Storage(String),
    #[error("Platform API error: {0}")]
    Platform(String),
    #[error("Access denied for chat {chat_id} (user {user_id}): {reason}")]
    AccessDenied {
        chat_id: String,
        user_id: String,
        reason: AccessDeniedReason,
    },
    #[error("Channel {0} is disabled")]
    Disabled(String),
}

impl ChannelError {
    /// Allowlist misses get visible feedback (a reaction on the triggering
    /// message); blocklist hits and disabled channels stay silent.
    pub fn is_allowlist_miss(&self) -> bool {
        matches!(
            self,
            Self::AccessDenied {
                reason: AccessDeniedReason::ChatNotAllowed | AccessDeniedReason::UserNotAllowed,
                ..
            }
        )
    }
}

impl From<ChannelError> for crate::types::KernelError {
    fn from(e: ChannelError) -> Self {
        match e {
            ChannelError::AccessDenied { .. } => {
                crate::types::KernelError::Permission(e.to_string())
            }
            ChannelError::Config(ref msg) | ChannelError::Disabled(ref msg) => {
                crate::types::KernelError::Config(msg.clone())
            }
            ChannelError::Storage(ref msg) => crate::types::KernelError::Storage(msg.clone()),
            ChannelError::Platform(ref msg) => crate::types::KernelError::Tool(msg.clone()),
        }
    }
}

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelStatus {
    Idle,
    Connecting,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::struct_excessive_bools)] // feature toggles are naturally bools
pub struct ChannelConfig {
    pub name: String,
    pub enabled: bool,
    pub platform: PlatformConfig,
    #[serde(default)]
    pub allowed_chats: Vec<String>,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub blocked_chats: Vec<String>,
    #[serde(default)]
    pub blocked_users: Vec<String>,
    #[serde(default = "default_require_mention")]
    pub require_mention: bool,
    /// When enabled, group-chat replies are anchored to the triggering
    /// message so they land in its thread (Feishu thread reply, Telegram
    /// quote-reply). Private chats are unaffected.
    #[serde(default)]
    pub reply_in_thread: bool,
    #[serde(default)]
    pub auto_approve_level: Level,
    /// Status card + run receipts for run observability. When disabled,
    /// no status card or run receipts are shown; reply buffering (only the
    /// last assistant text becomes the reply bubble) still applies.
    #[serde(default = "default_observability")]
    pub observability: bool,
    /// Attach the run trace (tool calls + intermediate texts) to the final
    /// reply bubble — a collapsible panel on card-capable platforms
    /// (Feishu, requires client V7.9+), plain-text lines elsewhere.
    #[serde(default = "default_tool_trace")]
    pub tool_trace: bool,
    /// Mid-run split: when the user posts messages while a run is in
    /// flight, freeze the status card in place as a terminal receipt and
    /// deliver the final reply as a NEW message below them (carrying the
    /// run trace). When disabled, the status card always morphs into the
    /// final reply in place (one message per run), leaving the answer
    /// above the user's mid-run messages.
    #[serde(default = "default_mid_run_split")]
    pub mid_run_split: bool,
    /// Recent-chat messages injected as context when the bot is triggered
    /// in a group (fetched since the last trigger in that thread/chat,
    /// newest cap). 0 disables.
    #[serde(default = "default_history_context")]
    pub history_context: usize,
    /// Target group chat for doc-permission application notifications.
    /// When unset, each of `admin_users` is notified by DM instead; when
    /// both are unset the feature is off (applications are only logged).
    #[serde(default)]
    pub approval_chat_id: Option<String>,
    /// `open_id`s allowed to approve/deny doc-permission applications
    /// (buttons and `/approve` `/deny` `/permits` commands alike). Also the
    /// DM recipients when `approval_chat_id` is unset.
    #[serde(default)]
    pub admin_users: Vec<String>,
    /// Runtime kill-switch for platform event features (the vocabulary is
    /// per-platform, see [`PlatformConfig::known_event_names`]); unset =
    /// all enabled. Event delivery itself requires console-side
    /// subscription — this disables *reacting* without a console trip.
    #[serde(default)]
    pub disabled_events: Vec<String>,
}

/// Event feature: Feishu doc comments (`drive.notice.comment_add_v1`).
pub(crate) const EVENT_DOC_COMMENT: &str = "doc_comment";

fn default_history_context() -> usize {
    20
}

fn default_tool_trace() -> bool {
    true
}

fn default_mid_run_split() -> bool {
    true
}

fn default_observability() -> bool {
    true
}

fn default_require_mention() -> bool {
    true
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: false,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            allowed_chats: Vec::new(),
            allowed_users: Vec::new(),
            blocked_chats: Vec::new(),
            blocked_users: Vec::new(),
            require_mention: true,
            reply_in_thread: false,
            auto_approve_level: Level::Safe,
            observability: true,
            tool_trace: true,
            mid_run_split: true,
            history_context: default_history_context(),
            approval_chat_id: None,
            admin_users: Vec::new(),
            disabled_events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlatformConfig {
    Telegram { token: String },
    Feishu { app_id: String, app_secret: String },
}

/// Default platform for CLI channel selection (currently the only
/// thread-capable one). Shared by the CLI flag and the kernel-side
/// Local/remote defaults so they can't drift.
pub const DEFAULT_PLATFORM: &str = "feishu";

impl PlatformConfig {
    /// Ack reaction for a message accepted for processing. Values are
    /// platform-specific emoji identifiers (Feishu `emoji_type`, Telegram
    /// unicode emoji).
    pub(crate) fn ack_reaction(&self) -> &'static str {
        match self {
            Self::Feishu { .. } => "OneSecond",
            Self::Telegram { .. } => "👀",
        }
    }

    /// Case-insensitive platform-name match (`feishu` / `telegram`) for
    /// CLI-side channel selection.
    pub(crate) fn name_is(&self, name: &str) -> bool {
        match self {
            Self::Feishu { .. } => name.eq_ignore_ascii_case("feishu"),
            Self::Telegram { .. } => name.eq_ignore_ascii_case("telegram"),
        }
    }

    /// Ack reaction for a `/queue`d message — "noted, queued for later".
    /// Deliberately not the run-trigger ack: `OneSecond`/"👀" promises
    /// imminent processing, which a queue makes no claim about. (Feishu
    /// reaction candidates probed live: Hourglass/Bookmark/Pushpin/
    /// InboxTray are all rejected as invalid `emoji_type`.)
    pub(crate) fn queue_reaction(&self) -> &'static str {
        match self {
            Self::Feishu { .. } => "Get",
            Self::Telegram { .. } => "👌",
        }
    }

    /// Reaction shown when an addressed message is rejected by the
    /// allowlist. A soft 🙏 ("sorry, no") rather than a harsh ✖/👎.
    pub(crate) fn access_denied_reaction(&self) -> &'static str {
        match self {
            Self::Feishu { .. } => "THANKS",
            Self::Telegram { .. } => "🙏",
        }
    }

    /// Event feature names this platform understands — the valid
    /// vocabulary of `ChannelConfig::disabled_events` (startup validation
    /// only warns on unknown names; serde can't reject array contents).
    pub(crate) fn known_event_names(&self) -> &'static [&'static str] {
        match self {
            Self::Feishu { .. } => &[EVENT_DOC_COMMENT],
            Self::Telegram { .. } => &[],
        }
    }
}

/// Platform-independent message from an external chat platform
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub external_chat_id: String,
    pub external_user_id: String,
    pub external_message_id: Option<String>,
    pub is_mention: bool,
    /// Raw platform text used for command parsing, before model-context metadata is added.
    pub raw_text: Option<String>,
    pub content: Vec<ContentBlock>,
    /// Opaque platform keys of images attached to this message (Feishu
    /// `image_key`, Telegram photo `file_id`). Adapters must NOT download
    /// them eagerly — the hub downloads via
    /// [`PlatformAdapter::download_message_image`] only after the message
    /// passes the gate, so gated-out group chatter costs no bandwidth.
    pub image_keys: Vec<String>,
    /// Thread ID for platforms that support threaded conversations (e.g. Feishu).
    /// When present, the hub uses this as the session mapping key instead of
    /// `external_chat_id` so that each thread gets its own session.
    pub thread_id: Option<String>,
    /// Root message ID of the reply chain (e.g. Feishu `root_id`). In a
    /// thread, every message replies to the thread's root message, so this
    /// identifies the message that started the thread. Plain quote-replies
    /// outside any thread carry it too — the hub only uses it for session
    /// mapping when `thread_id` is also present.
    pub root_id: Option<String>,
    /// The message this message directly quote-replies to (Feishu
    /// `parent_id`). Distinct from `root_id` (the chain root): this is the
    /// specific message the user pointed at. The hub fetches its content
    /// post-gate for quoted-message context injection.
    pub parent_id: Option<String>,
    /// Whether the message was sent in a group chat (vs. private/p2p).
    pub is_group: bool,
    /// The platform's creation timestamp in unix **milliseconds** (Feishu;
    /// `None` on platforms that don't provide one). Used to advance the
    /// history cursor on every processed message.
    pub create_time: Option<i64>,
    /// Doc-comment provenance: this message was assembled from a Feishu doc
    /// comment (see `comment.rs`) rather than a chat message. Drives the
    /// per-comment-thread session mapping key and the doc-bound reply
    /// delivery; the chat-scoped fields (`external_chat_id`, thread/quote
    /// ids) are empty for such messages.
    pub doc_comment: Option<DocCommentRef>,
}

/// Platform inbound payload: a chat message, a platform event, or a card
/// button callback. Platform events and callbacks bypass access control
/// and the mention requirement (they are not user chat messages).
#[derive(Debug, Clone)]
pub enum ChannelEvent {
    Message(ChannelMessage),
    /// Feishu `drive.file.permission_member_applied_v1`: someone requested
    /// collaborator access to an app-owned document.
    DocPermissionApplied(DocPermissionRequest),
    /// Feishu `card.action.trigger`: a button tap on a notification card.
    /// The button's `value` carries the approval action and request id.
    CardAction(CardAction),
    /// Feishu `drive.notice.comment_add_v1`: a doc comment @-mentioned the
    /// bot. Carries ids only — the comment content is fetched post-policy
    /// (same deferred pattern as `ChannelMessage::image_keys`), so
    /// filtered-out events cost no platform API calls.
    DocCommentAdded(DocCommentNotice),
}

/// A Feishu `drive.notice.comment_add_v1` event (ids only).
#[derive(Debug, Clone)]
pub struct DocCommentNotice {
    pub file_token: String,
    /// docx/sheet/bitable/...
    pub file_type: String,
    pub comment_id: String,
    /// The triggering reply inside the comment thread (`None` only on
    /// older/unexpected event shapes — `add_comment` events carry the
    /// first reply's id too).
    pub reply_id: Option<String>,
    /// `notice_meta.from_user_id.open_id` — the comment's author.
    pub commenter_open_id: String,
    /// Whether the comment @-mentioned the bot (the trigger condition).
    pub is_mentioned: bool,
    /// `add_comment` / `add_reply` (other values are filtered out).
    pub notice_type: String,
    /// `header.create_time`, unix **milliseconds**.
    pub create_time: Option<i64>,
}

/// Doc-comment provenance/routing: which comment thread a message came
/// from and where the session's replies go. (The triggering reply id
/// lives only on `DocCommentNotice` and in the meta header — delivery
/// always targets the comment thread.)
///
/// For **whole-document comments** the `comment_id` is the
/// [`WHOLE_COMMENT_ID`] sentinel instead of a real id: every whole
/// comment of one document shares a single session (the doc's bottom
/// comment area behaves like one conversation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocCommentRef {
    pub file_token: String,
    pub file_type: String,
    pub comment_id: String,
}

/// A fetched doc comment (`batch_query` covers whole and partial comments
/// alike — the single-comment GET serves whole comments only).
#[derive(Debug, Clone)]
pub struct DocCommentDetail {
    /// Whole-document comment (vs. partial/anchored to a text selection).
    /// `None` when `batch_query` has not caught up with the event yet
    /// (read lag) — the session-mapping decision needs this, so the
    /// caller retries while it is unknown.
    pub is_whole: Option<bool>,
    /// The quoted source text (partial comments only).
    pub quote: Option<String>,
    /// The comment thread's replies, in thread order.
    pub replies: Vec<DocCommentReplyLite>,
}

/// One reply inside a doc comment thread, text already extracted.
#[derive(Debug, Clone)]
pub struct DocCommentReplyLite {
    pub reply_id: String,
    /// Commenter `open_id`.
    pub user_id: String,
    /// Unix seconds (the comment API's native precision).
    pub create_time: i64,
    pub text: String,
    /// Authored by the bot itself — excluded from injected thread history
    /// (those turns are already in the session as assistant messages).
    pub is_from_bot: bool,
}

/// Sentinel `comment_id` segment for the shared whole-document-comment
/// session: whole comments (the doc's bottom comment area) each get a
/// fresh platform comment thread, but they belong to ONE session per
/// document. Platform comment ids are numeric, so "whole" never
/// collides. Delivery resolves it to posting a new whole comment (whole
/// comments take no thread replies — platform error 1069302).
pub(crate) const WHOLE_COMMENT_ID: &str = "whole";

/// The session mapping key for a doc-comment session:
/// `doc:{file_type}:{file_token}:{comment_id}` — one session per comment
/// thread, or per **document** for whole comments (the `comment_id`
/// segment is then the [`WHOLE_COMMENT_ID`] sentinel). The key doubles
/// as the persisted delivery target: `find_routing_by_session` parses it
/// back (no extra schema column). None of the segments can contain `:`
/// (platform ids are alphanumeric).
pub(crate) fn doc_comment_mapping_key(
    file_type: &str,
    file_token: &str,
    comment_id: &str,
) -> String {
    format!("doc:{file_type}:{file_token}:{comment_id}")
}

/// Parse a doc-comment mapping key back into the delivery target. Strict
/// four-segment shape; anything else is a plain chat/thread key → `None`.
/// The `comment_id` may be the [`WHOLE_COMMENT_ID`] sentinel (the shared
/// whole-comment session) — delivery handles it.
pub(crate) fn parse_doc_comment_mapping_key(key: &str) -> Option<DocCommentRef> {
    let mut parts = key.split(':');
    let (Some("doc"), Some(file_type), Some(file_token), Some(comment_id), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if file_type.is_empty() || file_token.is_empty() || comment_id.is_empty() {
        return None;
    }
    Some(DocCommentRef {
        file_token: file_token.to_string(),
        file_type: file_type.to_string(),
        comment_id: comment_id.to_string(),
    })
}

/// A Feishu cloud document's web URL (`https://feishu.cn/{file_type}/{file_token}` —
/// docx/sheet/bitable/... are isomorphic).
pub(crate) fn doc_link(file_type: &str, file_token: &str) -> String {
    format!("https://feishu.cn/{file_type}/{file_token}")
}

/// A Feishu cloud-document collaborator-permission application. The
/// applicant can be any mix of users / chats / departments.
#[derive(Debug, Clone)]
pub struct DocPermissionRequest {
    pub file_token: String,
    /// docx/sheet/bitable/...
    pub file_type: String,
    /// `view` / `edit` / `full_access`
    pub permission: String,
    pub remark: Option<String>,
    /// `open_id` list.
    pub applicant_users: Vec<String>,
    /// `chat_id` list.
    pub applicant_chats: Vec<String>,
    /// department id list.
    pub applicant_departments: Vec<String>,
}

/// A card button callback (`card.action.trigger`).
#[derive(Debug, Clone)]
pub struct CardAction {
    pub operator_open_id: String,
    /// Chat the callback happened in (for feedback messages), when the
    /// platform provides it.
    pub chat_id: Option<String>,
    /// Message the button card lives in (for in-place card refresh),
    /// when the platform provides it.
    pub message_id: Option<String>,
    /// Button value, e.g. `{"action": "approve"|"deny", "id": N}`.
    pub value: serde_json::Value,
}

/// Runtime info about a channel, for UI listing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChannelInfo {
    pub name: String,
    pub status: ChannelStatus,
}

/// The kind of a channel session mapping (the `kind` column).
///
/// `Watch`/`WatchPaused` mark a chat's observer session (see `/watch`
/// and `hub/watch.rs`): it receives a mirror of every message while on,
/// but the channel delivers NOTHING for it — the flag is checked at
/// every exit where the channel would speak for a session (event-
/// forwarder dispatch, and thereby cards/typing/settle/notify).
/// `/watch off` flips `Watch` → `WatchPaused`: the row, the session and
/// its context stay put (a later `/watch on` resumes the same observer);
/// only the mirror tap closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingKind {
    Normal,
    Watch,
    WatchPaused,
}

impl MappingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Watch => "watch",
            Self::WatchPaused => "watch_off",
        }
    }

    /// DB parse: unknown/absent values degrade to `Normal` (rows written
    /// before the column existed carry the `'normal'` default anyway).
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "watch" => Self::Watch,
            "watch_off" => Self::WatchPaused,
            _ => Self::Normal,
        }
    }
}

/// Mapping-key prefix of a chat's watch-observer session
/// (`watch:{chat_id}`) — a private namespace that can never collide with
/// platform ids (`om_…`/`oc_…`).
pub const WATCH_KEY_PREFIX: &str = "watch:";

/// The observer session's mapping key for a watched chat.
pub fn watch_mapping_key(chat_id: &str) -> String {
    format!("{WATCH_KEY_PREFIX}{chat_id}")
}

/// Runtime routing info for a session that belongs to an external channel.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRouting {
    pub channel_name: String,
    pub external_chat_id: String,
    pub reply_msg_id: Option<String>,
    /// The session's mapping key (the conversation-scope key the mapping is
    /// stored under): the chat id for chat-level sessions, the thread
    /// root/thread id for thread sessions, the `doc:…` comment-thread key
    /// for doc-comment sessions. Used to match run subscriptions.
    pub mapping_key: String,
    /// Delivery target for doc-comment sessions (parsed from
    /// `mapping_key`); `None` for ordinary chat-routed sessions.
    pub doc_comment: Option<DocCommentRef>,
    /// Normal conversation session or watch observer (see [`MappingKind`]).
    pub kind: MappingKind,
}

impl SessionRouting {
    /// Watch observers (on or paused) get no channel delivery of any kind.
    pub fn is_watch(&self) -> bool {
        matches!(self.kind, MappingKind::Watch | MappingKind::WatchPaused)
    }
}

// ── Store trait ──────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait ChannelStore: Send + Sync {
    async fn save_mapping(
        &self,
        channel_name: &str,
        mapping_key: &str,
        session_id: &SessionId,
        actual_chat_id: &str,
        reply_msg_id: Option<&str>,
        kind: MappingKind,
    ) -> KernelResult<()>;

    async fn find_mapping(
        &self,
        channel_name: &str,
        mapping_key: &str,
    ) -> KernelResult<Option<SessionId>>;

    async fn list_mappings(&self, channel_name: &str) -> KernelResult<Vec<(String, SessionId)>>;

    /// Find routing info for a session (actual `chat_id` and `reply_msg_id`).
    async fn find_routing_by_session(
        &self,
        session_id: &SessionId,
    ) -> KernelResult<Option<SessionRouting>>;

    /// Delete a channel session mapping
    async fn delete_mapping(&self, channel_name: &str, mapping_key: &str) -> KernelResult<()>;

    /// Delete all mappings belonging to the given sessions (used by gc).
    /// Returns the number of rows deleted.
    async fn delete_by_sessions(&self, session_ids: &[SessionId]) -> KernelResult<u64>;

    /// The history cursor for a container (thread or chat): the
    /// `create_time` (unix **milliseconds**, same precision as the
    /// platform) of the message last consumed as agent context. `None` =
    /// never consumed (first trigger).
    async fn get_history_cursor(
        &self,
        channel_name: &str,
        container_id: &str,
    ) -> KernelResult<Option<i64>> {
        let _ = (channel_name, container_id);
        Ok(None)
    }

    /// Advance the history cursor after a successful fetch+delivery.
    async fn set_history_cursor(
        &self,
        channel_name: &str,
        container_id: &str,
        cursor_ts: i64,
    ) -> KernelResult<()> {
        let _ = (channel_name, container_id, cursor_ts);
        Ok(())
    }

    /// The conversation-level `require_mention` override for a container
    /// (thread or chat), set at runtime via `/mention on|off`. `None` =
    /// no override (the parent scope or channel config applies).
    async fn get_mention_override(
        &self,
        channel_name: &str,
        container_id: &str,
    ) -> KernelResult<Option<bool>> {
        let _ = (channel_name, container_id);
        Ok(None)
    }

    /// Set or replace the container's `require_mention` override.
    async fn set_mention_override(
        &self,
        channel_name: &str,
        container_id: &str,
        require_mention: bool,
    ) -> KernelResult<()> {
        let _ = (channel_name, container_id, require_mention);
        Ok(())
    }

    /// Remove the container's override, falling back to the parent scope
    /// (thread → chat → channel config).
    async fn clear_mention_override(
        &self,
        channel_name: &str,
        container_id: &str,
    ) -> KernelResult<()> {
        let _ = (channel_name, container_id);
        Ok(())
    }

    /// The chat-level `reply_in_thread` override, set at runtime via
    /// `/threads on|off`. `None` = no override (the channel config
    /// applies). Chat-scoped only: threads are a product of the mode,
    /// not an override target.
    async fn get_rit_override(
        &self,
        channel_name: &str,
        chat_id: &str,
    ) -> KernelResult<Option<bool>> {
        let _ = (channel_name, chat_id);
        Ok(None)
    }

    /// Set or replace the chat's `reply_in_thread` override.
    async fn set_rit_override(
        &self,
        channel_name: &str,
        chat_id: &str,
        reply_in_thread: bool,
    ) -> KernelResult<()> {
        let _ = (channel_name, chat_id, reply_in_thread);
        Ok(())
    }

    /// Remove the chat's override, falling back to the channel config.
    async fn clear_rit_override(&self, channel_name: &str, chat_id: &str) -> KernelResult<()> {
        let _ = (channel_name, chat_id);
        Ok(())
    }

    /// The watch state of a chat: the `kind` of its `watch:{chat_id}`
    /// mapping — `Some(Watch)` while on, `Some(WatchPaused)` after
    /// `/watch off` (row and session kept for resume), `None` when the
    /// chat has never been watched (or the observer was gc'd).
    async fn get_watch_state(
        &self,
        channel_name: &str,
        chat_id: &str,
    ) -> KernelResult<Option<MappingKind>> {
        let _ = (channel_name, chat_id);
        Ok(None)
    }

    /// Persist a doc-permission application as a pending approval row.
    /// Deduplicates on (channel, file, permission, applicant sets) while a
    /// pending row exists — ws redelivery carries no unique event id, so
    /// this is the best dedup key. Returns `None` on a duplicate.
    async fn save_perm_request(
        &self,
        channel_name: &str,
        req: &DocPermissionRequest,
    ) -> KernelResult<Option<i64>>;

    /// Record the notification card message ids for later terminal-state
    /// updates (one id in group mode, one per admin in DM mode).
    async fn set_perm_notify_msgs(&self, id: i64, msg_ids: &[String]) -> KernelResult<()>;

    /// List a channel's pending applications, oldest first.
    async fn list_pending_perm_requests(
        &self,
        channel_name: &str,
    ) -> KernelResult<Vec<PermRequestRow>>;

    /// Conditionally flip a pending request to `status` ("approved" /
    /// "denied"), returning the row only when this call won the race —
    /// concurrent resolutions (buttons and commands alike) take effect
    /// exactly once.
    async fn resolve_perm_request(
        &self,
        id: i64,
        status: &str,
        resolved_by: &str,
        resolved_perm: Option<&str>,
    ) -> KernelResult<Option<PermRequestRow>>;

    /// Reopen a request whose grant API call failed after winning the
    /// resolve race: back to pending, resolution fields cleared.
    async fn reopen_perm_request(&self, id: i64) -> KernelResult<()>;

    /// Subscribe a user to run-completion notifications for a conversation
    /// scope (`scope_key` = mapping key: chat id for chat level, thread
    /// key for threads). `target_chat_id = None` notifies the subscriber
    /// by DM. Upserts on (channel, scope, subscriber).
    async fn save_run_subscription(
        &self,
        channel_name: &str,
        scope_key: &str,
        chat_id: &str,
        recursive: bool,
        subscriber_open_id: &str,
        target_chat_id: Option<&str>,
    ) -> KernelResult<()>;

    /// Remove a user's subscription for a scope; returns rows deleted.
    async fn remove_run_subscription(
        &self,
        channel_name: &str,
        scope_key: &str,
        subscriber_open_id: &str,
    ) -> KernelResult<u64>;

    /// Subscriptions matching a finished run: exact scope match
    /// (`scope_key` = the run's mapping key) plus recursive chat-level
    /// subscriptions (`chat_id` = the run's actual chat).
    async fn list_matching_run_subscriptions(
        &self,
        channel_name: &str,
        scope_key: &str,
        chat_id: &str,
    ) -> KernelResult<Vec<RunSubscriptionRow>>;
}

/// A persisted doc-permission application row.
#[derive(Debug, Clone)]
pub struct PermRequestRow {
    pub id: i64,
    pub channel_name: String,
    pub file_token: String,
    pub file_type: String,
    pub permission: String,
    pub remark: Option<String>,
    pub applicant_users: Vec<String>,
    pub applicant_chats: Vec<String>,
    pub applicant_departments: Vec<String>,
    pub status: String,
    pub notify_msg_ids: Vec<String>,
    pub resolved_by: Option<String>,
    pub resolved_perm: Option<String>,
    pub created_at: String,
}

/// A persisted run-completion subscription row.
#[derive(Debug, Clone)]
pub struct RunSubscriptionRow {
    pub id: i64,
    pub channel_name: String,
    /// The subscribed conversation scope (mapping key: chat id at chat
    /// level, thread key in threads).
    pub scope_key: String,
    /// The chat the scope belongs to (for recursive matching).
    pub chat_id: String,
    /// Chat-level subscriptions only: also match runs in this chat's
    /// threads.
    pub recursive: bool,
    pub subscriber_open_id: String,
    /// Where to send the notification; `None` = DM the subscriber.
    pub target_chat_id: Option<String>,
    pub created_at: String,
}

// ── Platform adapter trait ─────────────────────────────────────────

/// Platform adapter: converts between external platform protocol and kernel
/// `ContentBlocks`.  Business logic (session management, access control) lives
/// in `ChannelManager`; the adapter only does protocol conversion and I/O.
#[async_trait::async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// Start receiving messages from the platform.
    ///
    /// This should run until `cancel` is triggered.
    /// Received messages and platform events are sent through `incoming`.
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError>;

    /// Send a message (text, image, etc.) back to the platform.
    ///
    /// `reply_msg_id` is the original message ID to reply to. For Feishu, this
    /// is used with the reply API to place the response in the same thread.
    ///
    /// Returns the platform message ID of the sent message when available
    /// (used by observability to react on the last content message).
    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError>;

    /// Send a raw card message (platform-specific card JSON), returning its
    /// message ID for later [`update_card`](Self::update_card) calls.
    ///
    /// Default implementation returns `Ok(None)` — platforms without card
    /// support simply skip status cards.
    async fn send_card(
        &self,
        _external_chat_id: &str,
        _card_json: &str,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        Ok(None)
    }

    /// Send a raw card message directly to a user (DM / p2p), returning
    /// its message ID for later [`update_card`](Self::update_card) calls.
    /// Default: unsupported on this platform.
    async fn send_direct_card(
        &self,
        _user_id: &str,
        _card_json: &str,
    ) -> Result<Option<String>, ChannelError> {
        Err(ChannelError::Platform(
            "direct card not supported for this platform".into(),
        ))
    }

    /// Grant collaborator permission on an app-owned document to every
    /// applicant of a doc-permission request (users, chats, departments).
    /// Default: unsupported on this platform.
    async fn grant_doc_permission(
        &self,
        _file_token: &str,
        _file_type: &str,
        _req: &DocPermissionRequest,
        _perm: &str,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::Platform(
            "doc permission grant not supported for this platform".into(),
        ))
    }

    /// Fetch a document's display title for notification cards. Default:
    /// `None` — cards fall back to the raw file token.
    async fn fetch_doc_title(&self, _file_token: &str, _file_type: &str) -> Option<String> {
        None
    }

    /// Fetch one doc comment by id (Feishu `batch_query` — covers whole and
    /// partial comments alike; the single-comment GET serves whole
    /// comments only). `Ok(None)` = the comment is gone (deleted).
    /// Default: unsupported.
    async fn fetch_doc_comment(
        &self,
        _file_token: &str,
        _file_type: &str,
        _comment_id: &str,
    ) -> Result<Option<DocCommentDetail>, ChannelError> {
        Ok(None)
    }

    /// Reply to a doc comment thread with one plain-text chunk. Whole
    /// comments can't take thread replies (platform error 1069302) — the
    /// adapter falls back to posting a new whole comment. Chunking is the
    /// caller's job. Returns the created reply/comment id when available.
    /// Default: unsupported.
    async fn reply_doc_comment(
        &self,
        _file_token: &str,
        _file_type: &str,
        _comment_id: &str,
        _text: &str,
    ) -> Result<Option<String>, ChannelError> {
        Err(ChannelError::Platform(
            "doc comment reply not supported for this platform".into(),
        ))
    }

    /// Add a reaction to a doc comment reply (the ack for an accepted
    /// comment trigger; keyed by the reply's id). Best-effort — default:
    /// silent no-op for platforms without the concept.
    async fn react_doc_comment(
        &self,
        _file_token: &str,
        _file_type: &str,
        _reply_id: &str,
        _emoji: &str,
    ) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Update a previously sent card message in place.
    ///
    /// Default implementation does nothing for platforms that don't support it.
    async fn update_card(&self, _message_id: &str, _card_json: &str) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Whether the platform supports status cards (`send_card`/`update_card`).
    /// When false, the hub falls back to typing indicators for run progress.
    fn supports_status_card(&self) -> bool {
        false
    }

    /// Send a reaction (emoji) to a message on the platform.
    ///
    /// `emoji` is a platform-specific emoji identifier (Feishu `emoji_type`).
    /// `external_chat_id` is unused by platforms whose message IDs are
    /// globally unique (Feishu) — callers pass `""` there.
    /// Returns the reaction ID when available (needed for removal).
    /// Default implementation does nothing for platforms that don't support it.
    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<Option<String>, ChannelError> {
        Ok(None)
    }

    /// Remove a reaction previously added by the bot (`reaction_id` from
    /// [`send_reaction`](Self::send_reaction)). Re-adding the same emoji
    /// on the same message is deduplicated by platforms (no new event, no
    /// re-notification), so repeated signals delete-then-re-add instead.
    /// Default implementation does nothing for platforms that don't
    /// support it.
    async fn delete_reaction(
        &self,
        _external_chat_id: &str,
        _message_id: &str,
        _reaction_id: &str,
    ) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Send a typing action to indicate the bot is processing.
    ///
    /// Default implementation does nothing for platforms that don't support it.
    async fn send_typing(&self, _external_chat_id: &str) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Send multiple files to the platform.
    ///
    /// `reply_msg_id` is forwarded the same way as in `send_message`.
    /// Default implementation does nothing for platforms that don't support it yet.
    async fn send_files(
        &self,
        _external_chat_id: &str,
        _files: &[(&std::path::Path, Option<&str>)],
        _reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::Platform(
            "send_files not supported for this platform".into(),
        ))
    }

    /// Fetch recent messages of a container (thread or chat), newest-first
    /// up to `limit` (platform-capped at 50), strictly newer than
    /// `since_ts` (unix **milliseconds**; `None` = fetch the latest page).
    /// Note: one page only — bot/deleted messages filtered after the fetch
    /// can shrink the result below `limit` (no top-up fetch). Used to
    /// inject recent-chat context when the bot is triggered. Default: no
    /// history (platforms without a history API or where it isn't
    /// implemented).
    async fn fetch_history(
        &self,
        _container: &HistoryContainer,
        _since_ts: Option<i64>,
        _limit: usize,
    ) -> Result<Vec<HistoryMessage>, ChannelError> {
        Ok(Vec::new())
    }

    /// Fetch a single message by id, for quoted-reply context injection.
    /// Unlike the history fetch, bot messages are kept — quoting the bot's
    /// own answer is a primary use case. Default: unsupported.
    async fn fetch_message(
        &self,
        _message_id: &str,
    ) -> Result<Option<HistoryMessage>, ChannelError> {
        Ok(None)
    }

    /// The root message id of a platform thread (the message the thread
    /// hangs off). Resolves the canonical thread key (see
    /// `effective_mapping_key`) when the event payload doesn't carry
    /// `root_id`. Implementations should cache successful lookups —
    /// callers may invoke this once per in-thread message. Default:
    /// unsupported (no threads).
    async fn thread_root_id(&self, _thread_id: &str) -> Result<Option<String>, ChannelError> {
        Ok(None)
    }

    /// Build a user-clickable link that jumps to a message in the client
    /// (Feishu applink). May cost one platform API call (the jump link
    /// needs the message's position). Default: unsupported — `None`.
    async fn message_link(&self, _chat_id: &str, _message_id: &str) -> Option<String> {
        None
    }

    /// Build a user-clickable link that jumps to a chat itself (Feishu
    /// applink) — the fallback when a session has no message to anchor
    /// to. Default: unsupported — `None`.
    async fn chat_link(&self, _chat_id: &str) -> Option<String> {
        None
    }

    /// Build a user-clickable link that jumps to the thread containing
    /// `message_id` (no in-thread position — just the conversation).
    /// May cost one platform API call (the thread id is read off the
    /// message). Default: unsupported — `None`.
    async fn thread_link(&self, _chat_id: &str, _message_id: &str) -> Option<String> {
        None
    }

    /// Fetch a chat's display name (for human-friendly notification
    /// text). Best-effort; default: unsupported — `None`.
    async fn fetch_chat_name(&self, _chat_id: &str) -> Option<String> {
        None
    }

    /// Fetch a user's display name by open id (to attribute quoted context
    /// in notifications). Best-effort: deployments without contact
    /// permission simply return `None`. Default: unsupported — `None`.
    async fn fetch_user_name(&self, _open_id: &str) -> Option<String> {
        None
    }

    /// Download one image attached to a message as an `ImageUrl` content
    /// block. `image_key` is opaque and platform-specific: from
    /// [`ChannelMessage::image_keys`] (deferred receive-path download,
    /// post-gate) or [`HistoryMessage::image_keys`] (history injection).
    /// Default: unsupported.
    async fn download_message_image(
        &self,
        _message_id: &str,
        _image_key: &str,
    ) -> Result<ContentBlock, ChannelError> {
        Err(ChannelError::Platform(
            "image download not supported for this platform".into(),
        ))
    }
}

/// A thread or chat a history fetch targets.
#[derive(Debug, Clone)]
pub enum HistoryContainer {
    Chat(String),
    Thread(String),
}

impl HistoryContainer {
    /// The platform id of the container (`chat_id` or `thread_id`).
    pub fn id(&self) -> &str {
        match self {
            Self::Chat(id) | Self::Thread(id) => id,
        }
    }

    /// Human-readable scope label for command replies.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Chat(_) => "chat",
            Self::Thread(_) => "thread",
        }
    }
}

/// One message from a container's history, ready for context assembly.
#[derive(Debug, Clone)]
pub struct HistoryMessage {
    /// Platform message id (used to drop the triggering message itself).
    pub message_id: String,
    /// Unix milliseconds (the platform's native precision — cursors keep
    /// it too, so two messages in one second can't be skipped by a
    /// truncated comparison).
    pub create_time: i64,
    /// Full open id of the sender (attribution).
    pub sender_id: String,
    /// Extracted text (non-text messages become a `[type]` placeholder).
    pub text: String,
    /// Opaque platform keys of images attached to this message (Feishu
    /// `image_key`s from `image` messages and post `img` runs). Each is
    /// downloadable via [`PlatformAdapter::download_message_image`].
    pub image_keys: Vec<String>,
    /// The message this one quote-replies to, when any — lets quoted
    /// injection walk the quote chain (a quoted message's own quoted
    /// context would otherwise be lost).
    pub parent_id: Option<String>,
}

// ── Internal helper: access control ──────────────────────────────────

/// User-level gate for card-button callbacks: button clicks bypass the
/// message gate entirely, so its user rule is re-applied at the hub's
/// card-action router for every button — blocked users are refused;
/// when `allowed_users` is set the operator must be in it (empty
/// allowlist = open). Admin-gated surfaces stack `check_admin` in their
/// handlers. Returns the denial text on refusal.
pub(crate) fn check_user_access(config: &ChannelConfig, user_id: &str) -> Option<String> {
    if config.blocked_users.iter().any(|u| u == user_id) {
        return Some("Permission denied: blocked user.".to_string());
    }
    if !config.allowed_users.is_empty() && !config.allowed_users.iter().any(|u| u == user_id) {
        return Some("Permission denied: not in allowed_users.".to_string());
    }
    None
}

impl ChannelConfig {
    pub fn check_access(&self, chat_id: &str, user_id: &str) -> Result<(), ChannelError> {
        if !self.enabled {
            return Err(ChannelError::Disabled(self.name.clone()));
        }

        // Blocklist wins over allowlist
        if self.blocked_chats.contains(&chat_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
                reason: AccessDeniedReason::BlockedChat,
            });
        }
        if self.blocked_users.contains(&user_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
                reason: AccessDeniedReason::BlockedUser,
            });
        }

        // If allowlist is specified, check it
        if !self.allowed_chats.is_empty() && !self.allowed_chats.contains(&chat_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
                reason: AccessDeniedReason::ChatNotAllowed,
            });
        }
        if !self.allowed_users.is_empty() && !self.allowed_users.contains(&user_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
                reason: AccessDeniedReason::UserNotAllowed,
            });
        }

        Ok(())
    }
}

/// Convert a slice of `ContentBlock`s into a single plain-text string for
/// external platform delivery. Thinking/redacted blocks are stripped to avoid
/// leaking internal reasoning. Non-text blocks (image, audio) are represented as
/// placeholders.
pub fn blocks_to_text(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text } => parts.push(text.clone()),
            // Skip thinking blocks — they are internal model reasoning and must
            // not be sent to end users on Telegram / Feishu.
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            ContentBlock::ImageUrl { image_url } => {
                parts.push(format!("[image: {}]", image_url.url));
            }
            ContentBlock::Audio { audio } => {
                parts.push(format!("[audio: {}]", audio.format));
            }
        }
    }
    parts.join("\n")
}

pub mod store;

pub(crate) use hub::{
    command as hub_command, context as hub_context, deliver as hub_deliver, delivery_pool,
    gate as hub_gate, handlers as hub_handlers, routing as hub_routing, watch as hub_watch,
};

#[cfg(test)]
mod tests;
