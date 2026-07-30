use super::strip_bot_mention;
use crate::channels::PlatformAdapter;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn strip_bot_mention_preserves_other_mentions() {
    let mentions = vec![
        json!({ "key": "@_user_1", "id": { "open_id": "bot-id" } }),
        json!({ "key": "@_user_2", "id": { "open_id": "alice-id" } }),
    ];

    let raw_text = strip_bot_mention(
        "@_user_1 /steer ask @_user_2 to inspect logs",
        Some(&mentions),
        Some("bot-id"),
    );

    assert_eq!(raw_text, "/steer ask @_user_2 to inspect logs");
}

#[test]
fn strip_bot_mention_keeps_text_when_bot_id_is_unknown() {
    let mentions = vec![json!({
        "key": "@_user_1",
        "id": { "open_id": "someone-else" }
    })];

    let raw_text = strip_bot_mention("@_user_1 hello", Some(&mentions), None);

    assert_eq!(raw_text, "@_user_1 hello");
}

// ── Minimal HTTP stub for request-construction tests ───────────────

/// Captured request: (method, path-with-query, body).
type Captured = (String, String, String);

struct StubFeishu {
    base_url: String,
    requests: Arc<Mutex<Vec<Captured>>>,
}

impl StubFeishu {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let reqs = Arc::clone(&requests);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let reqs = Arc::clone(&reqs);
                tokio::spawn(async move {
                    let (method, path, body) = read_request(&mut sock).await;
                    reqs.lock()
                        .unwrap()
                        .push((method.clone(), path.clone(), body));
                    let resp = response_for(&method, &path);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n",
                        resp.len(),
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&resp).await;
                });
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            requests,
        }
    }

    fn find(&self, method: &str, path_prefix: &str) -> Captured {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|(m, p, _)| m == method && p.starts_with(path_prefix))
            .unwrap_or_else(|| panic!("no {method} {path_prefix}* request captured"))
            .clone()
    }

    fn body_json(req: &Captured) -> serde_json::Value {
        serde_json::from_str(&req.2).unwrap()
    }
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Captured {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = sock.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before headers");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4) {
            break pos;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut parts = headers.lines().next().unwrap().split_whitespace();
    let method = parts.next().unwrap().to_string();
    let path = parts.next().unwrap().to_string();
    let content_length: usize = headers
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = sock.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before full body");
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    (method, path, String::from_utf8_lossy(&body).to_string())
}

