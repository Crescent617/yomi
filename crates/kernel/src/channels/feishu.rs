use crate::types::ContentBlock;
use futures::{SinkExt, StreamExt};
use lru::LruCache;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::num::NonZeroUsize;
use tokio_tungstenite::tungstenite;

use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::feishu_events::{build_ping, FRAME_TIMEOUT, PING_INTERVAL};
use super::{ChannelError, ChannelEvent, DocPermissionRequest, PlatformAdapter, MAX_RETRY_DELAY};

const FEISHU_BASE_URL: &str = "https://open.feishu.cn";

/// Platform upload caps: images 10MB, files 30MB; empty uploads are also
/// rejected. All violations surface as Feishu's generic API error 234001,
/// so the adapter fails fast with a precise reason instead.
const IMAGE_MAX_BYTES: usize = 10 * 1024 * 1024;

const FILE_MAX_BYTES: usize = 30 * 1024 * 1024;

const RECEIVE_ID_TYPE: &str = "chat_id";

/// Platform error: whole-document comments take no thread replies — the
/// adapter falls back to posting a new whole comment instead.
const WHOLE_COMMENT_NO_REPLY: i64 = 1_069_302;

#[derive(Debug, Clone, Deserialize)]
struct TokenResp {
    code: i32,
    msg: String,
    tenant_access_token: Option<String>,
    expire: Option<i64>,
}

pub(crate) struct TokenCache {
    token: String,
    expires_at: std::time::Instant,
}

/// Bounded dedup of forwarded message IDs: Feishu resends events whose ACK
/// was lost; redeliveries land within seconds, so a few thousand is ample.
const DEDUP_CAP: NonZeroUsize = NonZeroUsize::new(4096).unwrap();

/// Cap for the sent-card text cache (see [`FeishuAdapter::cache_card_text`]).
const SENT_TEXT_CAP: NonZeroUsize = DEDUP_CAP;

/// Cap for the thread-root cache (thread_id → root message id). Threads
/// are few and long-lived; a miss just costs one API re-fetch.
const THREAD_ROOT_CAP: NonZeroUsize = DEDUP_CAP;

/// KV namespace and retention for the sent-card text cache: quoting a
/// reply happens in the same conversation arc, so a week is ample.
const SENT_TEXT_NS: &str = "feishu_sent_card_text";

const SENT_TEXT_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

/// The full-table prune is throttled: `update_card` fires per status-card
/// patch, so per-write pruning would churn cache.db.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_hours(1);

// ── Adapter ─────────────────────────────────────────────────────────

pub struct FeishuAdapter {
    app_id: String,
    app_secret: String,
    client: Client,
    base_url: String,
    token_cache: Mutex<Option<TokenCache>>,
    bot_open_id: tokio::sync::Mutex<Option<String>>,
    pub(crate) seen_messages: Mutex<LruCache<String, ()>>,
    sent_texts: Mutex<LruCache<String, String>>,
    /// Persistent backstop for `sent_texts` (survives restarts); `None`
    /// in tests and when the kernel has no cache db.
    kv: Option<std::sync::Arc<crate::kv_cache::KvCache>>,
    /// Last `sent_texts` prune time (throttled, see PRUNE_INTERVAL).
    last_prune: tokio::sync::Mutex<Option<std::time::Instant>>,
    /// Thread id → root message id, filled by `thread_root_id`. Memory
    /// only: a cold cache after restart costs one refetch per thread.
    thread_roots: tokio::sync::Mutex<LruCache<String, String>>,
}

pub(crate) fn cached_token(cache: Option<&TokenCache>) -> Option<String> {
    cache.and_then(|c| {
        if std::time::Instant::now() < c.expires_at {
            Some(c.token.clone())
        } else {
            None
        }
    })
}

pub(crate) fn api_err(action: &str, e: impl std::fmt::Display) -> ChannelError {
    let e = format!("{e}");
    if e.is_empty() {
        ChannelError::Platform(action.to_string())
    } else {
        ChannelError::Platform(format!("{action}: {e}"))
    }
}

