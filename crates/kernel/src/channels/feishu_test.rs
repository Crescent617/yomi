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
    // Single message get (quoted-reply injection): `om_quoted` serves a
    // text message, `om_deleted` a deleted one, anything else an empty list.
    if method == "GET" && path.starts_with("/open-apis/im/v1/messages/") {
        let p = path.split('?').next().unwrap_or(path);
        return match p {
            "/open-apis/im/v1/messages/om_quoted" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_quoted","create_time":"1700000000000","msg_type":"text","deleted":false,
                 "sender":{"id":"ou_q","sender_type":"user"},"body":{"content":"{\"text\":\"被引用的内容\"}"}}
            ]}}"#
            .into(),
            "/open-apis/im/v1/messages/om_deleted" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_deleted","create_time":"1700000000000","msg_type":"text","deleted":true,
                 "sender":{"id":"ou_q","sender_type":"user"},"body":{"content":"{\"text\":\"gone\"}"}}
            ]}}"#
            .into(),
            // The get-message API echoes schema 2.0 cards as a legacy-
            // rendered placeholder, never the card JSON that was sent.
            "/open-apis/im/v1/messages/om_new" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_new","create_time":"1700000000000","msg_type":"interactive","deleted":false,
                 "sender":{"id":"cli_bot","sender_type":"app"},
                 "body":{"content":"{\"title\":null,\"elements\":[[{\"tag\":\"text\",\"text\":\"请升级至最新版本客户端，以查看内容\"}]]}"}}
            ]}}"#
            .into(),
            // message_link fixtures: chat-level (plain position), in-thread
            // (negative chat position + thread position), and a thread root
            // (positive chat position AND a thread id).
            "/open-apis/im/v1/messages/om_pos" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_pos","create_time":"1700000000000","msg_type":"interactive","deleted":false,
                 "message_position":"573","sender":{"id":"cli_bot","sender_type":"app"},"body":{"content":"{}"}}
            ]}}"#
            .into(),
            "/open-apis/im/v1/messages/om_threaded" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_threaded","create_time":"1700000000000","msg_type":"interactive","deleted":false,
                 "message_position":"-3","thread_id":"omt_9","thread_message_position":"2",
                 "sender":{"id":"cli_bot","sender_type":"app"},"body":{"content":"{}"}}
            ]}}"#
            .into(),
            "/open-apis/im/v1/messages/om_root" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_root","create_time":"1700000000000","msg_type":"text","deleted":false,
                 "message_position":"574","thread_id":"omt_9","thread_message_position":"-1",
                 "sender":{"id":"ou_q","sender_type":"user"},"body":{"content":"{}"}}
            ]}}"#
            .into(),
            // With `card_msg_content_type=user_card_content` the get-message
            // API echoes the real schema 2.0 body (top-level markdown only;
            // the collapsed panel stays folded) for any sender.
            "/open-apis/im/v1/messages/om_card" => r#"{"code":0,"msg":"ok","data":{"items":[
                {"message_id":"om_card","create_time":"1700000000000","msg_type":"interactive","deleted":false,
                 "sender":{"id":"ou_user","sender_type":"user"},
                 "body":{"content":"{\"schema\":\"2.0\",\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"卡片里的问题：怎么配 daemon？\"},{\"tag\":\"collapsible_panel\",\"elements\":[{\"tag\":\"markdown\",\"content\":\"折叠的轨迹\"}]}]}}"}}
            ]}}"#
            .into(),
            _ => r#"{"code":0,"msg":"ok","data":{"items":[]}}"#.into(),
        };
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
                {"message_id":"m2c","create_time":"1700000055000","msg_type":"interactive","deleted":false,
                 "sender":{"id":"ou_c","sender_type":"user"},
                 "body":{"content":"{\"schema\":\"2.0\",\"body\":{\"elements\":[{\"tag\":\"markdown\",\"content\":\"卡片正文\"},{\"tag\":\"collapsible_panel\",\"expanded\":false,\"header\":{\"title\":{\"tag\":\"markdown\",\"content\":\"折叠\"}},\"elements\":[{\"tag\":\"markdown\",\"content\":\"折叠噪音\"}]}]}}"}},
                {"message_id":"m2","create_time":"1700000050000","msg_type":"post","deleted":false,
                 "sender":{"id":"ou_b","sender_type":"user"},
                 "body":{"content":"{\"zh_cn\":{\"content\":[[{\"tag\":\"text\",\"text\":\"第一段 \"},{\"tag\":\"text\",\"text\":\"第二段\"}],[{\"tag\":\"img\",\"image_key\":\"img_p1\"}]]}}"}},
                {"message_id":"m2b","create_time":"1700000046000","msg_type":"text","deleted":false,
                 "sender":{"id":"cli_other","sender_type":"app"},"body":{"content":"{\"text\":\"CI 构建成功\"}"}},
                {"message_id":"m1","create_time":"1700000040000","msg_type":"text","deleted":false,
                 "sender":{"id":"app","sender_type":"app"},"body":{"content":"{\"text\":\"bot said\"}"}},
                {"message_id":"m0","create_time":"1700000010000","msg_type":"text","deleted":false,
                 "sender":{"id":"ou_a","sender_type":"user"},"body":{"content":"{\"text\":\"too old\"}"}}
            ]}}"#
            .into()
        }
        "PATCH" if p.starts_with("/open-apis/im/v1/messages/") => {
            r#"{"code":0,"msg":"ok"}"#.into()
        }
        "POST" if p.starts_with("/open-apis/drive/v1/permissions/") => {
            r#"{"code":0,"msg":"ok","data":{}}"#.into()
        }
        // Doc comment APIs: the replies LIST serves the (fresh) timeline;
        // batch_query serves quote/is_whole (its own reply_list lags and
        // is intentionally not parsed).
        "GET" if p.starts_with("/open-apis/drive/v1/files/") && p.ends_with("/replies") => {
            r#"{"code":0,"msg":"ok","data":{"has_more":false,"page_token":"","items":[
                {"reply_id":"r_1","user_id":"ou_commenter","create_time":1700000000,
                 "content":{"elements":[
                    {"type":"text_run","text_run":{"text":"这段"}},
                    {"type":"person","person":{"user_id":"ou_bot"}},
                    {"type":"text_run","text_run":{"text":" 改写一下，参考 "}},
                    {"type":"docs_link","docs_link":{"url":"https://feishu.cn/docx/other"}}
                 ]}},
                {"reply_id":"r_2","user_id":"ou_bot","create_time":1700000006,
                 "content":{"elements":[{"type":"text_run","text_run":{"text":"bot 的回复"}}]}}
            ]}}"#.into()
        }
        "POST" if p.starts_with("/open-apis/drive/v1/files/") && p.ends_with("/comments/batch_query") => {
            r#"{"code":0,"msg":"ok","data":{"items":[{
                "comment_id":"c_1","user_id":"ou_commenter","create_time":1700000000,"is_whole":false,
                "quote":"引用原文段落","reply_list":{"replies":[]}
            }]}}"#.into()
        }
        "POST" if p.starts_with("/open-apis/drive/v1/files/") && p.ends_with("/replies") => {
            if p.contains("/comments/c_whole/") {
                r#"{"code":1069302,"msg":"whole comment can not add reply"}"#.into()
            } else {
                r#"{"code":0,"msg":"ok","data":{"reply_id":"r_new"}}"#.into()
            }
        }
        "POST" if p.starts_with("/open-apis/drive/v1/files/") && p.ends_with("/comments") => {
            r#"{"code":0,"msg":"ok","data":{"comment_id":"c_new"}}"#.into()
        }
        "POST" if p.starts_with("/open-apis/drive/v2/files/") && p.ends_with("/comments/reaction") => {
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
async fn delete_reaction_deletes_by_message_and_reaction_id() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    adapter.delete_reaction("", "om_1", "rid_1").await.unwrap();

    let req = stub.find("DELETE", "/open-apis/im/v1/messages/om_1/reactions/rid_1");
    assert_eq!(req.0, "DELETE");
    assert_eq!(req.1, "/open-apis/im/v1/messages/om_1/reactions/rid_1");
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

    // m0 dropped (older than cursor), m1 dropped (our own app — sender id
    // == app_id); other apps and users kept; result is chronological.
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].message_id, "m2b");
    assert_eq!(out[0].text, "CI 构建成功");
    assert_eq!(out[1].message_id, "m2");
    assert_eq!(out[1].text, "第一段 第二段");
    assert_eq!(out[1].image_keys, vec!["img_p1".to_string()]);
    assert_eq!(out[1].create_time, 1_700_000_050_000);
    // Card history keeps its top-level markdown; the collapsed panel is
    // excluded from context.
    assert_eq!(out[2].message_id, "m2c");
    assert_eq!(out[2].text, "卡片正文");
    assert!(
        !out[2].text.contains("折叠噪音"),
        "collapsed panel excluded"
    );
    assert_eq!(out[3].message_id, "m3");
    assert_eq!(out[3].text, "最新");
    assert!(out[3].image_keys.is_empty());
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
    assert!(
        path.contains("card_msg_content_type=user_card_content"),
        "card bodies echoed: {path}"
    );
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

