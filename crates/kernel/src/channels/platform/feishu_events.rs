//! Feishu incoming-event pipeline: websocket frames → parsed events.

use crate::types::ContentBlock;
use futures::SinkExt;
use prost::Message as ProstMessage;
use tokio_tungstenite::tungstenite;

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::channels::{
    CardAction, ChannelError, ChannelEvent, ChannelMessage, DocPermissionRequest, PlatformAdapter,
};

pub(crate) const FRAME_TYPE_CONTROL: i32 = 0;

pub(crate) const FRAME_TYPE_DATA: i32 = 1;

pub(crate) const HEADER_TYPE: &str = "type";

pub(crate) const MSG_TYPE_EVENT: &str = "event";

pub(crate) const MSG_TYPE_CARD: &str = "card";

pub(crate) const MSG_TYPE_PING: &str = "ping";

pub(crate) const MSG_TYPE_PONG: &str = "pong";

pub(crate) const PAYLOAD_GZIP: u8 = 1;

pub(crate) const PAYLOAD_PB: u8 = 2;

/// Application-level ping cadence; the gateway answers every ping with a pong.
pub(crate) const PING_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// No inbound frame (pongs included) for this long means a zombie
/// connection (half-open TCP never errors) — reconnect. 2.5× ping interval.
pub(crate) const FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(150);

// ── Types ────────────────────────────────────────────────────────────

use crate::channels::feishu::{api_err, FeishuAdapter};
use crate::channels::feishu_text::strip_bot_mention;

impl FeishuAdapter {
    /// Handle one binary frame. The `write` lock is held only around the
    /// ACK send — parsing (token HTTP, queue send) must not starve pings.
    pub(crate) async fn handle_binary<S>(
        &self,
        data: &[u8],
        incoming: &mpsc::Sender<ChannelEvent>,
        write: &Mutex<S>,
    ) -> Result<(), ChannelError>
    where
        S: futures::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        if data.is_empty() {
            return Ok(());
        }

        let (body, is_gzip) =
            if data.len() > 1 && (data[0] == PAYLOAD_GZIP || data[0] == PAYLOAD_PB) {
                (&data[1..], data[0] == PAYLOAD_GZIP)
            } else {
                (data, false)
            };

        if is_gzip {
            warn!("gzip protobuf not supported");
            return Ok(());
        }

        let frame = lark_websocket_protobuf::pbbp2::Frame::decode(body)
            .map_err(|e| api_err("protobuf decode", e))?;

        let msg_type = frame
            .headers
            .iter()
            .find(|h| h.key == HEADER_TYPE)
            .map_or("", |h| h.value.as_str());

