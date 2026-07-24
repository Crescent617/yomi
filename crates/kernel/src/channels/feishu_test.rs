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
                    let out = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{}",
                        resp.len(),
                        resp
                    );
                    let _ = sock.write_all(out.as_bytes()).await;
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

fn response_for(method: &str, path: &str) -> &'static str {
    let p = path.split('?').next().unwrap_or(path);
    match method {
        "POST" if p == "/open-apis/auth/v3/tenant_access_token/internal" => {
            r#"{"code":0,"msg":"ok","tenant_access_token":"tok-1","expire":7200}"#
        }
        "POST" if p == "/open-apis/im/v1/messages" => {
            r#"{"code":0,"msg":"ok","data":{"message_id":"om_new"}}"#
        }
        "POST" if p.starts_with("/open-apis/im/v1/messages/") && p.ends_with("/reply") => {
            r#"{"code":0,"msg":"ok","data":{"message_id":"om_reply"}}"#
        }
        "POST" if p.starts_with("/open-apis/im/v1/messages/") && p.ends_with("/reactions") => {
            r#"{"code":0,"msg":"ok","data":{"reaction_id":"rid_1"}}"#
        }
        "PATCH" if p.starts_with("/open-apis/im/v1/messages/") => r#"{"code":0,"msg":"ok"}"#,
        "DELETE" if p.contains("/reactions/") => r#"{"code":0,"msg":"ok"}"#,
        _ => r#"{"code":999,"msg":"unexpected request"}"#,
    }
}

fn stub_adapter(base_url: &str) -> super::FeishuAdapter {
    super::FeishuAdapter::new("app".to_string(), "secret".to_string(), false)
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
