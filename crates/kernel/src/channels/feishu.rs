use crate::types::ContentBlock;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::tungstenite;

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::{ChannelError, ChannelMessage, PlatformAdapter, MAX_RETRY_DELAY};

const FEISHU_BASE_URL: &str = "https://open.feishu.cn";

/// Platform upload caps: images 10MB, files 30MB; empty uploads are also
/// rejected. All violations surface as Feishu's generic API error 234001,
/// so the adapter fails fast with a precise reason instead.
const IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;
const FILE_MAX_BYTES: usize = 30 * 1024 * 1024;
const RECEIVE_ID_TYPE: &str = "chat_id";

const FRAME_TYPE_CONTROL: i32 = 0;
const FRAME_TYPE_DATA: i32 = 1;
const HEADER_TYPE: &str = "type";
const MSG_TYPE_EVENT: &str = "event";
const MSG_TYPE_PING: &str = "ping";
const MSG_TYPE_PONG: &str = "pong";

const PAYLOAD_GZIP: u8 = 1;
const PAYLOAD_PB: u8 = 2;

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct TokenResp {
    code: i32,
    msg: String,
    tenant_access_token: Option<String>,
    expire: Option<i64>,
}

struct TokenCache {
    token: String,
    expires_at: std::time::Instant,
}

// ── Adapter ─────────────────────────────────────────────────────────

pub struct FeishuAdapter {
    app_id: String,
    app_secret: String,
    client: Client,
    base_url: String,
    token_cache: Mutex<Option<TokenCache>>,
    bot_open_id: tokio::sync::Mutex<Option<String>>,
}

impl FeishuAdapter {
    pub fn new(app_id: String, app_secret: String) -> Self {
        Self {
            app_id,
            app_secret,
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client build"),
            base_url: FEISHU_BASE_URL.to_string(),
            token_cache: Mutex::new(None),
            bot_open_id: tokio::sync::Mutex::new(None),
        }
    }

    /// Point the adapter at a different API base URL (tests only).
    #[cfg(test)]
    fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    async fn ensure_bot_open_id(&self, token: &str) -> Option<String> {
        // Fast path: check cache.
        {
            let guard = self.bot_open_id.lock().await;
            if let Some(ref id) = *guard {
                return Some(id.clone());
            }
        }

        // Slow path: fetch from API (no lock held).
        let resp = self
            .client
            .get(format!("{}/open-apis/bot/v3/info", self.base_url))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        let open_id = json["bot"]["open_id"].as_str().map(|s| s.to_string());

        // Store in cache.
        let mut guard = self.bot_open_id.lock().await;
        guard.clone_from(&open_id);
        open_id
    }

    // ── Token ────────────────────────────────────────────────────────

    async fn get_token(&self) -> Result<String, ChannelError> {
        // Try cache first.
        {
            let cache = self.token_cache.lock().await;
            if let Some(t) = cached_token(cache.as_ref()) {
                return Ok(t);
            }
        }

        // Fetch new token.
        let resp: TokenResp = self
            .client
            .post(format!(
                "{}/open-apis/auth/v3/tenant_access_token/internal",
                self.base_url
            ))
            .json(&json!({ "app_id": self.app_id, "app_secret": self.app_secret }))
            .send()
            .await
            .map_err(|e| api_err("token request", e))?
            .json()
            .await
            .map_err(|e| api_err("token parse", e))?;

        if resp.code != 0 {
            return Err(ChannelError::Platform(format!(
                "token API error: {} - {}",
                resp.code, resp.msg
            )));
        }

        let token = resp
            .tenant_access_token
            .ok_or_else(|| api_err("no token", ""))?;
        let expires = std::time::Instant::now()
            + std::time::Duration::from_secs(resp.expire.unwrap_or(7200) as u64 * 9 / 10);

        // Double-check: another task may have refreshed while we were fetching.
        let mut cache = self.token_cache.lock().await;
        if let Some(t) = cached_token(cache.as_ref()) {
            return Ok(t);
        }
        *cache = Some(TokenCache {
            token: token.clone(),
            expires_at: expires,
        });
        Ok(token)
    }