#[tokio::test]
async fn fetch_message_returns_quoted_content() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let m = adapter
        .fetch_message("om_quoted")
        .await
        .unwrap()
        .expect("message found");
    assert_eq!(m.message_id, "om_quoted");
    assert_eq!(m.text, "被引用的内容");
    assert_eq!(m.sender_id, "ou_q");
    assert_eq!(m.create_time, 1_700_000_000_000);

    // Deleted and missing messages yield None, not an error.
    assert!(adapter.fetch_message("om_deleted").await.unwrap().is_none());
    assert!(adapter.fetch_message("om_missing").await.unwrap().is_none());
}

#[tokio::test]
async fn sent_card_text_is_cached_and_served_on_fetch() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let card = r#"{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"答案正文"}]}}"#;

    // The API echoes only a legacy placeholder for our card; the cached
    // sent text must win.
    adapter.send_card("oc_chat", card, None).await.unwrap();
    let m = adapter
        .fetch_message("om_new")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(m.text, "答案正文");

    // A card morph (status → reply) refreshes the cached text.
    let morphed =
        r#"{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"morph 后的正文"}]}}"#;
    adapter.update_card("om_new", morphed).await.unwrap();
    let m = adapter
        .fetch_message("om_new")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(m.text, "morph 后的正文");
}

/// Restart simulation: process A sends the card (memory + kv db), process
/// B (fresh adapter, cold memory, same db) must still serve the text.
#[tokio::test]
async fn sent_card_text_survives_restart_via_kv() {
    let stub = StubFeishu::start().await;
    let dir = tempfile::tempdir().unwrap();
    let kv_path = dir.path().join("cache.db");
    let card =
        r#"{"schema":"2.0","body":{"elements":[{"tag":"markdown","content":"重启前的答案"}]}}"#;

    let mut adapter_a = stub_adapter(&stub.base_url);
    adapter_a.set_kv_cache(Some(std::sync::Arc::new(
        crate::kv_cache::KvCache::open(&kv_path).await.unwrap(),
    )));
    adapter_a.send_card("oc_chat", card, None).await.unwrap();

    let mut adapter_b = stub_adapter(&stub.base_url);
    adapter_b.set_kv_cache(Some(std::sync::Arc::new(
        crate::kv_cache::KvCache::open(&kv_path).await.unwrap(),
    )));
    let m = adapter_b
        .fetch_message("om_new")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(m.text, "重启前的答案");
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
fn extract_history_content_reads_card_markdown() {
    // Schema 2.0 card: markdown elements concatenated, panels skipped.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"schema":"2.0","body":{"elements":[
            {"tag":"markdown","content":"答案正文"},
            {"tag":"collapsible_panel","elements":[{"tag":"markdown","content":"轨迹噪声"}]},
            {"tag":"markdown","content":"第二段"}
        ]}}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "答案正文\n第二段");

    // Legacy v1 shape (top-level elements).
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"elements":[{"tag":"markdown","content":"旧卡"}]}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "旧卡");

    // No markdown elements → placeholder fallback.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"schema":"2.0","body":{"elements":[{"tag":"div"}]}}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "[interactive]");

    // get-message API echo of a v1 card: legacy-rendered runs keep the
    // real text.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"title":null,"elements":[[{"tag":"text","text":"真实文本"}]]}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "真实文本");

    // get-message API echo of a schema 2.0 card: the upgrade notice must
    // NOT leak — degrade to the placeholder instead.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"title":null,"elements":[[{"tag":"img","image_key":"img_v3_x"},{"tag":"text","text":"请升级至最新版本客户端，以查看内容"},{"tag":"text","text":""}]]}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "[interactive]");

    // The notice riding alongside real content: only the notice run is
    // filtered, the real text survives.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"title":null,"elements":[[{"tag":"text","text":"真实内容"}],[{"tag":"text","text":"请升级至最新版本客户端，以查看内容"}]]}"# }
    });
    let (text, _) = super::FeishuAdapter::extract_history_content(&item);
    assert_eq!(text, "真实内容");
}