/// A real tiny PNG — the compression pipeline reads dimensions from the
/// header, so magic-bytes-only fixtures no longer pass.
fn ok_png() -> &'static [u8] {
    static PNG: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    PNG.get_or_init(|| {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image::ImageBuffer::from_pixel(
            8,
            8,
            image::Rgb([1u8, 2, 3]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
        out
    })
}

fn response_for(method: &str, path: &str) -> Vec<u8> {
    // Message resource download (received images/files): `img_ok` serves
    // PNG bytes, anything else a JSON error body (which must fail
    // magic-byte detection).
    if method == "GET" && path.contains("/resources/") {
        if path.starts_with("/open-apis/im/v1/messages/om_1/resources/img_ok?type=image") {
            return ok_png().to_vec();
        }
        return br#"{"code":234001,"msg":"no such image"}"#.into();
    }
    // Query-marked variant for history edge cases (boundary/deleted/empty/
    // placeholder/same-second).
    if method == "GET"
        && path.starts_with("/open-apis/im/v1/messages?")
        && path.contains("start_time=1700000060")
    {
        return r#"{"code":0,"msg":"ok","data":{"items":[
            {"message_id":"e4","create_time":"1700000060800","msg_type":"text","deleted":false,
             "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"same second later\"}"}},
            {"message_id":"e3","create_time":"1700000060400","msg_type":"text","deleted":true,
             "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"deleted\"}"}},
            {"message_id":"e2","create_time":"1700000060200","msg_type":"text","deleted":false,
             "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"\"}"}},
            {"message_id":"e1","create_time":"1700000060100","msg_type":"image","deleted":false,
             "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"image_key\":\"img_h1\"}"}},
            {"message_id":"e0","create_time":"1700000060000","msg_type":"text","deleted":false,
             "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"boundary\"}"}}
        ]}}"#
        .into();
    }
    let p = path.split('?').next().unwrap_or(path);
    match method {
        "POST" if p == "/open-apis/auth/v3/tenant_access_token/internal" => {
            r#"{"code":0,"msg":"ok","tenant_access_token":"tok-1","expire":7200}"#.into()
        }
        "POST" if p == "/open-apis/im/v1/messages" => {
            r#"{"code":0,"msg":"ok","data":{"message_id":"om_new"}}"#.into()
        }
        "POST" if p == "/open-apis/im/v1/files" => {
            r#"{"code":0,"msg":"ok","data":{"file_key":"fk_1"}}"#.into()
        }
        "POST" if p == "/open-apis/im/v1/images" => {
            r#"{"code":0,"msg":"ok","data":{"image_key":"ik_1"}}"#.into()
        }
        "POST" if p.starts_with("/open-apis/im/v1/messages/") && p.ends_with("/reply") => {
            r#"{"code":0,"msg":"ok","data":{"message_id":"om_reply"}}"#.into()
        }
        "POST" if p.starts_with("/open-apis/im/v1/messages/") && p.ends_with("/reactions") => {
            r#"{"code":0,"msg":"ok","data":{"reaction_id":"rid_1"}}"#.into()
        }
        "GET" if p == "/open-apis/im/v1/messages" => {
            r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"m3","create_time":"1700000060000","msg_type":"text","deleted":false,
                 "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"最新\"}"}},
                {"message_id":"m2","create_time":"1700000050000","msg_type":"post","deleted":false,
                 "sender":{"id":"ou_b","sender_type":"user"},
                 "body":{"content":"{\"zh_cn\":{\"content\":[[{\"tag\":\"text\",\"text\":\"第一段 \"},{\"tag\":\"text\",\"text\":\"第二段\"}],[{\"tag\":\"img\",\"image_key\":\"img_p1\"}]]}}"}},
                {"message_id":"m1","create_time":"1700000040000","msg_type":"text","deleted":false,
                 "sender":{"id":"ou_bot","sender_type":"app"},"body":{"content":"{\"text\":\"bot said\"}"}},
                {"message_id":"m0","create_time":"1700000010000","msg_type":"text","deleted":false,
                 "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"too old\"}"}}
            ]}}"#
            .into()
        }
        "PATCH" if p.starts_with("/open-apis/im/v1/messages/") => {
            r#"{"code":0,"msg":"ok"}"#.into()
        }
        "DELETE" if p.contains("/reactions/") => r#"{"code":0,"msg":"ok"}"#.into(),
        _ => r#"{"code":999,"msg":"unexpected request"}"#.into(),
    }
}

fn stub_adapter(base_url: &str) -> super::FeishuAdapter {
    super::FeishuAdapter::new("app".to_string(), "secret".to_string())
        .with_base_url(base_url.to_string())
}

// ── Request-construction tests ──────────────────────────────────────

#[tokio::test]
async fn send_card_posts_interactive_message_and_returns_id() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let card = r#"{"schema":"2.0","body":{"elements":[]}}"#;

    let id = adapter.send_card("oc_chat", card, None).await.unwrap();

    assert_eq!(id.as_deref(), Some("om_new"));
    let req = stub.find("POST", "/open-apis/im/v1/messages?receive_id_type=chat_id");
    let v = StubFeishu::body_json(&req);
    assert_eq!(v["receive_id"], "oc_chat");
    assert_eq!(v["msg_type"], "interactive");
    assert_eq!(v["content"], card);
}

#[tokio::test]
async fn send_card_with_anchor_uses_reply_api() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let card = r#"{"schema":"2.0"}"#;

    let id = adapter
        .send_card("oc_chat", card, Some("om_anchor"))
        .await
        .unwrap();

    assert_eq!(id.as_deref(), Some("om_reply"));
    let req = stub.find("POST", "/open-apis/im/v1/messages/om_anchor/reply");
    let v = StubFeishu::body_json(&req);
    assert_eq!(v["msg_type"], "interactive");
    assert_eq!(v["content"], card);
    assert_eq!(v["reply_in_thread"], true);
}