    // ── HTTP helpers ─────────────────────────────────────────────────

    async fn api_post(
        &self,
        token: &str,
        url: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ChannelError> {
        self.api_json(self.client.post(url), token, body).await
    }

    async fn api_patch(
        &self,
        token: &str,
        url: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ChannelError> {
        self.api_json(self.client.patch(url), token, body).await
    }

    async fn api_get(
        &self,
        token: &str,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<serde_json::Value, ChannelError> {
        let resp = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .query(query)
            .send()
            .await
            .map_err(|e| api_err("API request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("API parse", e))?;
        check_api_resp(resp)
    }

    async fn api_json(
        &self,
        builder: reqwest::RequestBuilder,
        token: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ChannelError> {
        let resp = builder
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| api_err("API request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("API parse", e))?;

        check_api_resp(resp)
    }

    async fn upload(
        &self,
        token: &str,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<serde_json::Value, ChannelError> {
        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| api_err("upload request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("upload parse", e))?;

        check_api_resp(resp)
    }

    // ── Send helpers ─────────────────────────────────────────────────

    /// Upload and message a single file. Feishu expects `file_type` +
    /// `file_name` (or `image_type` for images) as multipart **form
    /// fields** — putting them in the URL query or omitting them yields
    /// API error 234001.
    async fn send_one_file(
        &self,
        token: &str,
        chat_id: &str,
        path: &std::path::Path,
        caption: Option<&str>,
        reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        let upload =
            super::utils::read_upload(path, IMAGE_MAX_BYTES, FILE_MAX_BYTES, "image", "file")
                .await?;
        let kind = if upload.is_image { "image" } else { "file" };
        let part =
            reqwest::multipart::Part::bytes(upload.bytes).file_name(upload.file_name.clone());
        let (form, upload_url, key_field) = if upload.is_image {
            (
                reqwest::multipart::Form::new()
                    .text("image_type", "message")
                    .part("image", part),
                format!("{}/open-apis/im/v1/images", self.base_url),
                "image_key",
            )
        } else {
            (
                reqwest::multipart::Form::new()
                    .text("file_type", "stream")
                    .text("file_name", upload.file_name)
                    .part("file", part),
                format!("{}/open-apis/im/v1/files", self.base_url),
                "file_key",
            )
        };
        let resp = self.upload(token, &upload_url, form).await?;

        let key = resp["data"][key_field]
            .as_str()
            .ok_or_else(|| api_err("no upload key", ""))?;
        let content = json!({ key_field: key }).to_string();

        let _ = self
            .send_or_reply(token, chat_id, reply_msg_id, &content, kind)
            .await?;

        if let Some(caption) = caption {
            if !caption.is_empty() {
                let content = Self::build_card(caption);
                // The file itself is already delivered — a caption failure
                // must not mark the file as undelivered.
                if let Err(e) = self
                    .send_or_reply(token, chat_id, reply_msg_id, &content, "interactive")
                    .await
                {
                    warn!(error = %e, "failed to send file caption");
                }
            }
        }
        Ok(())
    }

    /// Send or reply; returns the platform message ID when available.
    async fn send_or_reply(
        &self,
        token: &str,
        chat_id: &str,
        reply_msg_id: Option<&str>,
        content: &str,
        msg_type: &str,
    ) -> Result<Option<String>, ChannelError> {
        if let Some(msg_id) = reply_msg_id {
            self.reply_msg(token, msg_id, content, msg_type).await
        } else {
            self.send_msg(token, chat_id, content, msg_type).await
        }
    }

    async fn send_msg(
        &self,
        token: &str,
        chat_id: &str,
        content: &str,
        msg_type: &str,
    ) -> Result<Option<String>, ChannelError> {
        let resp = self
            .api_post(
                token,
                &format!(
                    "{}/open-apis/im/v1/messages?receive_id_type={RECEIVE_ID_TYPE}",
                    self.base_url
                ),
                json!({ "receive_id": chat_id, "content": content, "msg_type": msg_type }),
            )
            .await?;
        Ok(resp_data_str(&resp, "message_id"))
    }

    async fn reply_msg(
        &self,
        token: &str,
        msg_id: &str,
        content: &str,
        msg_type: &str,
    ) -> Result<Option<String>, ChannelError> {
        let resp = self
            .api_post(
                token,
                &format!("{}/open-apis/im/v1/messages/{msg_id}/reply", self.base_url),
                json!({
                    "content": content,
                    "msg_type": msg_type,
                    "reply_in_thread": true,
                }),
            )
            .await?;
        Ok(resp_data_str(&resp, "message_id"))
    }

    // ── WebSocket helpers ───────────────────────────────────────────

    async fn ws_endpoint(&self) -> Result<(String, i32), ChannelError> {
        let resp = self
            .client
            .post(format!("{}/callback/ws/endpoint", self.base_url))
            .json(&json!({ "AppID": self.app_id, "AppSecret": self.app_secret }))
            .send()
            .await
            .map_err(|e| api_err("ws endpoint request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("ws endpoint parse", e))?;

        if resp["code"].as_i64().unwrap_or(-1) != 0 {
            return Err(ChannelError::Platform(format!(
                "ws endpoint error: {}",
                resp["msg"].as_str().unwrap_or("unknown")
            )));
        }

        let url = resp["data"]["URL"]
            .as_str()
            .ok_or_else(|| api_err("no ws URL", ""))?
            .to_string();

        let service_id = url
            .find("service_id=")
            .and_then(|i| url[i + 11..].split('&').next()?.parse().ok())
            .unwrap_or(0);

        Ok((url, service_id))
    }
}

// ── PlatformAdapter impl ────────────────────────────────────────────

#[async_trait::async_trait]
impl PlatformAdapter for FeishuAdapter {
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelMessage>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        let mut retry = std::time::Duration::from_secs(5);

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let (url, service_id) = match self.ws_endpoint().await {
                Ok(ep) => ep,
                Err(e) => {
                    error!(error = %e, "ws endpoint failed, retrying");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(retry) => {},
                    }
                    retry = (retry * 2).min(MAX_RETRY_DELAY);
                    continue;
                }
            };

            let (ws_stream, _resp) = match tokio_tungstenite::connect_async(&url).await {
                Ok(pair) => {
                    info!("Feishu ws connected");
                    retry = std::time::Duration::from_secs(5);
                    pair
                }
                Err(e) => {
                    error!(error = %e, "ws connect failed, retrying");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(retry) => {},
                    }
                    retry = (retry * 2).min(MAX_RETRY_DELAY);
                    continue;
                }
            };

            let (write, mut read) = ws_stream.split();
            let write = std::sync::Arc::new(Mutex::new(write));

            // Ping loop
            let write_ping = write.clone();
            let ping_cancel = cancel.clone();
            let ping = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            let mut w = write_ping.lock().await;
                            let ping = build_ping(service_id);
                            let _ = w.send(tungstenite::Message::Binary(ping.into())).await;
                        }
                        () = ping_cancel.cancelled() => break,
                    }
                }
            });

            // Receive loop
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    Some(msg) = read.next() => {
                        match msg {
                            Ok(tungstenite::Message::Binary(data)) => {
                                let mut w = write.lock().await;
                                if let Err(e) = self.handle_binary(&data, &incoming, &mut *w).await {
                                    warn!(error = %e, "handle binary failed");
                                }
                            }
                            Ok(tungstenite::Message::Text(text)) => {
                                debug!(text = %text, "ws text msg");
                                if let Err(e) = self.handle_text(&text, &incoming).await {
                                    warn!(error = %e, "handle text failed");
                                }
                            }
                            Ok(tungstenite::Message::Ping(_)) => {
                                let mut w = write.lock().await;
                                let _ = w.send(tungstenite::Message::Pong(Vec::new().into())).await;
                            }
                            Ok(tungstenite::Message::Close(_)) => {
                                warn!("ws closed by server");
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                error!(error = %e, "ws read error");
                                break;
                            }
                        }
                    }
                    else => {
                        warn!("ws stream ended");
                        break;
                    }
                }
            }

            ping.abort();
        }

        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        // Feishu rejects oversized card payloads; cap the markdown body at
        // 30KB (bytes, UTF-8 safe — a char count would undercut CJK text).
        const MAX_MD: usize = 30_000;
        let token = self.get_token().await?;
        let text = super::blocks_to_text(&blocks);
        if text.is_empty() {
            return Ok(None);
        }
        let text = crate::utils::strs::truncate_with_suffix(&text, MAX_MD, "\n\n...(内容已截断)");

        let content = Self::build_card(&text);

        self.send_or_reply(
            &token,
            external_chat_id,
            reply_msg_id,
            &content,
            "interactive",
        )
        .await
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let token = self.get_token().await?;
        self.send_or_reply(
            &token,
            external_chat_id,
            reply_msg_id,
            card_json,
            "interactive",
        )
        .await
    }