/// Card `<at id=...>` mention tags are rewritten to the neutral `<@id>name`
/// contract on extraction — quoted or bare ids, with or without a surviving
/// display name; fenced/inline code is left literal.
#[test]
fn extract_card_text_rewrites_at_tags() {
    let card = |content: &str| {
        json!({
            "msg_type": "interactive",
            "body": { "content": serde_json::to_string(&json!({
                "schema": "2.0",
                "body": { "elements": [{ "tag": "markdown", "content": content }] }
            })).unwrap() }
        })
    };
    let extract = |content: &str| super::FeishuAdapter::extract_history_content(&card(content)).0;

    let cases = [
        // bare id, empty name (the get-message echo drops the name)
        ("<at id=ou_bot></at> 看一下", "<@ou_bot> 看一下"),
        // quoted id
        (r#"<at id="ou_bot"></at> 看一下"#, "<@ou_bot> 看一下"),
        // name survives
        ("<at id=ou_a>小明</at> 好", "<@ou_a>小明 好"),
        (r#"<at id="ou_a">小明</at> 好"#, "<@ou_a>小明 好"),
        // multiple mentions
        (
            "<at id=ou_a></at> 和 <at id=ou_b></at>",
            "<@ou_a> 和 <@ou_b>",
        ),
        // not a mention tag (no id) — untouched
        ("<at> xxx", "<at> xxx"),
    ];
    for (input, want) in cases {
        assert_eq!(extract(input), want, "input: {input}");
    }

    // Inline code and fenced blocks stay literal.
    assert_eq!(
        extract(
            "语法是 `<at id=ou_a></at>`，但 <at id=ou_b></at> 是真的\n```\n<at id=ou_c></at>\n```"
        ),
        "语法是 `<at id=ou_a></at>`，但 <@ou_b> 是真的\n```\n<at id=ou_c></at>\n```"
    );
}

/// Another yomi bot's terminal card: the run trace lives in a
/// `collapsible_panel`, so a reading yomi bot must see the answer body but
/// never the trace. This is the cross-bot privacy contract — traces stay
/// invisible between yomi instances on every read path (all funnel into
/// `extract_card_text`).
#[test]
fn extract_history_content_hides_yomi_trace_panel() {
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"schema":"2.0","body":{"elements":[
            {"tag":"markdown","content":"✅ **Done** — ⏱ 6s · 💬 1"},
            {"tag":"collapsible_panel","expanded":false,
             "header":{"title":{"tag":"markdown","content":"<font color='grey'>🐾 Trace · 💬 1 · 🔧 1</font>"}},
             "elements":[{"tag":"markdown","text_size":"notation","content":"🔧 shell: cargo test\n✅ done"}]}
        ]}}"# }
    });

    let (text, _) = super::FeishuAdapter::extract_history_content(&item);

    assert_eq!(text, "✅ **Done** — ⏱ 6s · 💬 1");
    for leaked in ["Trace", "shell", "cargo test", "🐾"] {
        assert!(
            !text.contains(leaked),
            "trace leaked into context: {leaked} in {text:?}"
        );
    }
}

/// The **live mid-run** card: everything streaming — whisper tail, trace,
/// stats — rides inside a collapsible panel that starts *expanded* (so the
/// human watches it), and a reading bot strips the whole panel all the
/// same (`expanded` makes no difference to extraction). What survives is
/// the card **header title** — the fun phase line ("🐾 Typing…") is the
/// one deliberate transient signal other bots get to see.
#[test]
fn extract_history_content_hides_midrun_trace_panel() {
    // Mirrors render_running: card header carries the phase title; the
    // body is a single expanded collapsible_panel whose title rides the
    // stats line and whose body leads with the whisper, then the trace.
    let item = json!({
        "msg_type": "interactive",
        "body": { "content": r#"{"schema":"2.0",
            "header":{"title":{"tag":"plain_text","content":"🐾 Typing…"},"template":"blue"},
            "body":{"elements":[
            {"tag":"collapsible_panel","expanded":true,
             "header":{"title":{"tag":"markdown","content":"<font color='grey'>🐾 5s · k3-hs · out ~241</font>"}},
             "elements":[{"tag":"markdown","text_size":"notation","content":"<font color='grey'>💬 写作中…</font>\n✅ **web_fetch** · wttr.in · 1s"}]}
        ]}}"# }
    });

    let (text, _) = super::FeishuAdapter::extract_history_content(&item);

    assert_eq!(text, "🐾 Typing…");
    for leaked in [
        "💬",
        "写作中",
        "🐾 5s",
        "k3-hs",
        "out ~241",
        "web_fetch",
        "wttr.in",
    ] {
        assert!(
            !text.contains(leaked),
            "mid-run transient leaked: {leaked} in {text:?}"
        );
    }
}

