use crate::types::ContentBlock;
use teloxide_core::net::Download;
use teloxide_core::requests::{Request, Requester};
use teloxide_core::types::{ChatId, InputFile, MessageEntityKind, ParseMode, Recipient};
use teloxide_core::Bot;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{ChannelError, ChannelMessage, PlatformAdapter};

pub struct TelegramAdapter {
    bot: Bot,
    bot_username: tokio::sync::OnceCell<String>,
}

impl TelegramAdapter {
    pub fn new(token: String) -> Self {
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
        // Nothing is cached on failure, so a failed get_me retries on the
        // next batch: an empty username would degrade the mention check
        // to "any @mention counts" for the process lifetime.
        match self
            .bot_username
            .get_or_try_init(|| async {
                self.bot
                    .get_me()
                    .send()
                    .await
                    .map(|me| me.username.clone().unwrap_or_default())
            })
            .await
        {
            Ok(name) => name.as_str(),
            Err(e) => {
                warn!("get_me failed; mention detection degraded this batch: {e}");
                ""
            }
        }
    }

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

    /// Merge consecutive non-command messages into one channel message.
    /// Commands are always isolated, and a batch never crosses senders —
    /// access control and reactions target the (single) sender of each
    /// merged message.
    fn split_command_batches<T>(
        items: Vec<T>,
        is_command: impl Fn(&T) -> bool,
        sender_key: impl Fn(&T) -> u64,
    ) -> Vec<Vec<T>> {
        let mut batches = Vec::new();
        let mut regular_items = Vec::new();
        let mut regular_sender: Option<u64> = None;
        for item in items {
            if is_command(&item) {
                if !regular_items.is_empty() {
                    batches.push(std::mem::take(&mut regular_items));
                    regular_sender = None;
                }
                batches.push(vec![item]);
            } else {
                let sender = sender_key(&item);
                if regular_sender.is_some_and(|s| s != sender) {
                    batches.push(std::mem::take(&mut regular_items));
                }
                regular_sender = Some(sender);
                regular_items.push(item);
            }
        }
        if !regular_items.is_empty() {
            batches.push(regular_items);
        }
        batches
    }

    fn format_message_line(
        msg: &teloxide_core::types::Message,
        chat_id: &str,
        user_id: &str,
    ) -> Option<String> {
        let text = msg.text().or_else(|| msg.caption())?;
        if text.is_empty() {
            return None;
        }
        let ts = msg.date.format("%Y-%m-%d %H:%M:%S");
        Some(format!(
            "[{ts}][from_user_id: {user_id}][chat_id: {chat_id}][platform: telegram]\n{text}"
        ))
    }

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
            let user_id = msg
                .from
                .as_ref()
                .map_or_else(|| chat_id.to_string(), |u| u.id.0.to_string());

            if let Some(line) = Self::format_message_line(msg, chat_id, &user_id) {
                lines.push(line);
            }