    /// refer: <https://open.feishu.cn/document/server-docs/im-v1/message-card/patch>
    async fn update_card(&self, message_id: &str, card_json: &str) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        self.api_patch(
            &token,
            &format!("{}/open-apis/im/v1/messages/{message_id}", self.base_url),
            json!({ "content": card_json }),
        )
        .await?;
        Ok(())
    }

    async fn send_files(
        &self,
        external_chat_id: &str,
        files: &[(&std::path::Path, Option<&str>)],
        reply_msg_id: Option<&str>,
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        // Per-file resilience: one bad file must not block the rest; the
        // aggregated error names every failure so the caller can surface it.
        let mut failures = Vec::new();
        for (path, caption) in files {
            if let Err(e) = self
                .send_one_file(&token, external_chat_id, path, *caption, reply_msg_id)
                .await
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

    /// refer: <https://open.feishu.cn/document/server-docs/im-v1/message-reaction/emojis-introduce?lang=zh-CN>
    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<Option<String>, ChannelError> {
        let token = self.get_token().await?;
        let url = format!(
            "{}/open-apis/im/v1/messages/{message_id}/reactions",
            self.base_url
        );

        let resp = self
            .api_post(
                &token,
                &url,
                json!({ "reaction_type": { "emoji_type": emoji } }),
            )
            .await?;

        Ok(resp_data_str(&resp, "reaction_id"))
    }

    fn supports_status_card(&self) -> bool {
        true
    }

    /// refer: <https://open.feishu.cn/document/server-docs/im-v1/message/list>
    async fn fetch_history(
        &self,
        container: &super::HistoryContainer,
        since_ts: Option<i64>,
        limit: usize,
    ) -> Result<Vec<super::HistoryMessage>, ChannelError> {
        let (id_type, id) = match container {
            super::HistoryContainer::Chat(id) => ("chat", id),
            super::HistoryContainer::Thread(id) => ("thread", id),
        };
        let token = self.get_token().await?;

        let mut query = vec![
            ("container_id_type", id_type.to_string()),
            ("container_id", id.clone()),
            ("sort_type", "ByCreateTimeDesc".to_string()),
            ("page_size", limit.clamp(1, 50).to_string()),
        ];
        if let Some(ts) = since_ts {
            // start_time is a unix timestamp in seconds; the cursor keeps
            // millisecond precision so same-second messages aren't lost.
            query.push(("start_time", (ts / 1000).to_string()));
        }

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/messages", self.base_url),
                &query,
            ),
        )
        .await
        .map_err(|_| ChannelError::Platform("history fetch timed out".into()))??;

        let Some(items) = resp["data"]["items"].as_array() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            if item["deleted"].as_bool().unwrap_or(false) {
                continue;
            }
            // create_time is already milliseconds — keep the precision
            // (second-granularity cursors would drop same-second messages).
            let create_time = item["create_time"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or_default();
            if since_ts.is_some_and(|ts| create_time <= ts) {
                continue;
            }
            // Only humans — skips the bot itself and other apps.
            let sender = &item["sender"];
            if sender["sender_type"].as_str() != Some("user") {
                continue;
            }
            let text = Self::extract_history_text(item);
            if text.trim().is_empty() {
                continue;
            }
            out.push(super::HistoryMessage {
                message_id: item["message_id"].as_str().unwrap_or("").to_string(),
                create_time,
                sender_id: sender["id"].as_str().unwrap_or("").to_string(),
                text,
            });
        }
        // The API returns newest-first; assemble chronologically.
        out.reverse();
        Ok(out)
    }
}