        match frame.method {
            FRAME_TYPE_CONTROL if msg_type == MSG_TYPE_PONG => {
                debug!("pong received");
            }
            FRAME_TYPE_DATA => {
                let ack = build_ack(&frame);
                write
                    .lock()
                    .await
                    .send(tungstenite::Message::Binary(ack.into()))
                    .await
                    .map_err(|e| api_err("ACK", e))?;

                let Some(ref payload) = frame.payload else {
                    return Ok(());
                };
                let text = String::from_utf8_lossy(payload);
                match msg_type {
                    MSG_TYPE_EVENT => {
                        debug!(payload = %text, "event payload");
                        // The frame is already ACKed — a parse failure
                        // loses the event for good, so at least log it.
                        if let Err(e) = self.parse_event(&text, incoming).await {
                            warn!(error = %e, "event parse failed, event lost");
                        }
                    }
                    MSG_TYPE_CARD => {
                        debug!(payload = %text, "card callback payload");
                        if let Err(e) = Self::forward_card_action_str(&text, incoming).await {
                            warn!(error = %e, "card action parse failed, action lost");
                        }
                    }
                    _ => {
                        debug!(msg_type, "ignoring unknown data frame type");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) async fn handle_text(
        &self,
        text: &str,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<(), ChannelError> {
        let msg: serde_json::Value =
            serde_json::from_str(text).map_err(|e| api_err("JSON parse", e))?;
        match msg["type"].as_str().unwrap_or("") {
            "event" => {
                let _ = self.parse_event_json(&msg, incoming).await;
                Ok(())
            }
            MSG_TYPE_CARD => Self::forward_card_action(&msg, incoming).await,
            "ping" | "pong" | "auth_result" => {
                debug!(msg_type = msg["type"].as_str(), "control msg");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Parse event from protobuf payload (JSON string).
    pub(crate) async fn parse_event(
        &self,
        payload: &str,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<Option<String>, ChannelError> {
        let msg: serde_json::Value =
            serde_json::from_str(payload).map_err(|e| api_err("event JSON", e))?;
        self.parse_event_json(&msg, incoming).await
    }

    pub(crate) async fn parse_event_json(
        &self,
        msg: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<Option<String>, ChannelError> {
        // v2.0: header.event_type; v1.x: type
        let event_type = msg["header"]["event_type"]
            .as_str()
            .or_else(|| msg["type"].as_str())
            .unwrap_or("");

        match event_type {
            "im.message.receive_v1" => {} // parsed below
            "drive.file.permission_member_applied_v1" => {
                return Self::forward_doc_permission_event(&msg["event"], incoming).await;
            }
            "drive.notice.comment_add_v1" => {
                return self.forward_doc_comment_event(msg, incoming).await;
            }
            // Card callbacks are normally delivered as `card` data frames,
            // but tolerate delivery as a plain event too.
            "card.action.trigger" => {
                Self::forward_card_action(msg, incoming).await?;
                return Ok(None);
            }
            _ => {
                debug!(event_type, "ignoring event");
                return Ok(None);
            }
        }

        let event = &msg["event"];
        let message = &event["message"];
        let sender = &event["sender"];
        let chat_id = message["chat_id"]
            .as_str()
            .ok_or_else(|| api_err("missing chat_id", ""))?;
        let msg_id = message["message_id"]
            .as_str()
            .ok_or_else(|| api_err("missing message_id", ""))?
            .to_string();
        // Redelivery guard (lost ACK → resend): don't trigger the agent
        // twice. Chat messages only — perm events / card actions dedup in
        // the store. Relies on the sequential receive loop.
        if self.seen_messages.lock().await.contains(&msg_id) {
            info!(chat_id, msg_id, "duplicate event delivery, skipped");
            return Ok(None);
        }
        let user_id = sender["sender_id"]["open_id"].as_str().unwrap_or("unknown");
        let content_str = message["content"]
            .as_str()
            .ok_or_else(|| api_err("missing content", ""))?;
        let msg_type = message["message_type"].as_str().unwrap_or("");
        let content_json: serde_json::Value = match serde_json::from_str(content_str) {
            Ok(v) => v,
            // merge_forward event content is a fixed plain string
            // ("Merged and Forwarded Message"), not JSON; the real body
            // is fetched below. Other types keep failing hard on
            // corrupt content.
            Err(_) if msg_type == "merge_forward" => serde_json::Value::Null,
            Err(e) => return Err(api_err("content JSON", e)),
        };
        // Rich-text (post) content has no top-level `text` — its body lives
        // in per-locale paragraphs; images ride along as `img` runs there
        // or as the whole body of an `image` message.
        // Interactive (card) messages: the event content is only a legacy
        // placeholder, never the real card body — the text is fetched later
        // via `fetch_message` (with `card_msg_content_type`), but only after
        // the mention gate so group cards that don't @ the bot stay ignored.
        let is_card = msg_type == "interactive";
        // merge_forward events also carry only the fixed placeholder — the
        // sub-messages are fetched below like cards, but WITHOUT the
        // adapter-side mention gate: hub policy (incl. watch mirroring)
        // applies to it like any normal message.
        let is_merge_forward = msg_type == "merge_forward";
        let (text, image_keys) = match msg_type {
            "text" => (
                content_json["text"].as_str().unwrap_or("").to_string(),
                Vec::new(),
            ),
            "post" => (
                Self::extract_post_text(&content_json),
                Self::extract_post_image_keys(&content_json),
            ),
            "image" => (
                String::new(),
                content_json["image_key"]
                    .as_str()
                    .map(|k| vec![k.to_string()])
                    .unwrap_or_default(),
            ),
            // Stickers carry their key inline so the agent can echo the
            // same sticker back (lark send needs the file_key).
            "sticker" => (
                content_json["file_key"]
                    .as_str()
                    .map_or_else(|| "[sticker]".to_string(), |k| format!("[sticker: {k}]")),
                Vec::new(),
            ),
            _ => (String::new(), Vec::new()),
        };
        // Cards and merge_forwards defer their content to the fetch below,
        // so they are exempt from the empty-content drop here.
        if !is_card && !is_merge_forward && text.is_empty() && image_keys.is_empty() {
            debug!(chat_id, msg_type, "ignoring message without usable content");
            return Ok(None);
        }

        let thread_id = message["thread_id"].as_str().map(|s| s.to_string());
        let root_id = message["root_id"].as_str().map(|s| s.to_string());
        let parent_id = message["parent_id"].as_str().map(|s| s.to_string());

        let ts = Self::parse_feishu_timestamp(&message["create_time"]);

        let token = self.get_token().await?;
        let bot_open_id = self.ensure_bot_open_id(&token).await;

        let chat_type = message["chat_type"].as_str().unwrap_or("");
        let is_mention = if chat_type == "p2p" {
            true
        } else if let Some(ref bot_id) = bot_open_id {
            message["mentions"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|m| m["id"]["open_id"].as_str() == Some(bot_id))
            })
        } else {
            false
        };

        // Card messages follow mention semantics: p2p always, group only when
        // the bot is @'d. Fetch the real body (best-effort); on failure fall
        // back to the `[interactive]` placeholder but still trigger — never
        // silently swallow like before.
        // merge_forward: same fetch-and-degrade shape, but no mention gate —
        // the expansion also carries any sub-message image keys along.
        let (text, image_keys) = if is_card {
            if !is_mention {
                debug!(chat_id, msg_id, "ignoring group card without bot mention");
                return Ok(None);
            }
            let t = match self.fetch_message(&msg_id).await {
                Ok(Some(h)) if !h.text.is_empty() => h.text,
                Ok(_) => {
                    debug!(
                        chat_id,
                        msg_id, "card body empty on fetch, using placeholder"
                    );
                    "[interactive]".to_string()
                }
                Err(e) => {
                    warn!(chat_id, msg_id, error = %e, "card fetch failed, using placeholder");
                    "[interactive]".to_string()
                }
            };
            (t, image_keys)
        } else if is_merge_forward {
            match self.fetch_message(&msg_id).await {
                Ok(Some(h)) if !h.text.is_empty() => (h.text, h.image_keys),
                Ok(_) => {
                    debug!(
                        chat_id,
                        msg_id, "merge_forward body empty on fetch, using placeholder"
                    );
                    ("[merge_forward]".to_string(), image_keys)
                }
                Err(e) => {
                    warn!(chat_id, msg_id, error = %e, "merge_forward fetch failed, using placeholder");
                    ("[merge_forward]".to_string(), image_keys)
                }
            }
        } else {
            (text, image_keys)
        };

        let thread_part = thread_id
            .as_ref()
            .map_or(String::new(), |tid| format!("[thread: {tid}]"));
        let root_part = root_id
            .as_ref()
            .map_or(String::new(), |rid| format!("[root: {rid}]"));
        let header = format!(
            "[{ts}][from_user_id: {user_id}][chat_id: {chat_id}][msg_id: {msg_id}]{thread_part}{root_part}[platform: feishu]"
        );
        let formatted = if text.is_empty() {
            header
        } else {
            format!("{header}\n{text}")
        };

        info!(
            chat_id,
            msg_id,
            user_id,
            is_mention,
            thread_id = thread_id.as_deref().unwrap_or(""),
            root_id = root_id.as_deref().unwrap_or(""),
            text,
            image_count = image_keys.len(),
            "Feishu message"
        );

        let raw_text = strip_bot_mention(
            &text,
            message["mentions"].as_array(),
            bot_open_id.as_deref(),
        );

        // Images are NOT downloaded here — keys travel with the message
        // for post-gate download (see `ChannelMessage::image_keys`).
        let channel_msg = ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: user_id.to_string(),
            external_message_id: Some(msg_id.clone()),
            is_mention,
            raw_text: Some(raw_text),
            content: vec![ContentBlock::Text { text: formatted }],
            image_keys,
            thread_id,
            root_id,
            parent_id,
            is_group: chat_type == "group",
            create_time: message["create_time"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok()),
            doc_comment: None,
        };

        if incoming
            .send(ChannelEvent::Message(channel_msg))
            .await
            .is_err()
        {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }

        // Record only after a successful forward, so a copy that failed
        // mid-parse never blocks a later delivery of the same message.
        let _ = self.seen_messages.lock().await.put(msg_id.clone(), ());

        Ok(Some(msg_id))
    }

    /// Parse a `drive.notice.comment_add_v1` event and forward it as a
    /// [`ChannelEvent::DocCommentAdded`]. Ids only — the comment content is
    /// fetched by the hub post-policy (deferred, like image keys). The only
    /// adapter-side filter beyond dedup is the self-authored check (the
    /// bot's own comment replies must not retrigger it); it needs the bot
    /// identity, which is adapter domain knowledge, and only costs the
    /// (cached) bot-info call for mention events — non-mention events are
    /// forwarded untouched and filtered by the hub's policy instead.
    pub(crate) async fn forward_doc_comment_event(
        &self,
        msg: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<Option<String>, ChannelError> {
        let event = &msg["event"];
        let meta = &event["notice_meta"];
        let comment_id = event["comment_id"].as_str().unwrap_or_default();
        let reply_id = event["reply_id"].as_str().unwrap_or_default();
        // Redelivery guard (lost ACK → resend), keyed like chat messages.
        let dedup_key = format!("doc_comment:{comment_id}:{reply_id}");
        if self.seen_messages.lock().await.contains(&dedup_key) {
            info!(
                comment_id,
                reply_id, "duplicate comment event delivery, skipped"
            );
            return Ok(None);
        }
        let commenter = meta["from_user_id"]["open_id"].as_str().unwrap_or_default();
        let file_token = meta["file_token"].as_str().unwrap_or_default();
        let file_type = meta["file_type"].as_str().unwrap_or_default();
        if comment_id.is_empty() || commenter.is_empty() || file_token.is_empty() {
            warn!(payload = %msg, "comment event missing ids, ignored");
            return Ok(None);
        }
        let is_mentioned = event["is_mentioned"].as_bool().unwrap_or(false);
        if is_mentioned {
            let token = self.get_token().await?;
            if let Some(bot_id) = self.ensure_bot_open_id(&token).await {
                if bot_id == commenter {
                    debug!(comment_id, "self-authored comment event, skipped");
                    return Ok(None);
                }
            }
        }
        let notice = crate::channels::DocCommentNotice {
            file_token: file_token.to_string(),
            file_type: file_type.to_string(),
            comment_id: comment_id.to_string(),
            reply_id: if reply_id.is_empty() {
                None
            } else {
                Some(reply_id.to_string())
            },
            commenter_open_id: commenter.to_string(),
            is_mentioned,
            notice_type: meta["notice_type"].as_str().unwrap_or_default().to_string(),
            create_time: msg["header"]["create_time"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok()),
        };
        info!(
            comment_id,
            reply_id, commenter, is_mentioned, file_type, "Feishu doc comment event"
        );
        if incoming
            .send(ChannelEvent::DocCommentAdded(notice))
            .await
            .is_err()
        {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }
        // Record only after a successful forward (same rule as messages).
        self.seen_messages.lock().await.put(dedup_key, ());
        Ok(None)
    }

    /// Parse a `drive.file.permission_member_applied_v1` event and forward
    /// it as a [`ChannelEvent::DocPermissionApplied`]. The applicant can be
    /// any mix of users / chats / departments; all three lists ride along.
    pub(crate) async fn forward_doc_permission_event(
        event: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<Option<String>, ChannelError> {
        let open_ids = |key: &str| {
            event[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m["open_id"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let id_strings = |key: &str| {
            event[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let req = DocPermissionRequest {
            file_token: event["file_token"].as_str().unwrap_or_default().to_string(),
            file_type: event["file_type"].as_str().unwrap_or_default().to_string(),
            permission: event["permission"].as_str().unwrap_or_default().to_string(),
            remark: event["application_remark"].as_str().map(str::to_string),
            applicant_users: open_ids("application_user_list"),
            applicant_chats: id_strings("application_chat_list"),
            applicant_departments: id_strings("application_department_list"),
        };
        if req.file_token.is_empty() {
            warn!("doc permission event missing file_token, ignored");
            return Ok(None);
        }
        info!(
            file_token = %req.file_token,
            file_type = %req.file_type,
            permission = %req.permission,
            users = req.applicant_users.len(),
            chats = req.applicant_chats.len(),
            departments = req.applicant_departments.len(),
            "doc permission applied"
        );
        if incoming
            .send(ChannelEvent::DocPermissionApplied(req))
            .await
            .is_err()
        {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }
        Ok(None)
    }

    /// Parse a `card.action.trigger` callback from a protobuf `card` frame
    /// payload (JSON string).
    pub(crate) async fn forward_card_action_str(
        payload: &str,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<(), ChannelError> {
        let msg: serde_json::Value =
            serde_json::from_str(payload).map_err(|e| api_err("card action JSON", e))?;
        Self::forward_card_action(&msg, incoming).await
    }

    /// Forward a card button callback as a [`ChannelEvent::CardAction`].
    /// The real `card.action.trigger` payload is a v2 envelope
    /// (`{schema, header, event: {operator, action, context, token}}`) —
    /// a bare callback body is tolerated as well. The button value rides
    /// along opaquely — the hub validates its shape
    /// (`{"action": "approve"|"deny", "id": N}`).
    pub(crate) async fn forward_card_action(
        payload: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelEvent>,
    ) -> Result<(), ChannelError> {
        let body = if payload["event"].is_object() {
            &payload["event"]
        } else {
            payload
        };
        // Select/checker components report the chosen state NEXT TO the
        // static button value (verified: `option` = chosen option value,
        // `checked` = post-click state) — fold them into the value so
        // handlers read a single JSON document.
        let mut value = body["action"]["value"].clone();
        if value.is_object() {
            if let Some(opt) = body["action"]["option"].as_str() {
                value["option"] = serde_json::json!(opt);
            }
            if let Some(checked) = body["action"]["checked"].as_bool() {
                value["checked"] = serde_json::json!(checked);
            }
        }
        let action = CardAction {
            operator_open_id: body["operator"]["open_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            chat_id: body["context"]["open_chat_id"].as_str().map(str::to_string),
            message_id: body["context"]["open_message_id"]
                .as_str()
                .map(str::to_string),
            value,
        };
        if action.operator_open_id.is_empty() || action.value.is_null() {
            warn!(payload = %payload, "card action missing operator or value, ignored");
            return Ok(());
        }
        info!(
            operator = %action.operator_open_id,
            value = %action.value,
            "card action received"
        );
        if incoming
            .send(ChannelEvent::CardAction(action))
            .await
            .is_err()
        {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }
        Ok(())
    }
}

pub(crate) fn build_ping(service_id: i32) -> Vec<u8> {
    let frame = lark_websocket_protobuf::pbbp2::Frame {
        seq_id: 0,
        log_id: 0,
        service: service_id,
        method: FRAME_TYPE_CONTROL,
        headers: vec![lark_websocket_protobuf::pbbp2::Header {
            key: HEADER_TYPE.to_string(),
            value: MSG_TYPE_PING.to_string(),
        }],
        payload_encoding: None,
        payload_type: None,
        payload: None,
        log_id_new: None,
    };
    let mut buf = Vec::new();
    frame.encode(&mut buf).expect("protobuf encode");
    buf
}

pub(crate) fn build_ack(original: &lark_websocket_protobuf::pbbp2::Frame) -> Vec<u8> {
    let mut ack = original.clone();
    ack.payload = Some(r#"{"code":200}"#.as_bytes().to_vec());
    let mut buf = Vec::new();
    ack.encode(&mut buf).expect("protobuf encode");
    buf
}

// ── Small helpers ──────────────────────────────────────────────────
