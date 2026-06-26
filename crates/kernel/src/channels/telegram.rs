use crate::types::ContentBlock;
use teloxide_core::net::Download;
use teloxide_core::requests::{Request, Requester};
use teloxide_core::types::{ChatId, InputFile, MessageEntityKind, ParseMode, Recipient};
use teloxide_core::Bot;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{ChannelError, ChannelMessage, PlatformAdapter};

/// Escape characters that have special meaning in Telegram `MarkdownV2`.
fn escape_markdown_v2(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len() * 2), |mut s, c| {
            match c {
                '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '='
                | '|' | '{' | '}' | '.' | '!' => {
                    s.push('\\');
                    s.push(c);
                }
                _ => s.push(c),
            }
            s
        })
}

pub struct TelegramAdapter {
    bot: Bot,
    bot_username: tokio::sync::OnceCell<String>,
}

impl TelegramAdapter {
    pub fn new(token: String) -> Self {
        // HTTP timeout must exceed getUpdates timeout (30s) to avoid premature
        // disconnects that cause Conflict: terminated by other getUpdates.
        let client = teloxide_core::net::default_reqwest_settings()
            .timeout(std::time::Duration::from_mins(1))
            .build()
            .expect("creating reqwest client");
        let bot = Bot::with_client(token, client);

        Self {
            bot,
            bot_username: tokio::sync::OnceCell::new(),
        }
    }

    async fn ensure_username(&self) -> &str {
        self.bot_username
            .get_or_init(|| async {
                self.bot
                    .get_me()
                    .send()
                    .await
                    .ok()
                    .and_then(|me| me.username.clone())
                    .unwrap_or_default()
            })
            .await
    }