#[tokio::test]
async fn update_card_patches_message_content() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let card = r#"{"schema":"2.0","header":{"template":"green"}}"#;

    adapter.update_card("om_1", card).await.unwrap();

    let req = stub.find("PATCH", "/open-apis/im/v1/messages/om_1");
    assert_eq!(StubFeishu::body_json(&req)["content"], card);
}

#[tokio::test]
async fn send_reaction_posts_emoji_and_returns_reaction_id() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let id = adapter.send_reaction("", "om_1", "DONE").await.unwrap();

    assert_eq!(id.as_deref(), Some("rid_1"));
    let req = stub.find("POST", "/open-apis/im/v1/messages/om_1/reactions");
    assert_eq!(
        StubFeishu::body_json(&req)["reaction_type"]["emoji_type"],
        "DONE"
    );
}

#[tokio::test]
async fn fetch_history_queries_filters_and_orders() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let out = adapter
        .fetch_history(
            &crate::channels::HistoryContainer::Thread("omt_1".into()),
            Some(1_700_000_030_000),
            20,
        )
        .await
        .unwrap();

    // Query shape: thread container, desc sort, start_time in seconds
    // (the cursor keeps millisecond precision).
    let req = stub.find("GET", "/open-apis/im/v1/messages?");
    let (_, path, _) = &req;
    assert!(path.contains("container_id_type=thread"), "path: {path}");
    assert!(path.contains("container_id=omt_1"), "path: {path}");
    assert!(path.contains("sort_type=ByCreateTimeDesc"), "path: {path}");
    assert!(path.contains("page_size=20"), "path: {path}");
    assert!(path.contains("start_time=1700000030"), "path: {path}");

    // m0 dropped (older than cursor), m1 dropped (app sender); text and
    // post extracted; result is chronological (oldest first).
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].message_id, "m2");
    assert_eq!(out[0].text, "第一段 第二段");
    assert_eq!(out[0].image_keys, vec!["img_p1".to_string()]);
    assert_eq!(out[0].create_time, 1_700_000_050_000);
    assert_eq!(out[1].message_id, "m3");
    assert_eq!(out[1].text, "最新");
    assert!(out[1].image_keys.is_empty());
}

#[tokio::test]
async fn fetch_history_without_cursor_uses_chat_container() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let _ = adapter
        .fetch_history(
            &crate::channels::HistoryContainer::Chat("oc_1".into()),
            None,
            5,
        )
        .await
        .unwrap();

    let (_, path, _) = &stub.find("GET", "/open-apis/im/v1/messages?");
    assert!(path.contains("container_id_type=chat"), "path: {path}");
    assert!(path.contains("container_id=oc_1"), "path: {path}");
    assert!(path.contains("page_size=5"), "path: {path}");
    assert!(!path.contains("start_time"), "no cursor: {path}");
}

#[tokio::test]
async fn fetch_history_edge_cases_and_millisecond_cursor() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let out = adapter
        .fetch_history(
            &crate::channels::HistoryContainer::Chat("oc_1".into()),
            Some(1_700_000_060_000),
            20,
        )
        .await
        .unwrap();

    // start_time converted to seconds.
    let (_, path, _) = &stub.find("GET", "/open-apis/im/v1/messages?");
    assert!(path.contains("start_time=1700000060"), "path: {path}");

    // e0 dropped (create_time == cursor), e2 dropped (empty text), e3
    // dropped (deleted); e1 kept as a placeholder; e4 kept — the
    // millisecond cursor: e4 is in the SAME second as the cursor
    // (60.8s vs 60.0s) and a second-granularity cursor would drop it.
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].message_id, "e1");
    assert_eq!(out[0].text, "[image]");
    assert_eq!(out[0].image_keys, vec!["img_h1".to_string()]);
    assert_eq!(out[1].message_id, "e4");
    assert_eq!(out[1].text, "same second later");
    assert_eq!(out[1].create_time, 1_700_000_060_800);
}

// ── send_files: multipart form contract (regression for API error 234001) ──