/// The card header title is real content and is extracted as such: alone
/// when the body strips to nothing, prepended when both exist.
#[test]
fn extract_card_text_includes_header_title() {
    let card = |header: &str, body: &str| {
        json!({
            "msg_type": "interactive",
            "body": { "content": format!(
                r#"{{"schema":"2.0",{header}"body":{{"elements":[{body}]}}}}"#
            ) }
        })
    };
    let extract = |item: &serde_json::Value| super::FeishuAdapter::extract_history_content(item).0;

    // Header + body → title line first.
    let both = card(
        r#""header":{"title":{"tag":"plain_text","content":"📦 Compacting context…"},"template":"blue"},"#,
        r#"{"tag":"markdown","content":"正文"}"#,
    );
    assert_eq!(extract(&both), "📦 Compacting context…\n正文");

    // Header only (body panel stripped) → title alone.
    let title_only = card(
        r#""header":{"title":{"tag":"plain_text","content":"🐹 Shell…"},"template":"blue"},"#,
        r#"{"tag":"collapsible_panel","expanded":true,"elements":[{"tag":"markdown","content":"🔧 ls"}]}"#,
    );
    assert_eq!(extract(&title_only), "🐹 Shell…");

    // No header → body unchanged.
    let body_only = card("", r#"{"tag":"markdown","content":"正文"}"#);
    assert_eq!(extract(&body_only), "正文");
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

/// Unwrap a received [`ChannelEvent`] into its chat message — message-path
/// tests never expect platform events.
fn expect_message(event: crate::channels::ChannelEvent) -> crate::channels::ChannelMessage {
    let crate::channels::ChannelEvent::Message(msg) = event else {
        panic!("expected ChannelEvent::Message, got {event:?}");
    };
    msg
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
    let msg = expect_message(rx.try_recv().expect("post message forwarded"));
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

    let msg = expect_message(rx.try_recv().expect("text message forwarded"));
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

/// An `im.message.receive_v1` event for an interactive (card) message.
/// `chat_type` + `mentions` drive the mention gate; the body itself is only
/// the legacy placeholder — the real text is fetched from `om_card`.
fn card_event(chat_type: &str, mentions: serde_json::Value) -> serde_json::Value {
    json!({
        "header": { "event_type": "im.message.receive_v1" },
        "event": {
            "sender": { "sender_id": { "open_id": "ou_user" } },
            "message": {
                "message_id": "om_card",
                "chat_id": "oc_chat",
                "chat_type": chat_type,
                "message_type": "interactive",
                "create_time": "1700000000000",
                "mentions": mentions,
                "content": "{\"title\":null,\"elements\":[[{\"tag\":\"text\",\"text\":\"请升级至最新版本客户端，以查看内容\"}]]}",
            }
        }
    })
}

#[tokio::test]
async fn p2p_card_event_fetches_body_and_forwards() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    let msg_id = adapter
        .parse_event_json(&card_event("p2p", json!([])), &tx)
        .await
        .unwrap();

    assert_eq!(msg_id.as_deref(), Some("om_card"));
    let msg = expect_message(rx.try_recv().expect("card message forwarded"));
    // Real body fetched, collapsed panel excluded.
    let text = match &msg.content[0] {
        crate::types::ContentBlock::Text { text } => text.clone(),
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(
        text.contains("卡片里的问题：怎么配 daemon？"),
        "text: {text}"
    );
    assert!(
        !text.contains("折叠的轨迹"),
        "collapsed must be excluded: {text}"
    );
    assert!(msg.is_mention, "p2p is always a mention");
}

#[tokio::test]
async fn group_card_with_bot_mention_forwards() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    *adapter.bot_open_id.lock().await = Some("ou_bot".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mentions = json!([{ "key": "@_user_1", "id": { "open_id": "ou_bot" } }]);

    let msg_id = adapter
        .parse_event_json(&card_event("group", mentions), &tx)
        .await
        .unwrap();

    assert_eq!(msg_id.as_deref(), Some("om_card"));
    let msg = expect_message(rx.try_recv().expect("mentioned card forwarded"));
    assert!(msg.is_mention);
}

#[tokio::test]
async fn group_card_without_bot_mention_is_ignored() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    *adapter.bot_open_id.lock().await = Some("ou_bot".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    // A mention of someone else — not the bot.
    let mentions = json!([{ "key": "@_user_1", "id": { "open_id": "ou_other" } }]);

    let msg_id = adapter
        .parse_event_json(&card_event("group", mentions), &tx)
        .await
        .unwrap();

    assert_eq!(msg_id, None);
    assert!(rx.try_recv().is_err(), "un-mentioned group card ignored");
    // The mention gate runs before the fetch: no get-message request made.
    assert!(
        stub.requests
            .lock()
            .unwrap()
            .iter()
            .all(|(_, p, _)| !p.contains("/messages/om_card")),
        "no fetch for un-mentioned card"
    );
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

    let msg = expect_message(rx.try_recv().expect("image message forwarded"));
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

    let msg = expect_message(rx.try_recv().expect("pure-image post forwarded"));
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

// ── Receive path: doc permission events ────────────────────────────

#[tokio::test]
async fn doc_permission_event_is_forwarded() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = json!({
        "header": { "event_type": "drive.file.permission_member_applied_v1" },
        "event": {
            "file_type": "docx",
            "file_token": "doxcnXXXX",
            "permission": "view",
            "application_remark": "求权限看下方案",
            "application_user_list": [{ "open_id": "ou_aaa" }],
            "application_chat_list": ["oc_bbb"],
            "application_department_list": ["od_ccc"]
        }
    });

    adapter.parse_event_json(&event, &tx).await.unwrap();

    let crate::channels::ChannelEvent::DocPermissionApplied(req) =
        rx.try_recv().expect("doc permission event forwarded")
    else {
        panic!("expected DocPermissionApplied");
    };
    assert_eq!(req.file_token, "doxcnXXXX");
    assert_eq!(req.file_type, "docx");
    assert_eq!(req.permission, "view");
    assert_eq!(req.remark.as_deref(), Some("求权限看下方案"));
    assert_eq!(req.applicant_users, vec!["ou_aaa".to_string()]);
    assert_eq!(req.applicant_chats, vec!["oc_bbb".to_string()]);
    assert_eq!(req.applicant_departments, vec!["od_ccc".to_string()]);
}

#[tokio::test]
async fn doc_permission_event_without_file_token_is_ignored() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = json!({
        "header": { "event_type": "drive.file.permission_member_applied_v1" },
        "event": { "file_type": "docx", "permission": "view" }
    });

    adapter.parse_event_json(&event, &tx).await.unwrap();
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

#[tokio::test]
async fn unknown_event_type_is_ignored() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = json!({
        "header": { "event_type": "im.chat.updated_v1" },
        "event": {}
    });

    adapter.parse_event_json(&event, &tx).await.unwrap();
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

// ── Doc comment events & APIs ──────────────────────────────────────

fn doc_comment_event(is_mentioned: bool, commenter: &str) -> serde_json::Value {
    json!({
        "header": { "event_type": "drive.notice.comment_add_v1", "create_time": "1700000000123" },
        "event": {
            "notice_meta": {
                "file_type": "docx",
                "file_token": "doxcnABC123",
                "from_user_id": { "open_id": commenter },
                "to_user_id": { "open_id": "ou_bot" },
                "notice_type": "add_comment"
            },
            "comment_id": "7123456789",
            "reply_id": "r_2",
            "is_mentioned": is_mentioned
        }
    })
}

#[tokio::test]
async fn doc_comment_event_is_forwarded() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    adapter
        .parse_event_json(&doc_comment_event(true, "ou_commenter"), &tx)
        .await
        .unwrap();

    let crate::channels::ChannelEvent::DocCommentAdded(notice) =
        rx.try_recv().expect("doc comment event forwarded")
    else {
        panic!("expected DocCommentAdded");
    };
    assert_eq!(notice.file_token, "doxcnABC123");
    assert_eq!(notice.file_type, "docx");
    assert_eq!(notice.comment_id, "7123456789");
    assert_eq!(notice.reply_id.as_deref(), Some("r_2"));
    assert_eq!(notice.commenter_open_id, "ou_commenter");
    assert!(notice.is_mentioned);
    assert_eq!(notice.notice_type, "add_comment");
    assert_eq!(notice.create_time, Some(1_700_000_000_123));
}

#[tokio::test]
async fn doc_comment_event_redelivery_is_deduplicated() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let event = doc_comment_event(true, "ou_commenter");

    adapter.parse_event_json(&event, &tx).await.unwrap();
    adapter.parse_event_json(&event, &tx).await.unwrap();

    assert!(rx.try_recv().is_ok(), "first delivery forwarded");
    assert!(rx.try_recv().is_err(), "redelivery deduplicated");
}

#[tokio::test]
async fn doc_comment_event_from_bot_itself_is_skipped() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    // Seed the bot identity (normally fetched from the API, stubbed here)
    // so the self-authored check can match.
    *adapter.bot_open_id.lock().await = Some("ou_bot".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);

    adapter
        .parse_event_json(&doc_comment_event(true, "ou_bot"), &tx)
        .await
        .unwrap();

    assert!(
        rx.try_recv().is_err(),
        "self-authored event must be skipped"
    );
}

