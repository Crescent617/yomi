use crate::permission::Level;
use crate::types::{ContentBlock, Result as KernelResult, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) mod utils;
pub(crate) use utils::MAX_RETRY_DELAY;

pub(crate) mod attachments;

pub(crate) mod approval;

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
}

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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlatformConfig {
    Telegram { token: String },
    Feishu { app_id: String, app_secret: String },
}

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

    /// Reaction shown when an addressed message is rejected by the
    /// allowlist. A soft 🙏 ("sorry, no") rather than a harsh ✖/👎.
    pub(crate) fn access_denied_reaction(&self) -> &'static str {
        match self {
            Self::Feishu { .. } => "THANKS",
            Self::Telegram { .. } => "🙏",
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

/// Runtime routing info for a session that belongs to an external channel.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRouting {
    pub channel_name: String,
    pub external_chat_id: String,
    pub reply_msg_id: Option<String>,
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

pub(crate) mod obs;

pub(crate) mod reply;

pub mod hub;

pub mod telegram;

pub mod feishu;

#[cfg(test)]
mod tests;