#[tokio::test]
async fn send_files_uploads_file_with_required_form_fields() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report.pdf");
    std::fs::write(&file, b"%PDF-1 fake").unwrap();

    adapter
        .send_files("oc_chat", &[(file.as_path(), None)], None)
        .await
        .unwrap();

    // file_type/file_name are multipart FORM FIELDS, not URL query params.
    let (method, path, body) = stub.find("POST", "/open-apis/im/v1/files");
    assert_eq!(method, "POST");
    assert!(!path.contains("file_type"), "query param leaked: {path}");
    for needle in [
        "name=\"file_type\"",
        "stream",
        "name=\"file_name\"",
        "report.pdf",
        "name=\"file\"",
        "%PDF-1 fake",
    ] {
        assert!(body.contains(needle), "missing {needle} in multipart body");
    }

    // … then the file message references the upload key.
    let req = stub.find("POST", "/open-apis/im/v1/messages?receive_id_type=chat_id");
    let v = StubFeishu::body_json(&req);
    assert_eq!(v["msg_type"], "file");
    assert!(v["content"].as_str().unwrap().contains("fk_1"));
}

#[tokio::test]
async fn send_files_uploads_image_with_image_type_field() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pic.png");
    std::fs::write(&file, b"\x89PNG fake").unwrap();

    adapter
        .send_files("oc_chat", &[(file.as_path(), None)], None)
        .await
        .unwrap();

    let (_, _, body) = stub.find("POST", "/open-apis/im/v1/images");
    for needle in ["name=\"image_type\"", "message", "name=\"image\""] {
        assert!(body.contains(needle), "missing {needle} in multipart body");
    }

    let req = stub.find("POST", "/open-apis/im/v1/messages?receive_id_type=chat_id");
    let v = StubFeishu::body_json(&req);
    assert_eq!(v["msg_type"], "image");
    assert!(v["content"].as_str().unwrap().contains("ik_1"));
}

#[tokio::test]
async fn send_files_rejects_empty_before_upload() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("empty.txt");
    std::fs::write(&file, b"").unwrap();

    let err = adapter
        .send_files("oc_chat", &[(file.as_path(), None)], None)
        .await
        .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("empty.txt"), "names the file: {msg}");
    assert!(msg.contains("empty"), "reason: {msg}");
    // The upload endpoint was never hit.
    assert!(stub
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(_, p, _)| !p.starts_with("/open-apis/im/v1/files")));
}

#[tokio::test]
async fn send_files_one_bad_file_does_not_block_the_rest() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.txt");
    std::fs::write(&empty, b"").unwrap();
    let good = dir.path().join("good.txt");
    std::fs::write(&good, b"content").unwrap();

    let err = adapter
        .send_files(
            "oc_chat",
            &[(empty.as_path(), None), (good.as_path(), None)],
            None,
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("empty.txt"));
    // The good file was still uploaded and messaged.
    stub.find("POST", "/open-apis/im/v1/files");
    stub.find("POST", "/open-apis/im/v1/messages?receive_id_type=chat_id");
}

// ── Receive path: message_type extraction ─────────────────────────

#[test]
fn extract_post_text_includes_title_and_runs() {
    let content = json!({
        "zh_cn": {
            "title": " 周报 ",
            "content": [
                [
                    { "tag": "text", "text": "第一行 " },
                    { "tag": "a", "text": "链接", "href": "http://example.com" }
                ],
                [
                    { "tag": "img", "image_key": "img_x" },
                    { "tag": "text", "text": "第二行" }
                ]
            ]
        }
    });

    assert_eq!(
        super::FeishuAdapter::extract_post_text(&content),
        "周报\n第一行 链接\n第二行"
    );
}

#[test]
fn extract_post_text_falls_back_to_bare_form() {
    // Some API versions deliver post content without a locale wrapper.
    let content = json!({
        "title": "t",
        "content": [[{ "tag": "text", "text": "body" }]]
    });

    assert_eq!(super::FeishuAdapter::extract_post_text(&content), "t\nbody");
}

#[test]
fn extract_post_text_empty_for_unknown_locale() {
    let content = json!({ "ko_kr": { "content": [[{ "tag": "text", "text": "x" }]] } });

    assert_eq!(super::FeishuAdapter::extract_post_text(&content), "");
}