    /// Download a Telegram photo by `file_id` and convert it to a base64 data URL.
    async fn download_photo(&self, file_id: &str) -> Result<String, ChannelError> {
        let file = self
            .bot
            .get_file(file_id)
            .send()
            .await
            .map_err(|e| ChannelError::Platform(format!("get_file failed: {e}")))?;

        let mut buf = Vec::new();
        self.bot
            .download_file(&file.path, &mut buf)
            .await
            .map_err(|e| ChannelError::Platform(format!("download_file failed: {e}")))?;

        let mime_type = crate::utils::image::detect_mime_type(&buf).unwrap_or("image/jpeg");

        let base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buf);
        Ok(format!("data:{mime_type};base64,{base64}"))
    }

    /// Format sender display name from a Telegram User.
    fn format_user_name(user: &teloxide_core::types::User) -> String {
        user.username
            .as_deref()
            .filter(|u| !u.is_empty())
            .map_or_else(
                || {
                    let mut name = user.first_name.clone();
                    if let Some(last) = user.last_name.as_deref() {
                        if !last.is_empty() {
                            name.push(' ');
                            name.push_str(last);
                        }
                    }
                    name
                },
                |u| u.to_string(),
            )
    }

    /// Format a single message as a line with timestamp and sender.
    fn format_message_line(msg: &teloxide_core::types::Message) -> Option<String> {
        let text = msg.text().or_else(|| msg.caption())?;
        if text.is_empty() {
            return None;
        }
        let ts = msg.date.format("%H:%M:%S");
        let from = msg
            .from
            .as_ref()
            .map(Self::format_user_name)
            .unwrap_or_default();
        Some(format!("[{ts}][from: {from}] {text}"))
    }

    /// Build a batched `ChannelMessage` from a list of Telegram messages.
    async fn build_channel_message(
        &self,
        chat_id: &str,
        msgs: &[teloxide_core::types::Message],
        bot_username: &str,
    ) -> Option<ChannelMessage> {
        let is_mention = msgs
            .iter()
            .any(|m| Self::is_mention_of_bot(m, bot_username));
        let mut lines = Vec::new();
        let mut content: Vec<ContentBlock> = Vec::new();

        for msg in msgs {
            if let Some(line) = Self::format_message_line(msg) {
                lines.push(line);
            }

            if let Some(photo) = msg.photo().and_then(|p| p.last()) {
                match self.download_photo(&photo.file.id).await {
                    Ok(data_url) => content.push(ContentBlock::ImageUrl {
                        image_url: data_url.into(),
                    }),
                    Err(e) => {
                        warn!("Failed to download Telegram photo: {e}");
                        let ts = msg.date.format("%H:%M:%S");
                        let from = msg
                            .from
                            .as_ref()
                            .map(Self::format_user_name)
                            .unwrap_or_default();
                        lines.push(format!(
                            "[{ts}][from: {from}] [Failed to download image: {e}]"
                        ));
                    }
                }
            }
        }

        if !lines.is_empty() {
            content.insert(
                0,
                ContentBlock::Text {
                    text: lines.join("\n"),
                },
            );
        }

        if content.is_empty() {
            return None;
        }

        let user_id = msgs
            .last()
            .and_then(|m| m.from.as_ref())
            .map_or_else(|| chat_id.to_string(), |u| u.id.0.to_string());

        Some(ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: user_id,
            external_message_id: msgs.last().map(|m| m.id.0.to_string()),
            is_mention,
            content,
        })
    }

    /// Fire-and-forget a reaction emoji on a message.
    fn fire_reaction(&self, chat_id: &str, message_id: &str, emoji: &str) {
        let bot = self.bot.clone();
        let chat_id = chat_id.to_string();
        let message_id = message_id.to_string();
        let emoji = emoji.to_string();
        tokio::spawn(async move {
            let Ok(chat_id) = chat_id.parse::<i64>() else {
                return;
            };
            let Ok(msg_id) = message_id.parse::<i32>() else {
                return;
            };
            let _ = bot
                .set_message_reaction(
                    Recipient::Id(ChatId(chat_id)),
                    teloxide_core::types::MessageId(msg_id),
                )
                .reaction(vec![ReactionType::Emoji { emoji }])
                .send()
                .await;
        });
    }

    /// Check if the message contains a mention of the bot itself.
    fn is_mention_of_bot(msg: &teloxide_core::types::Message, bot_username: &str) -> bool {
        // In a private chat, every message is implicitly directed at the bot.
        if msg.chat.is_private() {
            return true;
        }

        // Helper: check entities in a given text slice for @bot_username
        let check_entities = |entities: &[teloxide_core::types::MessageEntity], text: &str| {
            entities.iter().any(|e| {
                if let MessageEntityKind::Mention = e.kind {
                    let mention = &text[e.offset..e.offset + e.length];
                    mention == format!("@{bot_username}")
                } else {
                    false
                }
            })
        };

        if bot_username.is_empty() {
            // We don't know the bot username yet; conservatively treat any @mention as ours.
            let text_entities = msg.entities().is_some_and(|e| {
                e.iter()
                    .any(|e| matches!(e.kind, MessageEntityKind::Mention))
            });
            let caption_entities = msg.caption_entities().is_some_and(|e| {
                e.iter()
                    .any(|e| matches!(e.kind, MessageEntityKind::Mention))
            });
            return text_entities || caption_entities;
        }

        if let Some(text) = msg.text() {
            if let Some(entities) = msg.entities() {
                if check_entities(entities, text) {
                    return true;
                }
            }
        }
        if let Some(caption) = msg.caption() {
            if let Some(entities) = msg.caption_entities() {
                if check_entities(entities, caption) {
                    return true;
                }
            }
        }
        false
    }
}

use teloxide_core::payloads::SetMessageReactionSetters;
use teloxide_core::types::ReactionType;

#[async_trait::async_trait]
impl PlatformAdapter for TelegramAdapter {
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelMessage>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        let mut offset: i64 = 0;

        info!("starting Telegram long polling receiver");