#[tokio::test]
async fn doc_comment_event_missing_ids_is_ignored() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = json!({
        "header": { "event_type": "drive.notice.comment_add_v1" },
        "event": { "notice_meta": { "file_type": "docx" }, "is_mentioned": true }
    });

    adapter.parse_event_json(&event, &tx).await.unwrap();
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

#[tokio::test]
async fn fetch_doc_comment_parses_quote_and_replies() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    // Seed the bot identity so reply attribution (is_from_bot) resolves.
    *adapter.bot_open_id.lock().await = Some("ou_bot".to_string());

    let detail = adapter
        .fetch_doc_comment("doxcnABC123", "docx", "c_1")
        .await
        .unwrap()
        .expect("comment found");

    assert_eq!(detail.is_whole, Some(false));
    assert_eq!(detail.quote.as_deref(), Some("引用原文段落"));
    assert_eq!(detail.replies.len(), 2);
    assert_eq!(detail.replies[0].reply_id, "r_1");
    assert!(!detail.replies[0].is_from_bot);
    assert!(detail.replies[1].is_from_bot);
    // person element renders as @bot (bot id seeded), docs_link as its URL.
    assert_eq!(
        detail.replies[0].text,
        "这段@bot 改写一下，参考 https://feishu.cn/docx/other"
    );
    assert_eq!(detail.replies[1].text, "bot 的回复");

    let req = stub.find(
        "POST",
        "/open-apis/drive/v1/files/doxcnABC123/comments/batch_query",
    );
    assert_eq!(req.0, "POST");
    assert!(req.1.contains("file_type=docx"), "{}", req.1);
    assert_eq!(StubFeishu::body_json(&req)["comment_ids"][0], "c_1");
}

#[tokio::test]
async fn react_doc_comment_posts_add_reaction() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    adapter
        .react_doc_comment("doxcnABC123", "docx", "r_2", "OneSecond")
        .await
        .unwrap();

    let req = stub.find(
        "POST",
        "/open-apis/drive/v2/files/doxcnABC123/comments/reaction",
    );
    assert!(req.1.contains("file_type=docx"), "{}", req.1);
    let body = StubFeishu::body_json(&req);
    assert_eq!(body["action"], "add");
    assert_eq!(body["reply_id"], "r_2");
    assert_eq!(body["reaction_type"], "OneSecond");
}

#[test]
fn extract_reply_text_renders_bot_mention_as_at_bot() {
    let elements = vec![
        json!({ "type": "text_run", "text_run": { "text": "hey " } }),
        json!({ "type": "person", "person": { "user_id": "ou_bot" } }),
        json!({ "type": "person", "person": { "user_id": "ou_alice" } }),
    ];
    let text = super::FeishuAdapter::extract_reply_text(Some(&elements), Some("ou_bot"));
    assert_eq!(text, "hey @bot@user:ou_alice");
}