fn receive_event(msg_type: &str, content: &serde_json::Value) -> serde_json::Value {
    json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_1",
                "chat_id": "oc_chat",
                "chat_type": "p2p",
                "message_type": msg_type,
                "create_time": "1700000000000",
                "content": content.to_string(),
            }
        }
    })
}

#[tokio::test]
async fn post_event_is_forwarded_with_extracted_text() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = receive_event(
        "post",
        &json!({
            "zh_cn": {
                "title": "标题",
                "content": [
                    [{ "tag": "text", "text": "第一行" }],
                    [{ "tag": "text", "text": "第二行" }]
                ]
            }
        }),
    );

    let msg_id = adapter.parse_event_json(&event, &tx).await.unwrap();

    assert_eq!(msg_id.as_deref(), Some("om_1"));
    let msg = rx.try_recv().expect("post message forwarded");
    assert!(msg.is_mention, "p2p counts as mention");
    assert_eq!(msg.raw_text.as_deref(), Some("标题\n第一行\n第二行"));
    let crate::types::ContentBlock::Text { text } = &msg.content[0] else {
        panic!("expected text block");
    };
    assert!(text.contains("标题\n第一行\n第二行"), "content: {text}");
}

#[tokio::test]
async fn text_event_still_forwarded() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = receive_event("text", &json!({ "text": "hello" }));

    adapter.parse_event_json(&event, &tx).await.unwrap();

    let msg = rx.try_recv().expect("text message forwarded");
    assert_eq!(msg.raw_text.as_deref(), Some("hello"));
}

#[tokio::test]
async fn non_text_event_without_text_is_ignored() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = receive_event("sticker", &json!({ "file_key": "st_x" }));

    let msg_id = adapter.parse_event_json(&event, &tx).await.unwrap();

    assert_eq!(msg_id, None);
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

// ── Receive path: images ───────────────────────────────────────────

#[test]
fn extract_post_image_keys_collects_img_runs_in_order() {
    let content = json!({
        "zh_cn": {
            "content": [
                [
                    { "tag": "text", "text": "看图" },
                    { "tag": "img", "image_key": "img_a" }
                ],
                [
                    { "tag": "img", "image_key": "img_b" },
                    { "tag": "media", "file_key": "fk_ignored" }
                ]
            ]
        }
    });

    assert_eq!(
        super::FeishuAdapter::extract_post_image_keys(&content),
        vec!["img_a".to_string(), "img_b".to_string()]
    );
}

#[tokio::test]
async fn image_message_forwards_keys_without_downloading() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = receive_event("image", &json!({ "image_key": "img_ok" }));

    adapter.parse_event_json(&event, &tx).await.unwrap();

    let msg = rx.try_recv().expect("image message forwarded");
    assert_eq!(msg.raw_text.as_deref(), Some(""));
    assert_eq!(msg.image_keys, vec!["img_ok".to_string()]);
    // Deferred download: no image block, no resources request (yet).
    assert!(
        msg.content
            .iter()
            .all(|b| !matches!(b, crate::types::ContentBlock::ImageUrl { .. })),
        "content: {:?}",
        msg.content
    );
    assert!(
        stub.requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, p, _)| !p.contains("/resources/")),
        "no download at receive time"
    );
}

#[tokio::test]
async fn pure_image_post_is_forwarded() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = receive_event(
        "post",
        &json!({
            "zh_cn": {
                "content": [[{ "tag": "img", "image_key": "img_ok" }]]
            }
        }),
    );

    adapter.parse_event_json(&event, &tx).await.unwrap();

    let msg = rx.try_recv().expect("pure-image post forwarded");
    assert_eq!(msg.image_keys, vec!["img_ok".to_string()]);
}

#[tokio::test]
async fn download_message_image_returns_image_block() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let block = adapter
        .download_message_image("om_1", "img_ok")
        .await
        .unwrap();

    let crate::types::ContentBlock::ImageUrl { image_url } = block else {
        panic!("expected image block");
    };
    assert!(
        image_url.url.starts_with("data:image/png;base64,"),
        "url: {}",
        image_url.url
    );

    // A bad key surfaces as an error (the hub turns it into a
    // placeholder text block).
    assert!(adapter
        .download_message_image("om_1", "img_bad")
        .await
        .is_err());
}