        loop {
            let mut req = self.bot.get_updates();
            req.offset = Some(offset as i32);
            req.limit = Some(100);
            req.timeout = Some(30);

            let result = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    info!("Telegram receiver cancelled, exiting");
                    break;
                }
                r = req.send() => r,
            };

            match result {
                Ok(updates) => {
                    // Advance offset and collect valid messages
                    let mut messages = Vec::new();
                    for update in updates {
                        offset = offset.max(i64::from(update.id.0) + 1);
                        if let teloxide_core::types::UpdateKind::Message(ref m) = update.kind {
                            let text = m
                                .text()
                                .or_else(|| m.caption())
                                .unwrap_or_default()
                                .to_string();
                            if text.is_empty() && m.photo().is_none() {
                                continue;
                            }
                            messages.push(m.clone());
                        }
                    }

                    if messages.is_empty() {
                        continue;
                    }

                    // Group by chat_id
                    let mut by_chat: std::collections::HashMap<
                        String,
                        Vec<teloxide_core::types::Message>,
                    > = std::collections::HashMap::new();
                    for msg in messages {
                        by_chat
                            .entry(msg.chat.id.0.to_string())
                            .or_default()
                            .push(msg);
                    }

                    let bot_username = self.ensure_username().await;

                    // Build and send batched ChannelMessage per chat
                    for (chat_id, msgs) in by_chat {
                        let Some(channel_msg) = self
                            .build_channel_message(&chat_id, &msgs, bot_username)
                            .await
                        else {
                            continue;
                        };

                        if let Some(ref msg_id) = channel_msg.external_message_id {
                            self.fire_reaction(&chat_id, msg_id, "👀");
                        }

                        if incoming.send(channel_msg).await.is_err() {
                            warn!("incoming channel closed, stopping receiver");
                            return Ok(());
                        }
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Telegram getUpdates failed, retrying");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
                    }
                }
            }
        }

        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
    ) -> Result<(), ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;

        let text = super::blocks_to_text(&blocks);
        if text.is_empty() {
            return Ok(());
        }

        let safe_text = escape_markdown_v2(&text);
        let mut req = self
            .bot
            .send_message(Recipient::Id(ChatId(chat_id)), safe_text);
        req.parse_mode = Some(ParseMode::MarkdownV2);
        req.send()
            .await
            .map_err(|e| ChannelError::Platform(format!("send_message failed: {e}")))?;

        Ok(())
    }

    async fn send_files(
        &self,
        external_chat_id: &str,
        files: &[(&std::path::Path, Option<&str>)],
    ) -> Result<(), ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;
        let recipient = Recipient::Id(ChatId(chat_id));

        for (path, caption) in files {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|e| ChannelError::Platform(format!("read file: {e}")))?;
            let input = InputFile::memory(bytes);

            let mime = mime_guess::from_path(path).first_or_octet_stream();
            if mime.type_() == "image" {
                let mut req = self.bot.send_photo(recipient.clone(), input);
                if let Some(caption) = caption {
                    req.caption = Some(escape_markdown_v2(caption));
                    req.parse_mode = Some(ParseMode::MarkdownV2);
                }
                req.send()
                    .await
                    .map_err(|e| ChannelError::Platform(format!("send_photo failed: {e}")))?;
            } else {
                let mut req = self.bot.send_document(recipient.clone(), input);
                if let Some(caption) = caption {
                    req.caption = Some(escape_markdown_v2(caption));
                    req.parse_mode = Some(ParseMode::MarkdownV2);
                }
                req.send()
                    .await
                    .map_err(|e| ChannelError::Platform(format!("send_document failed: {e}")))?;
            }
        }

        Ok(())
    }

    async fn send_reaction(
        &self,
        external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;
        let msg_id: i32 = message_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid message_id: {e}")))?;

        self.bot
            .set_message_reaction(
                Recipient::Id(ChatId(chat_id)),
                teloxide_core::types::MessageId(msg_id),
            )
            .reaction(vec![ReactionType::Emoji {
                emoji: emoji.to_string(),
            }])
            .send()
            .await
            .map_err(|e| ChannelError::Platform(format!("set_message_reaction failed: {e}")))?;
        Ok(())
    }

    async fn send_typing(&self, external_chat_id: &str) -> Result<(), ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;

        self.bot
            .send_chat_action(
                Recipient::Id(ChatId(chat_id)),
                teloxide_core::types::ChatAction::Typing,
            )
            .send()
            .await
            .map_err(|e| ChannelError::Platform(format!("send_chat_action failed: {e}")))?;
        Ok(())
    }
}