#[tokio::test]
async fn reply_doc_comment_posts_text_run() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let id = adapter
        .reply_doc_comment("doxcnABC123", "docx", "c_1", "回复内容")
        .await
        .unwrap();

    assert_eq!(id.as_deref(), Some("r_new"));
    let req = stub.find(
        "POST",
        "/open-apis/drive/v1/files/doxcnABC123/comments/c_1/replies",
    );
    assert_eq!(
        StubFeishu::body_json(&req)["content"]["elements"][0]["text_run"]["text"],
        "回复内容"
    );
}

#[tokio::test]
async fn reply_doc_comment_falls_back_to_new_comment_for_whole_comment() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let id = adapter
        .reply_doc_comment("doxcnABC123", "docx", "c_whole", "回复内容")
        .await
        .unwrap();

    assert_eq!(id.as_deref(), Some("c_new"));
    // The fallback posted a new whole comment after the 1069302 error
    // (the captured path carries its query — "…/comments?…" is the
    // fallback, "…/comments/c_whole/replies?…" the failed first attempt).
    let req = stub.find("POST", "/open-apis/drive/v1/files/doxcnABC123/comments?");
    assert_eq!(
        StubFeishu::body_json(&req)["reply_list"]["replies"][0]["content"]["elements"][0]
            ["text_run"]["text"],
        "回复内容"
    );
}

#[tokio::test]
async fn reply_doc_comment_with_whole_sentinel_creates_new_comment_directly() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let id = adapter
        .reply_doc_comment(
            "doxcnABC123",
            "docx",
            super::super::WHOLE_COMMENT_ID,
            "回复内容",
        )
        .await
        .unwrap();

    assert_eq!(id.as_deref(), Some("c_new"));
    let requests = stub.requests.lock().unwrap();
    // The doomed thread-reply attempt is skipped for the sentinel.
    assert!(
        requests
            .iter()
            .all(|(_, p, _)| !p.contains("/comments/whole/replies")),
        "no reply attempt against the sentinel: {requests:?}"
    );
    let req = requests
        .iter()
        .find(|(m, p, _)| {
            m == "POST" && p.starts_with("/open-apis/drive/v1/files/doxcnABC123/comments?")
        })
        .expect("create-comment called");
    assert_eq!(
        StubFeishu::body_json(req)["reply_list"]["replies"][0]["content"]["elements"][0]
            ["text_run"]["text"],
        "回复内容"
    );
}

// ── Doc permission grant & DM cards & card actions ─────────────────

#[tokio::test]
async fn grant_doc_permission_batches_all_applicant_kinds() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let req = crate::channels::DocPermissionRequest {
        file_token: "doxcnABC".to_string(),
        file_type: "docx".to_string(),
        permission: "view".to_string(),
        remark: None,
        applicant_users: vec!["ou_a".to_string()],
        applicant_chats: vec!["oc_c".to_string()],
        applicant_departments: vec!["od_d".to_string()],
    };

    adapter
        .grant_doc_permission("doxcnABC", "docx", &req, "edit")
        .await
        .unwrap();

    let requests = stub.requests.lock().unwrap();
    let (_, path, body) = requests
        .iter()
        .find(|(_, p, _)| p.contains("/members/batch_create"))
        .expect("batch_create called");
    assert!(path.contains("type=docx"), "{path}");
    assert!(path.contains("need_notification=true"), "{path}");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    let members = body["members"].as_array().unwrap();
    assert_eq!(
        members.as_slice(),
        &[
            json!({ "member_type": "openid", "member_id": "ou_a", "perm": "edit", "type": "user" }),
            json!({ "member_type": "openchat", "member_id": "oc_c", "perm": "edit", "type": "chat" }),
            json!({ "member_type": "opendepartmentid", "member_id": "od_d", "perm": "edit", "type": "department" }),
        ]
    );
}

#[tokio::test]
async fn grant_doc_permission_without_applicants_fails_fast() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let req = crate::channels::DocPermissionRequest {
        file_token: "doxcnABC".to_string(),
        file_type: "docx".to_string(),
        permission: "view".to_string(),
        remark: None,
        applicant_users: vec![],
        applicant_chats: vec![],
        applicant_departments: vec![],
    };

    let err = adapter
        .grant_doc_permission("doxcnABC", "docx", &req, "view")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no applicants"), "{err}");
}

#[tokio::test]
async fn direct_card_sent_to_open_id() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let mid = adapter
        .send_direct_card("ou_admin", r#"{"schema":"2.0"}"#)
        .await
        .unwrap();
    assert_eq!(mid.as_deref(), Some("om_new"));
    let requests = stub.requests.lock().unwrap();
    let (_, path, body) = requests
        .iter()
        .find(|(_, p, _)| p.starts_with("/open-apis/im/v1/messages?"))
        .expect("message sent");
    assert!(path.contains("receive_id_type=open_id"), "{path}");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["receive_id"], "ou_admin");
    assert_eq!(body["msg_type"], "interactive");
}

/// `message_link`: chat messages link by chat position; in-thread
/// messages (negative chat position) link by thread position; a thread
/// root (positive chat position despite having a thread id) keeps the
/// chat link.
#[tokio::test]
async fn message_link_builds_applink() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let link = adapter.message_link("oc_1", "om_pos").await.unwrap();
    assert_eq!(
        link,
        "https://applink.feishu.cn/client/chat/open?openChatId=oc_1&position=573"
    );

    let link = adapter.message_link("oc_1", "om_threaded").await.unwrap();
    assert_eq!(
        link,
        "https://applink.feishu.cn/client/thread/open?open_chat_id=oc_1&open_thread_id=omt_9&openchatid=oc_1&openthreadid=omt_9&thread_position=2"
    );

    let link = adapter.message_link("oc_1", "om_root").await.unwrap();
    assert_eq!(
        link,
        "https://applink.feishu.cn/client/chat/open?openChatId=oc_1&position=574"
    );

    assert!(adapter.message_link("oc_1", "om_unknown").await.is_none());
}