// ── Message handlers ────────────────────────────────────────────────

impl FeishuAdapter {
    async fn handle_binary<W>(
        &self,
        data: &[u8],
        incoming: &mpsc::Sender<ChannelMessage>,
        write: &mut W,
    ) -> Result<(), ChannelError>
    where
        W: futures::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
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
                    .send(tungstenite::Message::Binary(ack.into()))
                    .await
                    .map_err(|e| api_err("ACK", e))?;

                if msg_type == MSG_TYPE_EVENT {
                    if let Some(ref payload) = frame.payload {
                        let text = String::from_utf8_lossy(payload);
                        debug!(payload = %text, "event payload");
                        // The frame is already ACKed — a parse failure
                        // loses the event for good, so at least log it.
                        if let Err(e) = self.parse_event(&text, incoming).await {
                            warn!(error = %e, "event parse failed, event lost");
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_text(
        &self,
        text: &str,
        incoming: &mpsc::Sender<ChannelMessage>,
    ) -> Result<(), ChannelError> {
        let msg: serde_json::Value =
            serde_json::from_str(text).map_err(|e| api_err("JSON parse", e))?;
        match msg["type"].as_str().unwrap_or("") {
            "event" => {
                let _ = self.parse_event_json(&msg, incoming).await;
                Ok(())
            }
            "ping" | "pong" | "auth_result" => {
                debug!(msg_type = msg["type"].as_str(), "control msg");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Parse event from protobuf payload (JSON string).
    async fn parse_event(
        &self,
        payload: &str,
        incoming: &mpsc::Sender<ChannelMessage>,
    ) -> Result<Option<String>, ChannelError> {
        let msg: serde_json::Value =
            serde_json::from_str(payload).map_err(|e| api_err("event JSON", e))?;
        self.parse_event_json(&msg, incoming).await
    }

    /// Extract display text from a history item: text messages get their
    /// content, posts get concatenated text runs, everything else becomes a
    /// `[msg_type]` placeholder.
    fn extract_history_text(item: &serde_json::Value) -> String {
        let msg_type = item["msg_type"].as_str().unwrap_or("unknown");
        let content: serde_json::Value = item["body"]["content"]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        match msg_type {
            "text" => content["text"].as_str().unwrap_or("").to_string(),
            "post" => {
                let text = Self::extract_post_text(&content);
                if text.is_empty() {
                    "[post]".to_string()
                } else {
                    text
                }
            }
            other => format!("[{other}]"),
        }
    }

    /// Concatenate the text runs of a post message's paragraphs, trying
    /// the known locales (posts in other locales degrade to `[post]`).
    fn extract_post_text(content: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        let paragraphs = ["zh_cn", "en_us", "ja_jp"]
            .iter()
            .find_map(|k| content[k]["content"].as_array());
        if let Some(paragraphs) = paragraphs {
            for para in paragraphs {
                let line: String = para
                    .as_array()
                    .map(|runs| {
                        runs.iter()
                            .filter_map(|r| r["text"].as_str())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                if !line.is_empty() {
                    parts.push(line);
                }
            }
        }
        parts.join("\n")
    }

    /// Feishu `create_time` is in milliseconds, but some v1.x events may be in
    /// seconds or microseconds. Normalise to seconds and format.
    fn parse_feishu_timestamp(value: &serde_json::Value) -> String {
        let ts = value
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| value.as_i64())
            .unwrap_or_else(|| chrono::Local::now().timestamp());

        let dt = if ts < 10_000_000_000 {
            chrono::DateTime::from_timestamp(ts, 0)
        } else if ts < 10_000_000_000_000 {
            chrono::DateTime::from_timestamp_millis(ts)
        } else {
            chrono::DateTime::from_timestamp_millis(ts / 1000)
        };
        dt.map_or_else(
            || chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            |dt| dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        )
    }

    fn build_card(text: &str) -> String {
        json!({
            "schema": "2.0",
            "body": {
                "elements": [{ "tag": "markdown", "content": text }]
            }
        })
        .to_string()
    }

    async fn parse_event_json(
        &self,
        msg: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelMessage>,
    ) -> Result<Option<String>, ChannelError> {
        // v2.0: header.event_type; v1.x: type
        let event_type = msg["header"]["event_type"]
            .as_str()
            .or_else(|| msg["type"].as_str())
            .unwrap_or("");

        if event_type != "im.message.receive_v1" {
            debug!(event_type, "ignoring non-message event");
            return Ok(None);
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
        let user_id = sender["sender_id"]["open_id"].as_str().unwrap_or("unknown");
        let content_str = message["content"]
            .as_str()
            .ok_or_else(|| api_err("missing content", ""))?;
        let content_json: serde_json::Value =
            serde_json::from_str(content_str).map_err(|e| api_err("content JSON", e))?;
        let text = content_json["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return Ok(None);
        }

        let thread_id = message["thread_id"].as_str().map(|s| s.to_string());
        let root_id = message["root_id"].as_str().map(|s| s.to_string());

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

        let thread_part = thread_id
            .as_ref()
            .map_or(String::new(), |tid| format!("[thread: {tid}]"));
        let formatted = format!(
            "[{ts}][from_user_id: {user_id}][chat_id: {chat_id}]{thread_part}[platform: feishu]\n{text}"
        );

        info!(chat_id, msg_id, user_id, is_mention, text, "Feishu message");

        let raw_text =
            strip_bot_mention(text, message["mentions"].as_array(), bot_open_id.as_deref());

        let channel_msg = ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: user_id.to_string(),
            external_message_id: Some(msg_id.clone()),
            is_mention,
            raw_text: Some(raw_text),
            content: vec![ContentBlock::Text { text: formatted }],
            thread_id,
            root_id,
            is_group: chat_type == "group",
            create_time: message["create_time"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok()),
        };

        if incoming.send(channel_msg).await.is_err() {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }

        Ok(Some(msg_id))
    }
}

#[cfg(test)]
#[path = "feishu_test.rs"]
mod tests;

// ── Protobuf helpers ───────────────────────────────────────────────

fn build_ping(service_id: i32) -> Vec<u8> {
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

fn build_ack(original: &lark_websocket_protobuf::pbbp2::Frame) -> Vec<u8> {
    let mut ack = original.clone();
    ack.payload = Some(r#"{"code":200}"#.as_bytes().to_vec());
    let mut buf = Vec::new();
    ack.encode(&mut buf).expect("protobuf encode");
    buf
}

// ── Small helpers ──────────────────────────────────────────────────

fn strip_bot_mention(
    text: &str,
    mentions: Option<&Vec<serde_json::Value>>,
    bot_open_id: Option<&str>,
) -> String {
    let Some(bot_open_id) = bot_open_id else {
        return text.trim().to_string();
    };
    mentions
        .into_iter()
        .flatten()
        .filter(|mention| mention["id"]["open_id"].as_str() == Some(bot_open_id))
        .filter_map(|mention| mention["key"].as_str())
        .fold(text.to_string(), |text, key| text.replace(key, ""))
        .trim()
        .to_string()
}

fn cached_token(cache: Option<&TokenCache>) -> Option<String> {
    cache.and_then(|c| {
        if std::time::Instant::now() < c.expires_at {
            Some(c.token.clone())
        } else {
            None
        }
    })
}

fn api_err(action: &str, e: impl std::fmt::Display) -> ChannelError {
    let e = format!("{e}");
    if e.is_empty() {
        ChannelError::Platform(action.to_string())
    } else {
        ChannelError::Platform(format!("{action}: {e}"))
    }
}

/// Extract `data.<key>` as an owned string from a Feishu API response.
fn resp_data_str(resp: &serde_json::Value, key: &str) -> Option<String> {
    resp["data"][key].as_str().map(str::to_string)
}

fn check_api_resp(resp: serde_json::Value) -> Result<serde_json::Value, ChannelError> {
    let code = resp["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        return Err(ChannelError::Platform(format!(
            "API error {code}: {}",
            resp["msg"].as_str().unwrap_or("unknown")
        )));
    }
    Ok(resp)
}