            if let Some(photo) = msg.photo().and_then(|p| p.last()) {
                match self.download_photo(&photo.file.id).await {
                    Ok(data_url) => content.push(ContentBlock::ImageUrl {
                        image_url: data_url.into(),
                    }),
                    Err(e) => {
                        warn!("Failed to download Telegram photo: {e}");
                        let ts = msg.date.format("%Y-%m-%d %H:%M:%S");
                        lines.push(format!(
                            "[{ts}][from_user_id: {user_id}][chat_id: {chat_id}][platform: telegram]\n[Failed to download image: {e}]"
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

        let raw_text = (msgs.len() == 1)
            .then(|| msgs[0].text().or_else(|| msgs[0].caption()))
            .flatten()
            .map(str::to_string);

        let is_group = msgs.last().is_some_and(|m| !m.chat.is_private());

        Some(ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: user_id,
            external_message_id: msgs.last().map(|m| m.id.0.to_string()),
            is_mention,
            raw_text,
            content,
            thread_id: None,
            root_id: None,
            is_group,
            create_time: None,
        })
    }

    fn is_mention_of_bot(msg: &teloxide_core::types::Message, bot_username: &str) -> bool {
        if msg.chat.is_private() {
            return true;
        }

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

#[cfg(test)]
#[path = "telegram_test.rs"]
mod tests;

use teloxide_core::payloads::SetMessageReactionSetters;
use teloxide_core::types::{ReactionType, ReplyParameters};

/// Build quote-reply parameters for a platform message ID. Returns `None`
/// when the ID is absent or not a valid Telegram message ID.
/// `allow_sending_without_reply` keeps the reply functional when the original
/// message was deleted.
fn reply_parameters(reply_msg_id: Option<&str>) -> Option<ReplyParameters> {
    let msg_id = reply_msg_id?.parse::<i32>().ok()?;
    let mut params = ReplyParameters::new(teloxide_core::types::MessageId(msg_id));
    params.allow_sending_without_reply = Some(true);
    Some(params)
}

/// Telegram bot upload caps: photos 10MB, documents 50MB; empty uploads are
/// rejected — all with a generic 400, so fail fast with a precise reason.
const PHOTO_MAX_BYTES: usize = 10 * 1024 * 1024;
const DOCUMENT_MAX_BYTES: usize = 50 * 1024 * 1024;

/// Upload a single file as a photo (image MIME) or document, preserving
/// the original file name for the download.
async fn send_one_file(
    bot: &Bot,
    recipient: &Recipient,
    path: &std::path::Path,
    caption: Option<&str>,
    reply_msg_id: Option<&str>,
) -> Result<(), ChannelError> {
    let upload = super::utils::read_upload(
        path,
        PHOTO_MAX_BYTES,
        DOCUMENT_MAX_BYTES,
        "photo",
        "document",
    )
    .await?;
    let input = InputFile::memory(upload.bytes).file_name(upload.file_name);

    if upload.is_image {
        let mut req = bot.send_photo(recipient.clone(), input);
        if let Some(caption) = caption {
            req.caption = Some(caption.to_string());
            req.parse_mode = Some(ParseMode::MarkdownV2);
        }
        req.reply_parameters = reply_parameters(reply_msg_id);
        req.send()
            .await
            .map_err(|e| ChannelError::Platform(format!("send_photo failed: {e}")))?;
    } else {
        let mut req = bot.send_document(recipient.clone(), input);
        if let Some(caption) = caption {
            req.caption = Some(caption.to_string());
            req.parse_mode = Some(ParseMode::MarkdownV2);
        }
        req.reply_parameters = reply_parameters(reply_msg_id);
        req.send()
            .await
            .map_err(|e| ChannelError::Platform(format!("send_document failed: {e}")))?;
    }
    Ok(())
}

/// Telegram caps message text at 4096 UTF-16 code units; oversize texts fail
/// the whole send. Truncate with a marker instead — measured in UTF-16 units,
/// not chars, so non-BMP text (emoji) can't slip past the cap.
const MAX_MESSAGE_UTF16_UNITS: usize = 4000;

fn cap_message_length(text: &str) -> String {
    crate::utils::strs::truncate_by_utf16(text, MAX_MESSAGE_UTF16_UNITS, "\n\n...(内容已截断)")
}

/// Confirm all pending updates (a negative offset makes `getUpdates`
/// return only the latest one, confirming everything before it) and
/// return the offset to poll from. Failure degrades to 0 — the old
/// behavior of replaying the backlog.
async fn skip_backlog(bot: &teloxide_core::Bot) -> i64 {
    let mut req = bot.get_updates();
    req.offset = Some(-1);
    req.limit = Some(1);
    req.timeout = Some(0);
    match req.send().await {
        Ok(updates) => updates.last().map_or(0, |u| i64::from(u.id.0) + 1),
        Err(e) => {
            warn!("failed to skip Telegram update backlog: {e}");
            0
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for TelegramAdapter {
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelMessage>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        // Skip the update backlog: Telegram keeps unconfirmed updates for
        // up to 24h and the polling offset lives only in memory, so a
        // restart would otherwise replay the last unconfirmed batch —
        // duplicate steers, replies, and reactions for messages already
        // handled.
        let mut offset: i64 = skip_backlog(&self.bot).await;

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

                    for (chat_id, msgs) in by_chat {
                        let batches = Self::split_command_batches(
                            msgs,
                            |msg| {
                                let raw_text =
                                    msg.text().or_else(|| msg.caption()).unwrap_or_default();
                                super::hub::has_channel_command_prefix(raw_text)
                            },
                            |msg| msg.from.as_ref().map_or(0, |u| u.id.0),
                        );

                        for batch in batches {
                            let Some(channel_msg) = self
                                .build_channel_message(&chat_id, &batch, bot_username)
                                .await
                            else {
                                continue;
                            };

                            if incoming.send(channel_msg).await.is_err() {
                                warn!("incoming channel closed, stopping receiver");
                                return Ok(());
                            }
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
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;

        let text = super::blocks_to_text(&blocks);
        if text.is_empty() {
            return Ok(None);
        }
        let text = cap_message_length(&text);

        let recipient = Recipient::Id(ChatId(chat_id));

        let mut req = self.bot.send_message(recipient.clone(), text.clone());
        req.parse_mode = Some(ParseMode::MarkdownV2);
        req.reply_parameters = reply_parameters(reply_msg_id);
        if let Err(e) = req.send().await {
            warn!(error = %e, "MarkdownV2 send failed, falling back to plain text");
            let mut req = self.bot.send_message(recipient, text);
            req.reply_parameters = reply_parameters(reply_msg_id);
            req.send()
                .await
                .map_err(|e| ChannelError::Platform(format!("send_message failed: {e}")))?;
        }

        // Telegram reactions/ids are not tracked for observability.
        Ok(None)
    }

    async fn send_files(
        &self,
        external_chat_id: &str,
        files: &[(&std::path::Path, Option<&str>)],
        reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        let chat_id: i64 = external_chat_id
            .parse()
            .map_err(|e| ChannelError::Platform(format!("invalid chat_id: {e}")))?;
        let recipient = Recipient::Id(ChatId(chat_id));

        // Per-file resilience: one bad file must not block the rest; the
        // aggregated error names every failure so the caller can surface it.
        let mut failures = Vec::new();
        for (path, caption) in files {
            if let Err(e) = send_one_file(&self.bot, &recipient, path, *caption, reply_msg_id).await
            {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                warn!(error = %e, file = %path.display(), "failed to send file");
                failures.push(format!("{name} ({e})"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ChannelError::Platform(format!(
                "attachment(s) not delivered: {}",
                failures.join("; ")
            )))
        }
    }

    async fn send_reaction(
        &self,
        external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<Option<String>, ChannelError> {
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
        // Telegram reactions carry no removable ID.
        Ok(None)
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
