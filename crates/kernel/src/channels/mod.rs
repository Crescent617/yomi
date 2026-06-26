use crate::permissions::Level;
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
    #[serde(default)]
    pub auto_approve_level: Level,
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
            auto_approve_level: Level::Safe,
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
    pub content: Vec<ContentBlock>,
}

/// Runtime info about a channel, for UI listing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChannelInfo {
    pub name: String,
    pub status: ChannelStatus,
}

// ── Store trait ──────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait ChannelStore: Send + Sync {
    async fn save_mapping(
        &self,
        channel_name: &str,
        external_chat_id: &str,
        session_id: &SessionId,
    ) -> KernelResult<()>;

    async fn find_mapping(
        &self,
        channel_name: &str,
        external_chat_id: &str,
    ) -> KernelResult<Option<SessionId>>;

    async fn list_mappings(&self, channel_name: &str) -> KernelResult<Vec<(String, SessionId)>>;

    /// Find the channel name and `external_chat_id` for a given `session_id`.
    async fn find_by_session_id(
        &self,
        session_id: &SessionId,
    ) -> KernelResult<Option<(String, String)>>;
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
    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
    ) -> Result<(), ChannelError>;

    /// Send a reaction (emoji) to a message on the platform.
    ///
    /// Default implementation does nothing for platforms that don't support it.
    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        _message_id: &str,
        _emoji: &str,
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
    /// Default implementation does nothing for platforms that don't support it yet.
    async fn send_files(
        &self,
        _external_chat_id: &str,
        _files: &[(&std::path::Path, Option<&str>)],
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

pub mod hub;

pub mod telegram;

pub mod feishu;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_access_disabled() {
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: false,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            ..Default::default()
        };
        assert!(matches!(
            config.check_access("chat1", "user1"),
            Err(ChannelError::Disabled(_))
        ));
    }

    #[test]
    fn test_check_access_blocked_user() {
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            blocked_users: vec!["bad_user".to_string()],
            ..Default::default()
        };
        assert!(config.check_access("chat1", "bad_user").is_err());
        assert!(config.check_access("chat1", "good_user").is_ok());
    }

    #[test]
    fn test_check_access_blocked_chat() {
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            blocked_chats: vec!["bad_chat".to_string()],
            ..Default::default()
        };
        assert!(config.check_access("bad_chat", "user1").is_err());
        assert!(config.check_access("good_chat", "user1").is_ok());
    }

    #[test]
    fn test_check_access_allowed_users() {
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            allowed_users: vec!["alice".to_string()],
            ..Default::default()
        };
        assert!(config.check_access("chat1", "alice").is_ok());
        assert!(config.check_access("chat1", "bob").is_err());
    }

    #[test]
    fn test_check_access_allowed_chats() {
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            allowed_chats: vec!["group1".to_string()],
            ..Default::default()
        };
        assert!(config.check_access("group1", "user1").is_ok());
        assert!(config.check_access("group2", "user1").is_err());
    }

    #[test]
    fn test_check_access_blocklist_wins() {
        // Blocked user should be denied even if in allowed_users
        let config = ChannelConfig {
            name: "test".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: String::new(),
            },
            allowed_users: vec!["alice".to_string()],
            blocked_users: vec!["alice".to_string()],
            ..Default::default()
        };
        assert!(config.check_access("chat1", "alice").is_err());
    }

    #[test]
    fn test_blocks_to_text_text_only() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".into(),
            },
            ContentBlock::Text {
                text: "world".into(),
            },
        ];
        assert_eq!(blocks_to_text(&blocks), "hello\nworld");
    }

    #[test]
    fn test_blocks_to_text_mixed() {
        let blocks = vec![
            ContentBlock::Text {
                text: "text".into(),
            },
            ContentBlock::Thinking {
                thinking: "thinking".into(),
                signature: None,
            },
            ContentBlock::RedactedThinking {
                data: "redacted".into(),
            },
            ContentBlock::ImageUrl {
                image_url: crate::types::ImageUrl {
                    url: "http://example.com/img.png".into(),
                    detail: None,
                },
            },
            ContentBlock::Audio {
                audio: crate::types::AudioData {
                    format: "mp3".into(),
                    data: "data".into(),
                },
            },
        ];
        // Thinking/redacted blocks are stripped so they don't leak to external platforms
        assert_eq!(
            blocks_to_text(&blocks),
            "text\n[image: http://example.com/img.png]\n[audio: mp3]"
        );
    }

    #[test]
    fn test_blocks_to_text_empty() {
        assert_eq!(blocks_to_text(&[]), "");
    }
}