/// `thread_link`: any message in a thread yields the position-less
/// thread applink; threadless messages and unknown ids yield `None`.
#[tokio::test]
async fn thread_link_builds_position_less_applink() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let link = adapter.thread_link("oc_1", "om_threaded").await.unwrap();
    assert_eq!(
        link,
        "https://applink.feishu.cn/client/thread/open?open_chat_id=oc_1&open_thread_id=omt_9&openchatid=oc_1&openthreadid=omt_9"
    );

    assert!(adapter.thread_link("oc_1", "om_pos").await.is_none());
    assert!(adapter.thread_link("oc_1", "om_unknown").await.is_none());
}

/// `thread_root_id`: the earliest thread message is the root; successful
/// lookups are cached (no second fetch for the same thread).
#[tokio::test]
async fn thread_root_id_fetches_first_message_and_caches() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);

    let root = adapter.thread_root_id("omt_x").await.unwrap();
    assert_eq!(root.as_deref(), Some("m3"));
    let root2 = adapter.thread_root_id("omt_x").await.unwrap();
    assert_eq!(root2.as_deref(), Some("m3"));

    let fetches = stub
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, p, _)| m == "GET" && p.starts_with("/open-apis/im/v1/messages?"))
        .count();
    assert_eq!(fetches, 1, "the second call must hit the cache");
}

#[tokio::test]
async fn card_action_payload_is_forwarded() {
    // Real card.action.trigger callbacks arrive as a v2 envelope.
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let payload = json!({
        "schema": "2.0",
        "header": { "event_type": "card.action.trigger" },
        "event": {
            "operator": { "open_id": "ou_admin" },
            "action": { "value": { "action": "approve", "id": 3 } },
            "context": { "open_chat_id": "oc_chat" }
        }
    });

    super::FeishuAdapter::forward_card_action(&payload, &tx)
        .await
        .unwrap();

    let crate::channels::ChannelEvent::CardAction(action) =
        rx.try_recv().expect("card action forwarded")
    else {
        panic!("expected CardAction");
    };
    assert_eq!(action.operator_open_id, "ou_admin");
    assert_eq!(action.chat_id.as_deref(), Some("oc_chat"));
    assert_eq!(action.value, json!({ "action": "approve", "id": 3 }));
}

#[tokio::test]
async fn card_action_bare_body_is_tolerated() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let payload = json!({
        "operator": { "open_id": "ou_admin" },
        "action": { "value": { "action": "deny", "id": 5 } }
    });

    super::FeishuAdapter::forward_card_action(&payload, &tx)
        .await
        .unwrap();

    let crate::channels::ChannelEvent::CardAction(action) =
        rx.try_recv().expect("card action forwarded")
    else {
        panic!("expected CardAction");
    };
    assert_eq!(action.operator_open_id, "ou_admin");
    assert_eq!(action.value, json!({ "action": "deny", "id": 5 }));
}

#[tokio::test]
async fn card_action_trigger_event_frame_is_forwarded() {
    // Tolerated delivery shape: the callback riding a plain event frame.
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let event = json!({
        "schema": "2.0",
        "header": { "event_type": "card.action.trigger" },
        "event": {
            "operator": { "open_id": "ou_admin" },
            "action": { "value": { "action": "approve", "id": 3 } }
        }
    });

    adapter.parse_event_json(&event, &tx).await.unwrap();

    let crate::channels::ChannelEvent::CardAction(action) =
        rx.try_recv().expect("card action forwarded")
    else {
        panic!("expected CardAction");
    };
    assert_eq!(action.operator_open_id, "ou_admin");
    assert_eq!(action.value, json!({ "action": "approve", "id": 3 }));
}

#[tokio::test]
async fn card_action_without_operator_is_ignored() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let payload = json!({
        "schema": "2.0",
        "header": { "event_type": "card.action.trigger" },
        "event": { "action": { "value": { "action": "approve", "id": 3 } } }
    });

    super::FeishuAdapter::forward_card_action(&payload, &tx)
        .await
        .unwrap();
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

// ── Receive path: redelivery dedup ─────────────────────────────────

#[tokio::test]
async fn redelivered_message_is_deduplicated() {
    let stub = StubFeishu::start().await;
    let adapter = stub_adapter(&stub.base_url);
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let event = receive_event("text", &json!({ "text": "hello" }));

    let first = adapter.parse_event_json(&event, &tx).await.unwrap();
    assert_eq!(first.as_deref(), Some("om_1"));
    let msg = expect_message(rx.try_recv().expect("first delivery forwarded"));
    assert_eq!(msg.raw_text.as_deref(), Some("hello"));

    // A redelivery (lost ACK, reconnect replay) is dropped instead of
    // triggering the agent a second time.
    let dup = adapter.parse_event_json(&event, &tx).await.unwrap();
    assert_eq!(dup, None);
    assert!(rx.try_recv().is_err(), "redelivery must not be forwarded");
}

#[test]
fn dedup_cache_is_bounded() {
    let mut cache = lru::LruCache::<String, ()>::new(super::DEDUP_CAP);
    for i in 0..super::DEDUP_CAP.get() {
        cache.put(format!("om_{i}"), ());
    }
    cache.put("om_extra".to_string(), ());
    assert_eq!(cache.len(), super::DEDUP_CAP.get());
    assert!(!cache.contains("om_0"), "oldest evicted at capacity");
    assert!(cache.contains("om_extra"));
}

