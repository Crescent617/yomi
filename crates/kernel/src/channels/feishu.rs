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
const RECEIVE_ID_TYPE: &str = "chat_id";

const FRAME_TYPE_CONTROL: i32 = 0;
const FRAME_TYPE_DATA: i32 = 1;
const HEADER_TYPE: &str = "type";
const MSG_TYPE_EVENT: &str = "event";
const MSG_TYPE_PING: &str = "ping";
const MSG_TYPE_PONG: &str = "pong";

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
    token_cache: Mutex<Option<TokenCache>>,
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
            token_cache: Mutex::new(None),
        }
    }

    // ── Token ────────────────────────────────────────────────────────

    async fn get_token(&self) -> Result<String, ChannelError> {
        // Fast path: check under lock.
        {
            let cache = self.token_cache.lock().await;
            if let Some(ref cached) = *cache {
                if std::time::Instant::now() < cached.expires_at {
                    return Ok(cached.token.clone());
                }
            }
        } // lock dropped

        // Slow path: fetch new token (no lock held during HTTP).
        let resp: TokenResp = self
            .client
            .post(format!(
                "{FEISHU_BASE_URL}/open-apis/auth/v3/tenant_access_token/internal"
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
            .ok_or_else(|| api_err_str("no token"))?;
        let expires = std::time::Instant::now()
            + std::time::Duration::from_secs(resp.expire.unwrap_or(7200) as u64 * 9 / 10);

        // Double-check: another task may have refreshed while we were fetching.
        let mut cache = self.token_cache.lock().await;
        if let Some(ref cached) = *cache {
            if std::time::Instant::now() < cached.expires_at {
                return Ok(cached.token.clone());
            }
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
        let resp = self
            .client
            .post(url)
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

    async fn send_msg(
        &self,
        token: &str,
        chat_id: &str,
        content: &str,
        msg_type: &str,
    ) -> Result<(), ChannelError> {
        self.api_post(
            token,
            &format!(
                "{FEISHU_BASE_URL}/open-apis/im/v1/messages?receive_id_type={RECEIVE_ID_TYPE}"
            ),
            json!({ "receive_id": chat_id, "content": content, "msg_type": msg_type }),
        )
        .await?;
        Ok(())
    }

    // ── WebSocket helpers ───────────────────────────────────────────

    async fn ws_endpoint(&self) -> Result<(String, i32), ChannelError> {
        let resp = self
            .client
            .post(format!("{FEISHU_BASE_URL}/callback/ws/endpoint"))
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
            .ok_or_else(|| api_err_str("no ws URL"))?
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
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        let text = super::blocks_to_text(&blocks);
        if text.is_empty() {
            return Ok(());
        }

        // Feishu schema 2.0 markdown supports tables/fenced code; old lark_md does not.
        // Note: content must be the card root object, NOT wrapped in {"card": ...}.
        const MAX_MD: usize = 30_000;
        let text = if text.len() > MAX_MD {
            let split = text
                .char_indices()
                .nth(MAX_MD)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!("{}\n\n...(内容已截断)", &text[..split])
        } else {
            text
        };

        let content = json!({
            "schema": "2.0",
            "body": {
                "elements": [{ "tag": "markdown", "content": text }]
            }
        })
        .to_string();
        self.send_msg(&token, external_chat_id, &content, "interactive")
            .await
    }

    async fn send_files(
        &self,
        external_chat_id: &str,
        files: &[(&std::path::Path, Option<&str>)],
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;

        for (path, caption) in files {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|e| api_err("read file", e))?;
            let (form_field, upload_url, key_field, msg_type) = file_upload_info(path);
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();

            let form = reqwest::multipart::Form::new().part(
                form_field,
                reqwest::multipart::Part::bytes(bytes).file_name(file_name),
            );
            let resp = self.upload(&token, &upload_url, form).await?;

            let key = resp["data"][key_field]
                .as_str()
                .ok_or_else(|| api_err_str("no upload key"))?;
            let content = json!({ key_field: key }).to_string();

            self.send_msg(&token, external_chat_id, &content, msg_type)
                .await?;

            if let Some(caption) = caption {
                if !caption.is_empty() {
                    let content = json!({
                        "schema": "2.0",
                        "body": {
                            "elements": [{ "tag": "markdown", "content": caption }]
                        }
                    })
                    .to_string();
                    self.send_msg(&token, external_chat_id, &content, "interactive")
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        let url = format!("{FEISHU_BASE_URL}/open-apis/im/v1/messages/{message_id}/reactions");
        let emoji_type = map_emoji(emoji);

        info!(message_id, emoji, emoji_type, "Feishu sending reaction");

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({ "reaction_type": { "emoji_type": emoji_type } }))
            .send()
            .await
            .map_err(|e| api_err("reaction request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("reaction parse", e))?;

        let code = resp["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(ChannelError::Platform(format!(
                "reaction error {code}: {}",
                resp["msg"].as_str().unwrap_or("unknown")
            )));
        }

        info!(message_id, emoji, "Feishu reaction sent");
        Ok(())
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

        let (body, is_gzip) = if data.len() > 1 && (data[0] == 1 || data[0] == 2) {
            (&data[1..], data[0] == 1)
        } else {
            (data, false)
        };

        if is_gzip {
            warn!("gzip protobuf not supported");
            return Ok(());
        }

        let frame = lark_websocket_protobuf::pbbp2::Frame::decode(body)
            .map_err(|e| api_err_str(&format!("protobuf decode: {e}")))?;

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
                // ACK within 3s
                let ack = build_ack(&frame);
                write
                    .send(tungstenite::Message::Binary(ack.into()))
                    .await
                    .map_err(|e| api_err("ACK", e))?;

                if msg_type == MSG_TYPE_EVENT {
                    if let Some(ref payload) = frame.payload {
                        let text = String::from_utf8_lossy(payload);
                        debug!(payload = %text, "event payload");
                        if let Ok(Some(msg_id)) = self.parse_event(&text, incoming).await {
                            if let Err(e) = self.send_reaction("", &msg_id, "👀").await {
                                warn!(error = %e, "reaction failed");
                            }
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
                if let Ok(Some(msg_id)) = self.parse_event_json(&msg, incoming).await {
                    if let Err(e) = self.send_reaction("", &msg_id, "👀").await {
                        warn!(error = %e, "reaction failed");
                    }
                }
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

    /// Parse event from JSON value (v2.0 or v1.x). Returns `message_id` if a message was forwarded.
    async fn parse_event_json(
        &self,
        msg: &serde_json::Value,
        incoming: &mpsc::Sender<ChannelMessage>,
    ) -> Result<Option<String>, ChannelError> {
        // v2.0: {"schema":"2.0","header":{"event_type":"..."},"event":{"message":{...}}}
        // v1.x: {"type":"im.message.receive_v1","event":{"message":{...}}}
        let event_type = msg["header"]["event_type"]
            .as_str()
            .or_else(|| msg["type"].as_str())
            .unwrap_or("");

        if event_type != "im.message.receive_v1" {
            debug!(event_type, "ignoring non-message event");
            return Ok(None);
        }

        let event = msg.get("event").unwrap_or(msg);
        let message = &event["message"];
        let chat_id = message["chat_id"]
            .as_str()
            .ok_or_else(|| api_err_str("missing chat_id"))?;
        let msg_id = message["message_id"].as_str().unwrap_or("").to_string();
        let user_id = message["sender"]["sender_id"]["union_id"]
            .as_str()
            .or_else(|| message["sender"]["sender_id"]["user_id"].as_str())
            .unwrap_or("unknown");
        let content_str = message["content"].as_str().unwrap_or("{}");

        let content_json: serde_json::Value =
            serde_json::from_str(content_str).unwrap_or_else(|_| json!({}));
        let text = content_json["text"].as_str().unwrap_or("");
        if text.is_empty() {
            return Ok(None);
        }

        let chat_type = message["chat_type"].as_str().unwrap_or("");
        let is_mention = chat_type == "p2p"
            || message["mentions"]
                .as_array()
                .is_some_and(|a| !a.is_empty());

        info!(chat_id, msg_id, user_id, is_mention, text, "Feishu message");

        let channel_msg = ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: user_id.to_string(),
            external_message_id: if msg_id.is_empty() {
                None
            } else {
                Some(msg_id.clone())
            },
            is_mention,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        };

        if incoming.send(channel_msg).await.is_err() {
            return Err(ChannelError::Platform("incoming closed".to_string()));
        }
        Ok(if msg_id.is_empty() {
            None
        } else {
            Some(msg_id)
        })
    }
}

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
    let _ = frame.encode(&mut buf);
    buf
}

fn build_ack(original: &lark_websocket_protobuf::pbbp2::Frame) -> Vec<u8> {
    let mut ack = original.clone();
    ack.payload = Some(r#"{"code":200}"#.as_bytes().to_vec());
    let mut buf = Vec::new();
    let _ = ack.encode(&mut buf);
    buf
}

// ── Emoji mapping ──────────────────────────────────────────────────

#[allow(clippy::match_same_arms)]
fn map_emoji(emoji: &str) -> &'static str {
    // Only these emoji_type values are valid for Feishu reaction API:
    // THUMBSUP, LAUGH, WOW, ANGRY, CLAP, MUSCLE, GIFT, ROSE, SMILE, OK, THINKING, HEART, SALUTE
    match emoji {
        "👍" => "THUMBSUP",
        "😂" => "LAUGH",
        "😮" => "WOW",
        "😠" => "ANGRY",
        "👏" => "CLAP",
        "💪" => "MUSCLE",
        "🎁" => "GIFT",
        "🌹" => "ROSE",
        "😊" => "SMILE",
        "🆗" | "✅" => "OK",
        "🤔" => "THINKING",
        "❤️" | "❤" | "😍" | "💖" => "HEART",
        "🫡" => "SALUTE",
        "👀" => "THINKING",     // Feishu doesn't have EYES, closest is THINKING
        "👎" => "THUMBSUP",     // Feishu doesn't have THUMBSDOWN
        "😢" | "😭" => "LAUGH", // Feishu doesn't have SAD
        "🔥" => "HEART",
        "🎉" => "CLAP",
        "🙏" => "OK",
        "🌞" => "SMILE",
        "☕" => "SMILE",
        "🌸" => "ROSE",
        "🍚" => "OK",
        "💰" => "OK",
        "🏆" => "OK",
        "🧠" => "THINKING",
        "👻" => "OK",
        "✨" => "OK",
        "🦋" => "OK",
        "🪨" => "OK",
        "🎹" => "OK",
        "🎱" => "OK",
        "🚫" => "THUMBSUP",
        "🔔" => "OK",
        "💯" => "OK",
        "🎯" => "OK",
        _ => "OK",
    }
}

// ── Small helpers ──────────────────────────────────────────────────

fn api_err(action: &str, e: impl std::fmt::Display) -> ChannelError {
    ChannelError::Platform(format!("{action} failed: {e}"))
}

fn api_err_str(msg: &str) -> ChannelError {
    ChannelError::Platform(msg.to_string())
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

/// Returns (`form_field`, `upload_url`, `response_key_field`, `msg_type`) for a file path.
fn file_upload_info(path: &std::path::Path) -> (&'static str, String, &'static str, &'static str) {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    if mime.type_() == "image" {
        (
            "image",
            format!("{FEISHU_BASE_URL}/open-apis/im/v1/images"),
            "image_key",
            "image",
        )
    } else {
        (
            "file",
            format!("{FEISHU_BASE_URL}/open-apis/im/v1/files?file_type=stream"),
            "file_key",
            "file",
        )
    }
}
