use crate::permission::Level;
use crate::types::{ContentBlock, Result as KernelResult, SessionId};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) mod utils;
pub(crate) use utils::MAX_RETRY_DELAY;

/// Channel-level error type
#[derive(Debug, thiserror::Error, Clone)]
pub enum ChannelError {
    #[error("Channel config error: {0}")]
    Config(String),
    #[error("Channel storage error: {0}")]
    Storage(String),
    #[error("Platform API error: {0}")]
    Platform(String),
    #[error("Access denied for chat {chat_id} (user {user_id})")]
    AccessDenied { chat_id: String, user_id: String },
    #[error("Channel {0} is disabled")]
    Disabled(String),
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
    /// Status card + run receipts for run observability.
    /// When disabled, channels behave as before (ack reaction + final reply).
    #[serde(default = "default_observability")]
    pub observability: bool,
    /// Attach the run trace (tool calls + intermediate texts) to the final
    /// reply bubble — a collapsible panel on card-capable platforms
    /// (Feishu, requires client V7.9+), plain-text lines elsewhere.
    #[serde(default = "default_tool_trace")]
    pub tool_trace: bool,
}

fn default_tool_trace() -> bool {
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlatformConfig {
    Telegram { token: String },
    Feishu { app_id: String, app_secret: String },
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
    /// Thread ID for platforms that support threaded conversations (e.g. Feishu).
    /// When present, the hub uses this as the session mapping key instead of
    /// `external_chat_id` so that each thread gets its own session.
    pub thread_id: Option<String>,
    /// Root message ID of the reply chain (e.g. Feishu `root_id`). In a
    /// thread, every message replies to the thread's root message, so this
    /// identifies the message that started the thread.
    pub root_id: Option<String>,
    /// Whether the message was sent in a group chat (vs. private/p2p).
    pub is_group: bool,
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
    /// Received messages are sent through `incoming`.
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelMessage>,
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
}

// ── Internal helper: access control ──────────────────────────────────

impl ChannelConfig {
    pub fn check_access(&self, chat_id: &str, user_id: &str) -> Result<(), ChannelError> {
        if !self.enabled {
            return Err(ChannelError::Disabled(self.name.clone()));
        }

        // Blocklist wins over allowlist
        if self.blocked_chats.contains(&chat_id.to_string())
            || self.blocked_users.contains(&user_id.to_string())
        {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
            });
        }

        // If allowlist is specified, check it
        if !self.allowed_chats.is_empty() && !self.allowed_chats.contains(&chat_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
            });
        }
        if !self.allowed_users.is_empty() && !self.allowed_users.contains(&user_id.to_string()) {
            return Err(ChannelError::AccessDenied {
                chat_id: chat_id.to_string(),
                user_id: user_id.to_string(),
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