// ── e2e against the real Feishu ws gateway ─────────────────────────
//
// Run manually: FEISHU_E2E_APP_ID=cli_xxx FEISHU_E2E_APP_SECRET=xxx \
//   cargo test -p kernel channels::feishu::tests::e2e -- --ignored --nocapture
//
// Verifies the assumption behind FRAME_TIMEOUT: every app-level ping gets
// a pong. Data frames are NOT acked (they redeliver to a running daemon),
// so the soak is safe alongside production traffic.
#[tokio::test]
#[ignore = "e2e: real Feishu ws gateway, needs FEISHU_E2E_APP_ID/FEISHU_E2E_APP_SECRET"]
async fn e2e_ws_gateway_pongs_keep_connection_alive() {
    use futures::{SinkExt, StreamExt};
    use prost::Message as _;
    use tokio_tungstenite::tungstenite;

    let app_id = std::env::var("FEISHU_E2E_APP_ID").expect("FEISHU_E2E_APP_ID");
    let app_secret = std::env::var("FEISHU_E2E_APP_SECRET").expect("FEISHU_E2E_APP_SECRET");
    // The binaries do this at startup; the test binary must pick its own
    // rustls provider (both ring and aws-lc-rs are in the tree).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let adapter = super::FeishuAdapter::new(app_id, app_secret);
    let (url, service_id) = adapter.ws_endpoint().await.expect("ws endpoint");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");
    let (write, mut read) = ws.split();
    let write = std::sync::Arc::new(tokio::sync::Mutex::new(write));

    // Mirror the production ping cadence (first tick immediate, then
    // every PING_INTERVAL).
    let ping_write = std::sync::Arc::clone(&write);
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(super::PING_INTERVAL);
        loop {
            interval.tick().await;
            let mut w = ping_write.lock().await;
            let ping = super::build_ping(service_id);
            if w.send(tungstenite::Message::Binary(ping.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Soak long enough to cover 3 ping intervals; track pongs and the
    // longest silence between any two inbound frames.
    let soak = std::time::Duration::from_secs(190);
    let started = std::time::Instant::now();
    let mut last_frame_at = started;
    let mut max_gap = std::time::Duration::ZERO;
    let mut pongs = 0u32;
    while started.elapsed() < soak {
        let remaining = soak.saturating_sub(started.elapsed());
        let Ok(frame) = tokio::time::timeout(remaining, read.next()).await else {
            break; // soak window over
        };
        let frame = frame
            .expect("server closed the connection during soak")
            .expect("ws read error during soak");
        let now = std::time::Instant::now();
        max_gap = max_gap.max(now - last_frame_at);
        last_frame_at = now;
        if let tungstenite::Message::Binary(data) = frame {
            if let Ok(f) = lark_websocket_protobuf::pbbp2::Frame::decode(&data[..]) {
                let ty = f
                    .headers
                    .iter()
                    .find(|h| h.key == "type")
                    .map(|h| h.value.as_str());
                if ty == Some("pong") {
                    pongs += 1;
                    println!("[{:.1?}] pong #{pongs}", started.elapsed());
                } else {
                    println!(
                        "[{:.1?}] data frame type={ty:?} (not acked)",
                        started.elapsed()
                    );
                }
            }
        }
    }
    ping_task.abort();

    assert!(
        pongs >= 3,
        "every ping should be answered: {pongs} pongs in {soak:?}"
    );
    assert!(
        max_gap < super::FRAME_TIMEOUT,
        "longest silence {max_gap:?} must stay under FRAME_TIMEOUT {:?}",
        super::FRAME_TIMEOUT
    );
    println!("soak ok: {pongs} pongs, longest silence {max_gap:?}");
}

// e2e: send a real message, then read it back through the get-message API
// fetch_message relies on — proves the response contract (items[0], body
// content, sender) against the live service. The sent message stays in the
// target DM; use your own open_id. Needs FEISHU_E2E_USER_ID too.
#[tokio::test]
#[ignore = "e2e: real Feishu API, needs FEISHU_E2E_APP_ID/FEISHU_E2E_APP_SECRET/FEISHU_E2E_USER_ID"]
async fn e2e_fetch_message_round_trip() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let app_id = std::env::var("FEISHU_E2E_APP_ID").expect("FEISHU_E2E_APP_ID");
    let app_secret = std::env::var("FEISHU_E2E_APP_SECRET").expect("FEISHU_E2E_APP_SECRET");
    let user_id = std::env::var("FEISHU_E2E_USER_ID").expect("FEISHU_E2E_USER_ID");
    let adapter = super::FeishuAdapter::new(app_id, app_secret);

    let token = adapter.get_token().await.expect("token");
    let marker = format!("e2e 引用回读测试 {}", ulid::Ulid::new());
    let content = serde_json::json!({ "text": marker }).to_string();
    let mid = adapter
        .send_msg_to(&token, "open_id", &user_id, &content, "text")
        .await
        .expect("send")
        .expect("message id");

    let fetched = adapter
        .fetch_message(&mid)
        .await
        .expect("fetch ok")
        .expect("message found");
    assert_eq!(fetched.text, marker);
    assert!(fetched.image_keys.is_empty());
    // Sent by the bot itself — proves app senders are not filtered out
    // (quoting the bot's own answer is a primary use case).
    assert!(!fetched.sender_id.is_empty());

    // A card message reads back as its markdown body — quoting the bot's
    // own reply card must not degrade to an `[interactive]` placeholder.
    let card = serde_json::json!({
        "schema": "2.0",
        "body": { "elements": [{ "tag": "markdown", "content": marker }] }
    })
    .to_string();
    let card_mid = adapter
        .send_direct_card(&user_id, &card)
        .await
        .expect("send card")
        .expect("card message id");
    let fetched = adapter
        .fetch_message(&card_mid)
        .await
        .expect("fetch card ok")
        .expect("card message found");
    assert_eq!(fetched.text, marker);

    // A legacy v1 card has no cache entry (no markdown elements in its
    // JSON) — but the get-message echo keeps its real text in
    // legacy-rendered runs.
    let v1_card = serde_json::json!({
        "config": { "wide_screen_mode": true },
        "elements": [{ "tag": "div", "text": { "tag": "lark_md", "content": marker } }]
    })
    .to_string();
    let v1_mid = adapter
        .send_msg_to(&token, "open_id", &user_id, &v1_card, "interactive")
        .await
        .expect("send v1 card")
        .expect("v1 card message id");
    let fetched = adapter
        .fetch_message(&v1_mid)
        .await
        .expect("fetch v1 ok")
        .expect("v1 message found");
    assert_eq!(fetched.text, marker);
}

#[test]
fn build_card_rewrites_mentions() {
    // `<@USER_ID>` contract → feishu <at>; inline code untouched.
    let card = super::FeishuAdapter::build_card("cc <@ou_abc>，示例：`<@ou_x>`");
    assert!(card.contains("<at id=ou_abc></at>"), "{card}");
    assert!(card.contains("`<@ou_x>`"), "{card}");
}