/// Extract `data.<key>` as an owned string from a Feishu API response.
pub(crate) fn resp_data_str(resp: &serde_json::Value, key: &str) -> Option<String> {
    resp["data"][key].as_str().map(str::to_string)
}

pub(crate) fn check_api_resp(resp: serde_json::Value) -> Result<serde_json::Value, ChannelError> {
    let code = resp["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        return Err(ChannelError::Platform(format!(
            "API error {code}: {}",
            resp["msg"].as_str().unwrap_or("unknown")
        )));
    }
    Ok(resp)
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
            seen_messages: Mutex::new(LruCache::new(DEDUP_CAP)),
            sent_texts: Mutex::new(LruCache::new(SENT_TEXT_CAP)),
            kv: None,
            last_prune: tokio::sync::Mutex::new(None),
            thread_roots: tokio::sync::Mutex::new(LruCache::new(THREAD_ROOT_CAP)),
        }
    }

    /// Attach the persistent KV backstop for the sent-card text cache.
    pub fn set_kv_cache(&mut self, kv: Option<std::sync::Arc<crate::kv_cache::KvCache>>) {
        self.kv = kv;
    }

    /// Point the adapter at a different API base URL (tests only).
    #[cfg(test)]
    fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    pub(crate) async fn ensure_bot_open_id(&self, token: &str) -> Option<String> {
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

    pub(crate) async fn get_token(&self) -> Result<String, ChannelError> {
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

    async fn api_delete(&self, token: &str, url: &str) -> Result<serde_json::Value, ChannelError> {
        let resp = self
            .client
            .delete(url)
            .header("Authorization", format!("Bearer {token}"))
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

    /// Download an image from a received message and encode it as a
    /// base64 data URL (mirrors the Telegram adapter), so it can ride
    /// along as an `ImageUrl` content block for the model.
    ///
    /// Message resources (including images) must go through the message
    /// resources endpoint — `/im/v1/images/{key}` only serves images the
    /// app uploaded itself and rejects message keys with HTTP 400.
    async fn download_image(
        &self,
        token: &str,
        message_id: &str,
        image_key: &str,
    ) -> Result<String, ChannelError> {
        let resp = self
            .client
            .get(format!(
                "{}/open-apis/im/v1/messages/{message_id}/resources/{image_key}",
                self.base_url
            ))
            .query(&[("type", "image")])
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| api_err("image download", e))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let body = crate::utils::strs::truncate_with_suffix(&body, 200, "...");
            return Err(ChannelError::Platform(format!(
                "image download HTTP {status}: {body}"
            )));
        }
        let bytes = resp.bytes().await.map_err(|e| api_err("image read", e))?;
        if bytes.len() as u64 > crate::utils::image::MAX_IMAGE_SIZE {
            return Err(ChannelError::Platform(format!(
                "image too large: {} bytes (max: {})",
                bytes.len(),
                crate::utils::image::MAX_IMAGE_SIZE
            )));
        }
        // A rejected key comes back as a JSON error body, which fails
        // magic-byte detection inside bytes_to_data_url.
        let compressed = crate::utils::image::needs_compression(&bytes);
        let data_url = crate::utils::image::bytes_to_data_url_async(bytes.clone())
            .await
            .map_err(|e| ChannelError::Platform(format!("image download: {e}")))?;
        info!(
            image_key,
            bytes = bytes.len(),
            compressed,
            "image downloaded"
        );
        Ok(data_url)
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
        self.send_msg_to(token, RECEIVE_ID_TYPE, chat_id, content, msg_type)
            .await
    }

    /// Send a message to an arbitrary receive id type (`chat_id` for group
    /// or p2p chats, `open_id` to DM a user directly — the p2p chat is
    /// used/created implicitly).
    async fn send_msg_to(
        &self,
        token: &str,
        receive_id_type: &str,
        receive_id: &str,
        content: &str,
        msg_type: &str,
    ) -> Result<Option<String>, ChannelError> {
        let resp = self
            .api_post(
                token,
                &format!(
                    "{}/open-apis/im/v1/messages?receive_id_type={receive_id_type}",
                    self.base_url
                ),
                json!({ "receive_id": receive_id, "content": content, "msg_type": msg_type }),
            )
            .await?;
        let id = resp_data_str(&resp, "message_id");
        if msg_type == "interactive" {
            self.cache_card_text(id.as_deref(), content).await;
        }
        Ok(id)
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
        let id = resp_data_str(&resp, "message_id");
        if msg_type == "interactive" {
            self.cache_card_text(id.as_deref(), content).await;
        }
        Ok(id)
    }

    /// Cache a sent card's markdown body by message id. `fetch_message` asks
    /// the API for the real card body (`card_msg_content_type`), so this cache
    /// is only a backstop for when that echo still degrades to the legacy
    /// "请升级至最新版本客户端" placeholder (very old cards, edge cases).
    /// Writes through to the persistent KV cache (best-effort) so restarts
    /// keep the text.
    async fn cache_card_text(&self, msg_id: Option<&str>, content: &str) {
        let Some(msg_id) = msg_id else { return };
        let Ok(card) = serde_json::from_str::<serde_json::Value>(content) else {
            return;
        };
        let text = Self::extract_card_text(&card);
        if text.is_empty() {
            return;
        }
        self.sent_texts
            .lock()
            .await
            .put(msg_id.to_string(), text.clone());
        let Some(kv) = &self.kv else { return };
        if let Err(e) = kv.put(SENT_TEXT_NS, msg_id, &text).await {
            warn!(error = %e, "sent-text kv put failed");
        }
        let mut last = self.last_prune.lock().await;
        if last.is_none_or(|t| t.elapsed() >= PRUNE_INTERVAL) {
            let cutoff = chrono::Utc::now().timestamp_millis() - SENT_TEXT_TTL_MS;
            if let Err(e) = kv.prune_older_than(SENT_TEXT_NS, cutoff).await {
                warn!(error = %e, "sent-text kv prune failed");
            }
            *last = Some(std::time::Instant::now());
        }
    }

    /// The text of one of our own cards: memory first, persistent KV as
    /// the backstop (a hit backfills memory).
    async fn sent_card_text(&self, msg_id: &str) -> Option<String> {
        if let Some(text) = self.sent_texts.lock().await.get(msg_id) {
            return Some(text.clone());
        }
        let kv = self.kv.as_ref()?;
        match kv.get(SENT_TEXT_NS, msg_id).await {
            Ok(Some(text)) => {
                self.sent_texts
                    .lock()
                    .await
                    .put(msg_id.to_string(), text.clone());
                Some(text)
            }
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "sent-text kv get failed");
                None
            }
        }
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

    /// Post a new whole-document comment carrying `body` (a reply-shaped
    /// content payload) and return its comment id.
    async fn create_whole_comment(
        &self,
        token: &str,
        file_token: &str,
        file_type: &str,
        body: &serde_json::Value,
    ) -> Result<Option<String>, ChannelError> {
        // The create-comment API wraps the reply in a `reply_list`
        // (E2E-verified: a bare `{content}` body is rejected with
        // 9499 "Missing required parameter: ReplyList").
        let create_body = json!({ "reply_list": { "replies": [body] } });
        let resp = self
            .api_post(
                token,
                &format!(
                    "{}/open-apis/drive/v1/files/{file_token}/comments?file_type={file_type}&user_id_type=open_id",
                    self.base_url
                ),
                create_body,
            )
            .await?;
        Ok(resp_data_str(&resp, "comment_id"))
    }

    /// The working body of `fetch_doc_comment` (split out so the caller
    /// can wrap the whole thing in one 5s timeout).
    async fn fetch_doc_comment_inner(
        &self,
        token: &str,
        file_token: &str,
        file_type: &str,
        comment_id: &str,
    ) -> Result<Option<super::DocCommentDetail>, ChannelError> {
        let replies_url = format!(
            "{}/open-apis/drive/v1/files/{file_token}/comments/{comment_id}/replies",
            self.base_url
        );
        let batch_url = format!(
            "{}/open-apis/drive/v1/files/{file_token}/comments/batch_query?file_type={file_type}&user_id_type=open_id",
            self.base_url
        );
        let mut query = vec![
            ("file_type", file_type.to_string()),
            ("user_id_type", "open_id".to_string()),
            ("page_size", "50".to_string()),
        ];
        // The timeline (list endpoint) is required; quote/is_whole ride
        // batch_query (its only source). Both lag the event — the caller
        // retries while the trigger reply or is_whole is unreadable.
        let (first_page, batch_resp) = tokio::join!(
            self.api_get(token, &replies_url, &query),
            self.api_post(token, &batch_url, json!({ "comment_ids": [comment_id] })),
        );
        let batch_item = batch_resp.ok().and_then(|resp| {
            resp["data"]["items"]
                .as_array()
                .and_then(|a| a.first().cloned())
        });
        // Pages are chronological; long threads need the later pages (the
        // triggering reply is the newest). Bounded at 5 pages.
        let mut page = first_page?;
        let mut reply_items = Vec::new();
        for _ in 0..5 {
            let Some(items) = page["data"]["items"].as_array() else {
                break;
            };
            reply_items.extend(items.iter().cloned());
            if !page["data"]["has_more"].as_bool().unwrap_or(false) {
                break;
            }
            let Some(page_token) = page["data"]["page_token"].as_str().map(str::to_string) else {
                break;
            };
            query.retain(|(k, _)| *k != "page_token");
            query.push(("page_token", page_token));
            page = self.api_get(token, &replies_url, &query).await?;
        }
        if reply_items.is_empty() {
            return Ok(None);
        }
        let bot_open_id = self.ensure_bot_open_id(token).await;
        let replies = reply_items
            .iter()
            .map(|r| {
                let user_id = r["user_id"].as_str().unwrap_or_default().to_string();
                super::DocCommentReplyLite {
                    is_from_bot: bot_open_id.as_deref() == Some(user_id.as_str())
                        && !user_id.is_empty(),
                    user_id,
                    reply_id: r["reply_id"].as_str().unwrap_or_default().to_string(),
                    create_time: r["create_time"]
                        .as_i64()
                        .or_else(|| r["create_time"].as_str()?.parse().ok())
                        .unwrap_or_default(),
                    text: Self::extract_reply_text(
                        r["content"]["elements"].as_array(),
                        bot_open_id.as_deref(),
                    ),
                }
            })
            .collect();
        Ok(Some(super::DocCommentDetail {
            // None = batch_query has not caught up with the event yet
            // (the caller retries — the session mapping keys off this).
            is_whole: batch_item
                .as_ref()
                .map(|item| item["is_whole"].as_bool().unwrap_or_default()),
            quote: batch_item.as_ref().and_then(|item| {
                item["quote"]
                    .as_str()
                    .filter(|q| !q.is_empty())
                    .map(str::to_string)
            }),
            replies,
        }))
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for FeishuAdapter {
    async fn run_receiver(
        &self,
        incoming: mpsc::Sender<ChannelEvent>,
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
                let mut interval = tokio::time::interval(PING_INTERVAL);
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

            // Receive loop. The timeout catches zombie connections: any
            // inbound frame (a pong every PING_INTERVAL) proves liveness;
            // silence means half-open TCP — break to reconnect.
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    msg = tokio::time::timeout(FRAME_TIMEOUT, read.next()) => {
                        let msg = match msg {
                            Ok(Some(msg)) => msg,
                            Ok(None) => {
                                warn!("ws stream ended");
                                break;
                            }
                            Err(_) => {
                                warn!(
                                    timeout_secs = FRAME_TIMEOUT.as_secs(),
                                    "no frame within timeout (pong overdue), reconnecting"
                                );
                                break;
                            }
                        };
                        match msg {
                            Ok(tungstenite::Message::Binary(data)) => {
                                if let Err(e) = self.handle_binary(&data, &incoming, &write).await {
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

    /// DM a card to a user by `open_id` (`receive_id_type=open_id`).
    async fn send_direct_card(
        &self,
        user_id: &str,
        card_json: &str,
    ) -> Result<Option<String>, ChannelError> {
        let token = self.get_token().await?;
        self.send_msg_to(&token, "open_id", user_id, card_json, "interactive")
            .await
    }

    /// Grant collaborator permission to all applicants of the request in
    /// one batch call. `need_notification=true` lets Feishu notify approved
    /// users itself; denial has no API (not approving *is* the denial).
    async fn grant_doc_permission(
        &self,
        file_token: &str,
        file_type: &str,
        req: &DocPermissionRequest,
        perm: &str,
    ) -> Result<(), ChannelError> {
        let mut members = Vec::new();
        members.extend(req.applicant_users.iter().map(
            |id| json!({ "member_type": "openid", "member_id": id, "perm": perm, "type": "user" }),
        ));
        members.extend(req.applicant_chats.iter().map(|id| {
            json!({ "member_type": "openchat", "member_id": id, "perm": perm, "type": "chat" })
        }));
        members.extend(req.applicant_departments.iter().map(|id| {
            json!({ "member_type": "opendepartmentid", "member_id": id, "perm": perm, "type": "department" })
        }));
        if members.is_empty() {
            return Err(ChannelError::Platform("no applicants to grant".into()));
        }
        let token = self.get_token().await?;
        let resp = self
            .api_post(
                &token,
                &format!(
                    "{}/open-apis/drive/v1/permissions/{file_token}/members/batch_create?type={file_type}&need_notification=true",
                    self.base_url
                ),
                json!({ "members": &members }),
            )
            .await?;
        // Partial grant failures (R4) are handled manually for now — the
        // response data is logged so a mismatch is at least visible.
        debug!(
            file_token,
            requested = members.len(),
            response = %resp["data"],
            "doc permission grant response"
        );
        Ok(())
    }

    /// Resolve a document's display title via the drive meta API (for
    /// human-friendly notification cards).
    async fn fetch_doc_title(&self, file_token: &str, file_type: &str) -> Option<String> {
        let token = self.get_token().await.ok()?;
        let resp = self
            .api_post(
                &token,
                &format!("{}/open-apis/drive/v1/metas/batch_query", self.base_url),
                json!({
                    "request_docs": [{ "doc_token": file_token, "doc_type": file_type }],
                    "with_url": false,
                }),
            )
            .await
            .ok()?;
        resp["data"]["metas"][0]["title"]
            .as_str()
            .map(str::to_string)
    }

    /// refer: replies list <https://open.feishu.cn/document/server-docs/docs/CommentAPI/list>;
    /// `batch_query` <https://open.feishu.cn/document/server-docs/docs/CommentAPI/batch_query>
    /// The replies timeline comes from the list endpoint (fresh reads),
    /// while `quote`/`is_whole` ride `batch_query` (the only source for
    /// them). E2E-verified: `batch_query`'s `reply_list` lags the event by
    /// seconds to minutes — fetching the timeline from it injected STALE
    /// text. Bounded at 5s overall: this runs on the hub's dispatch path.
    async fn fetch_doc_comment(
        &self,
        file_token: &str,
        file_type: &str,
        comment_id: &str,
    ) -> Result<Option<super::DocCommentDetail>, ChannelError> {
        let token = self.get_token().await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.fetch_doc_comment_inner(&token, file_token, file_type, comment_id),
        )
        .await
        .map_err(|_| ChannelError::Platform("comment fetch timed out".into()))?
    }

    /// refer: <https://open.feishu.cn/document/server-docs/docs/CommentAPI/reply>
    /// Whole comments take no thread replies (platform error 1069302) —
    /// the answer goes out as a new whole comment. The
    /// [`super::WHOLE_COMMENT_ID`] sentinel (the shared whole-comment
    /// session's delivery target) skips the doomed reply attempt and
    /// creates the new comment directly.
    async fn reply_doc_comment(
        &self,
        file_token: &str,
        file_type: &str,
        comment_id: &str,
        text: &str,
    ) -> Result<Option<String>, ChannelError> {
        let token = self.get_token().await?;
        let body = json!({
            "content": { "elements": [{ "type": "text_run", "text_run": { "text": text } }] }
        });
        if comment_id == super::WHOLE_COMMENT_ID {
            return self
                .create_whole_comment(&token, file_token, file_type, &body)
                .await;
        }
        let url = format!(
            "{}/open-apis/drive/v1/files/{file_token}/comments/{comment_id}/replies?file_type={file_type}&user_id_type=open_id",
            self.base_url
        );
        // Raw post (not `api_post`): the 1069302 fallback needs the code.
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| api_err("comment reply request", e))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| api_err("comment reply parse", e))?;
        if resp["code"].as_i64() == Some(WHOLE_COMMENT_NO_REPLY) {
            return self
                .create_whole_comment(&token, file_token, file_type, &body)
                .await;
        }
        let resp = check_api_resp(resp)?;
        Ok(resp_data_str(&resp, "reply_id").or_else(|| resp_data_str(&resp, "comment_id")))
    }

    /// refer: <https://open.feishu.cn/document/server-docs/docs/CommentAPI/reaction>
    /// E2E-verified: keyed by **reply** id (a comment id is rejected with
    /// 1061001), and the IM emoji vocabulary applies (`OneSecond`/…).
    async fn react_doc_comment(
        &self,
        file_token: &str,
        file_type: &str,
        reply_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        self.api_post(
            &token,
            &format!(
                "{}/open-apis/drive/v2/files/{file_token}/comments/reaction?file_type={file_type}",
                self.base_url
            ),
            json!({ "action": "add", "reply_id": reply_id, "reaction_type": emoji }),
        )
        .await?;
        Ok(())
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
        // The card morphed (status → reply) — refresh the cached text.
        self.cache_card_text(Some(message_id), card_json).await;
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

    /// refer: <https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/message-reaction/delete>
    async fn delete_reaction(
        &self,
        _external_chat_id: &str,
        message_id: &str,
        reaction_id: &str,
    ) -> Result<(), ChannelError> {
        let token = self.get_token().await?;
        self.api_delete(
            &token,
            &format!(
                "{}/open-apis/im/v1/messages/{message_id}/reactions/{reaction_id}",
                self.base_url
            ),
        )
        .await?;
        Ok(())
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
            // Echo real card bodies (not the legacy placeholder) so card
            // history keeps its markdown; collapsed panels stay excluded.
            ("card_msg_content_type", "user_card_content".to_string()),
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
            // Skip only ourselves — our own replies are already in the
            // session's conversation, re-injecting them as chat history
            // is pure duplication. Other apps' messages stay: they can
            // be real context (CI bots, other assistants).
            let sender = &item["sender"];
            if sender["sender_type"].as_str() == Some("app")
                && sender["id"].as_str() == Some(self.app_id.as_str())
            {
                continue;
            }
            let (text, image_keys) = Self::extract_history_content(item);
            if text.trim().is_empty() {
                continue;
            }
            out.push(super::HistoryMessage {
                message_id: item["message_id"].as_str().unwrap_or("").to_string(),
                create_time,
                sender_id: sender["id"].as_str().unwrap_or("").to_string(),
                text,
                image_keys,
                parent_id: item["parent_id"].as_str().map(str::to_string),
            });
        }
        // The API returns newest-first; assemble chronologically.
        out.reverse();
        Ok(out)
    }

    /// refer: <https://open.feishu.cn/document/server-docs/im-v1/message/list>
    /// The thread's root is its earliest message — one ascending fetch,
    /// cached per thread so repeat calls are free.
    async fn thread_root_id(&self, thread_id: &str) -> Result<Option<String>, ChannelError> {
        if let Some(root) = self.thread_roots.lock().await.get(thread_id) {
            return Ok(Some(root.clone()));
        }
        let token = self.get_token().await?;
        let query = [
            ("container_id_type", "thread".to_string()),
            ("container_id", thread_id.to_string()),
            ("sort_type", "ByCreateTimeAsc".to_string()),
            ("page_size", "1".to_string()),
        ];
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/messages", self.base_url),
                &query,
            ),
        )
        .await
        .map_err(|_| ChannelError::Platform("thread root fetch timed out".into()))??;
        let root = resp["data"]["items"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|m| m["message_id"].as_str())
            .map(str::to_string);
        if let Some(root) = &root {
            self.thread_roots
                .lock()
                .await
                .put(thread_id.to_string(), root.clone());
        }
        Ok(root)
    }

    /// refer: <https://open.feishu.cn/document/server-docs/im-v1/message/get>
    async fn fetch_message(
        &self,
        message_id: &str,
    ) -> Result<Option<super::HistoryMessage>, ChannelError> {
        let token = self.get_token().await?;
        // `card_msg_content_type=user_card_content` (undocumented but stable,
        // verified) makes interactive cards echo their real schema 2.0 body
        // instead of the legacy "请升级至最新版本客户端" placeholder — for any
        // sender, not just our own cards.
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/messages/{message_id}", self.base_url),
                &[("card_msg_content_type", "user_card_content".to_string())],
            ),
        )
        .await
        .map_err(|_| ChannelError::Platform("message fetch timed out".into()))??;
        let Some(item) = resp["data"]["items"].as_array().and_then(|a| a.first()) else {
            return Ok(None);
        };
        if item["deleted"].as_bool().unwrap_or(false) {
            return Ok(None);
        }
        // No sender filter here — quoting the bot's own answer is a
        // primary use case. With `card_msg_content_type` the API echoes the
        // real card body for any sender, so it wins; our sent-text cache only
        // backfills when the API still degraded to the `[interactive]`
        // placeholder (very old cards, edge cases).
        let (text, image_keys) = Self::extract_history_content(item);
        let text = if item["msg_type"].as_str() == Some("interactive") && text == "[interactive]" {
            self.sent_card_text(message_id).await.unwrap_or(text)
        } else {
            text
        };
        Ok(Some(super::HistoryMessage {
            message_id: item["message_id"]
                .as_str()
                .unwrap_or(message_id)
                .to_string(),
            create_time: item["create_time"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or_default(),
            sender_id: item["sender"]["id"].as_str().unwrap_or("").to_string(),
            text,
            image_keys,
            parent_id: item["parent_id"].as_str().map(str::to_string),
        }))
    }

    /// refer: <https://open.feishu.cn/document/server-docs/contact-v3/user/get>
    async fn fetch_user_name(&self, open_id: &str) -> Option<String> {
        if open_id.is_empty() {
            return None;
        }
        let token = self.get_token().await.ok()?;
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/contact/v3/users/{open_id}", self.base_url),
                &[("user_id_type", "open_id".to_string())],
            ),
        )
        .await
        .map_err(|_| ChannelError::Platform("user fetch timed out".into()))
        .ok()?
        .ok()?;
        // Deployments without contact permission get code 0 but no name.
        resp["data"]["user"]["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    }

    /// Build a click-to-jump applink for a message (no official
    /// get-message-link API; the client-copied link is an opaque token).
    /// Same construction as Feishu's own lark-cli: the jump targets a
    /// message by its **position** — `message_position` at chat level,
    /// `thread_message_position` inside a thread — fetched with one extra
    /// get-message call.
    async fn message_link(&self, chat_id: &str, message_id: &str) -> Option<String> {
        let token = self.get_token().await.ok()?;
        // Bounded like the history/message fetches: this runs on the hub's
        // single event-forwarding loop — a hung API must not stall it.
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/messages/{message_id}", self.base_url),
                &[],
            ),
        )
        .await
        .map_err(|_| warn!(message_id, "message_link fetch timed out"))
        .ok()?
        .map_err(|e| warn!(error = %e, message_id, "message_link fetch failed"))
        .ok()?;
        let item = resp["data"]["items"].as_array()?.first()?;
        // A thread's root message shows up in the main chat flow with a
        // normal (non-negative) position — the verified chat link applies.
        // Messages inside a thread have a negative chat position; there
        // the thread link (positioned within the thread) must be used.
        let chat_pos = item["message_position"]
            .as_str()
            .and_then(|p| p.parse::<i64>().ok());
        if let Some(pos) = chat_pos {
            if pos >= 0 {
                return Some(format!(
                    "https://applink.feishu.cn/client/chat/open?openChatId={chat_id}&position={pos}"
                ));
            }
        }
        if let Some(thread_id) = item["thread_id"].as_str() {
            // thread_position: the root is -1, replies count from 0.
            let pos = item["thread_message_position"].as_str().unwrap_or("0");
            return Some(format!(
                "https://applink.feishu.cn/client/thread/open?open_chat_id={chat_id}&open_thread_id={thread_id}&openchatid={chat_id}&openthreadid={thread_id}&thread_position={pos}"
            ));
        }
        None
    }

    /// The plain chat applink — no API call, the construction is the one
    /// `message_link`'s chat branch uses.
    async fn chat_link(&self, chat_id: &str) -> Option<String> {
        Some(format!(
            "https://applink.feishu.cn/client/chat/open?openChatId={chat_id}"
        ))
    }

    /// The position-less thread applink (jumps to the thread, not a
    /// specific message): the thread id is read off any message in the
    /// thread (one fetch; a thread's root has it backfilled).
    async fn thread_link(&self, chat_id: &str, message_id: &str) -> Option<String> {
        let token = self.get_token().await.ok()?;
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/messages/{message_id}", self.base_url),
                &[],
            ),
        )
        .await
        .map_err(|_| warn!(message_id, "thread_link fetch timed out"))
        .ok()?
        .map_err(|e| warn!(error = %e, message_id, "thread_link fetch failed"))
        .ok()?;
        let thread_id = resp["data"]["items"].as_array()?.first()?["thread_id"].as_str()?;
        Some(format!(
            "https://applink.feishu.cn/client/thread/open?open_chat_id={chat_id}&open_thread_id={thread_id}&openchatid={chat_id}&openthreadid={thread_id}"
        ))
    }

    /// The chat's display name for notification text; p2p chats have no
    /// name (the caller falls back to a generic wording).
    async fn fetch_chat_name(&self, chat_id: &str) -> Option<String> {
        let token = self.get_token().await.ok()?;
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.api_get(
                &token,
                &format!("{}/open-apis/im/v1/chats/{chat_id}", self.base_url),
                &[],
            ),
        )
        .await
        .map_err(|_| warn!(chat_id, "chat name fetch timed out"))
        .ok()?
        .map_err(|e| warn!(error = %e, chat_id, "chat name fetch failed"))
        .ok()?;
        resp["data"]["name"]
            .as_str()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    }

    async fn download_message_image(
        &self,
        message_id: &str,
        image_key: &str,
    ) -> Result<ContentBlock, ChannelError> {
        let token = self.get_token().await?;
        let data_url = self.download_image(&token, message_id, image_key).await?;
        Ok(ContentBlock::ImageUrl {
            image_url: data_url.into(),
        })
    }
}

// ── Message handlers ────────────────────────────────────────────────

#[cfg(test)]
#[path = "feishu_test.rs"]
mod tests;
