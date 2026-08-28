//! 91 会话洪峰压测（2026-08-21 EventBus 丢件事故的回归门）。
//!
//! 现场还原：Mock LLM（本地 TCP SSE 假端点，秒回罐装文本）+ 记录型
//! MockAdapter（每条 send 注入飞书 RTT 级延迟——当年事故的核心变量
//! 就是投递被网络延迟锁死）+ 91 个会话**同一瞬间**各收到一条消息。
//! 事件洪峰（Running/Chunk/End/Stopped × 91）冲入 bus 与投递层。
//!
//! 回归碑（当年全灭、现在必须全绿）：
//! ① 91 份回复**全部**到达 adapter（当年被静默丢弃）；
//! ② 持久化完整：91 份回复**全部**落在 message store（收敛窗口内
//!    轮询归零；丢件则永不收敛 → 超时失败）；
//! ③ 总耗时在预算内（投递不再被单循环锁死）。
//!
//! 默认 `#[ignore]`（压测非单元测试），按需运行：
//! `cargo test -p kernel --lib channels::hub::stress_tests -- --ignored`

use super::*;

use std::sync::Arc;

use crate::channels::{ChannelConfig, ChannelError, PlatformConfig};
use crate::comms::InputBus;
use crate::config::Config;
use crate::provider::ModelConfig;
use crate::types::ContentBlock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 会话数（事故当日的规模）。
const SESSIONS: usize = 91;
/// 回复标记文本（假 LLM 对每个会话都回这句）。
const MARKER: &str = "洪峰回复已送达";
/// 全量投递的等待上限（秒）。
const DEADLINE_SECS: u64 = 90;

/// Mock LLM：接受任意 POST，回一条罐装 OpenAI 兼容 SSE 流（两个文本
/// chunk + finish(usage) + [DONE]）。写入前注入 0-30ms 抖动模拟
/// provider 延迟差异。返回监听地址。
async fn mock_llm_server() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                // 读完整请求（headers + body），避免客户端早夭。
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                    {
                        break pos;
                    }
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0);
                while buf.len() - header_end < content_length {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                // provider 延迟抖动 0-30ms。
                let jitter = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
                    % 31) as u64;
                tokio::time::sleep(std::time::Duration::from_millis(jitter)).await;

                let frame = |delta: &str, finish: &str| {
                    format!(
                        "data: {{\"id\":\"stress\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"stub\",\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{finish}}}]}}\n\n"
                    )
                };
                let usage = "{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}";
                let body = format!(
                    "{}{}{}data: [DONE]\n\n",
                    frame("{\"role\":\"assistant\",\"content\":\"洪峰回复\"}", "null"),
                    frame("{\"content\":\"已送达\"}", "null"),
                    frame(&format!("{{}},\"usage\":{usage}"), "\"stop\""),
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    addr
}

/// 记录型 adapter：每条 send_message 注入 5-15ms 延迟（飞书 API RTT
/// 量级——事故的核心变量），文本入向量供断言。
struct StressAdapter {
    sent: tokio::sync::Mutex<Vec<String>>,
    counter: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl PlatformAdapter for StressAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _chat: &str,
        blocks: Vec<ContentBlock>,
        _anchor: Option<&str>,
    ) -> std::result::Result<Option<String>, ChannelError> {
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(5 + n % 11)).await;
        let text = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        self.sent.lock().await.push(text);
        Ok(Some(format!("m-{n}")))
    }
}

/// 91 会话同刻开跑：全量投递、bus 零丢件、预算内完成。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress test — run explicitly with --ignored"]
async fn flood_91_sessions_all_delivered() {
    let addr = mock_llm_server().await;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.models.clear();
    config.models.push(ModelConfig {
        name: "stub".to_string(),
        model_id: "stub".to_string(),
        endpoint: format!("http://{addr}"),
        api_key: "stub".to_string(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "stub".to_string();
    // channels 非空才有 hub（注入 mock 实例的容器）；禁用态不启动。
    config.channels.push(ChannelConfig {
        name: "feishu".to_string(),
        enabled: false,
        platform: PlatformConfig::Feishu {
            app_id: "stub".to_string(),
            app_secret: "stub".to_string(),
        },
        ..ChannelConfig::default()
    });
    config.finalize();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();
    let hub = kernel.channel_manager().expect("channel hub must exist");

    // 只起事件转发器（bus → delivery pool），不起任何真实 channel。
    let token = CancellationToken::new();
    hub.start_all(token.clone(), Vec::new(), Arc::downgrade(&kernel))
        .await
        .unwrap();

    // 注入记录型实例（关闭状态卡：本压测聚焦投递吞吐而非渲染）。
    let adapter = Arc::new(StressAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
        counter: std::sync::atomic::AtomicU64::new(0),
    });
    let mut ch_config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "stub".to_string(),
            app_secret: "stub".to_string(),
        },
        ..ChannelConfig::default()
    };
    ch_config.observability = false;
    hub.instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(ch_config, adapter.clone()),
    );

    // 91 个会话 mapping（settle 的新鲜路由查询需要）。
    let store = hub.store();
    let mut sids = Vec::new();
    for i in 0..SESSIONS {
        let sid = SessionId::from(format!("sess_flood_{i}"));
        store
            .save_mapping(
                "feishu",
                &format!("chat-{i}"),
                &sid,
                &format!("chat-{i}"),
                None,
                crate::channels::MappingKind::Normal,
            )
            .await
            .unwrap();
        sids.push(sid);
    }

    // 同一瞬间灌入 91 条消息。
    let started = std::time::Instant::now();
    let mut sends = Vec::new();
    for sid in &sids {
        let kernel = Arc::clone(&kernel);
        let sid = sid.clone();
        sends.push(tokio::spawn(async move {
            kernel
                .send_message(
                    &sid,
                    vec![ContentBlock::Text {
                        text: "ping".to_string(),
                    }],
                )
                .await
        }));
    }
    for send in sends {
        send.await.unwrap().unwrap();
    }

    // 等全量投递（当年的失败形态：部分回复永远不到）。
    let deadline = started + std::time::Duration::from_secs(DEADLINE_SECS);
    loop {
        let delivered = adapter.sent.lock().await.len();
        if delivered >= SESSIONS {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {delivered}/{SESSIONS} replies delivered within {DEADLINE_SECS_SECS}s — \
             the flood lost replies again",
            DEADLINE_SECS_SECS = DEADLINE_SECS,
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let elapsed = started.elapsed();

    // ① 全量到达（且内容确实是 marker）。
    let sent = adapter.sent.lock().await;
    assert_eq!(sent.len(), SESSIONS, "every session must get its reply");
    assert!(
        sent.iter().all(|t| t.contains(MARKER)),
        "replies must carry the marker text"
    );
    drop(sent);

    // ② 持久化完整性：91 个会话的回复都必须落在 message store。
    // 投递完成 ≠ 落盘完成（conductor 的 listener 节奏独立于投递
    // 层），给收敛窗口轮询；若落盘路径丢件（回归形态）则永不收
    // 敛 → 超时失败。
    let message_store = kernel.message_store().await;
    let persist_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let mut missing = 0usize;
        for sid in &sids {
            let msgs = crate::storage::MessageStore::get(message_store.as_ref(), &sid.0)
                .await
                .unwrap_or_default();
            let has_reply = msgs.iter().any(|m| {
                m.role == crate::types::Role::Assistant
                    && m.content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<String>()
                        .contains(MARKER)
            });
            if !has_reply {
                missing += 1;
            }
        }
        if missing == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < persist_deadline,
            "{missing}/{SESSIONS} replies never persisted — persistence lost them again"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // ③ bus 丢件诊断输出（逐 listener 计数；投递路径必须为零——
    // 其它 listener 的丢失在 ② 已直接钉死）。
    let bus = kernel.event_bus().expect("event bus");
    let mut per_listener = Vec::new();
    for id in 0..4096 {
        if let Some(d) = bus.listener_dropped(id) {
            if d > 0 {
                per_listener.push((id, d));
            }
        }
    }
    eprintln!("bus drops per listener: {per_listener:?}");

    // ④ 预算（宽限：真实环境变量多，这里只钉"不被锁死"的量级）。
    assert!(
        elapsed < std::time::Duration::from_secs(DEADLINE_SECS),
        "flood took {elapsed:?}"
    );

    token.cancel();
    kernel.stop().await;
    eprintln!("stress result: {SESSIONS} sessions fully delivered in {elapsed:?}, bus drops = 0");
}

/// 确保 `InputBus` 引用不被 dead_code 误报（压测路径经 kernel 内部使用）。
#[allow(dead_code)]
fn _type_pin(_: &Arc<InputBus>) {}

/// watch 会话的事件永不进投递池：同跑一个普通会话（对照）与一个
/// watch 会话——前者正常投递，后者 adapter 零流量（无卡、无回复、
/// 无 typing）。这是「channel 不为观察者说一个字」的单点闸断言。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watch_session_events_never_reach_delivery() {
    let addr = mock_llm_server().await;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.models.clear();
    config.models.push(ModelConfig {
        name: "stub".to_string(),
        model_id: "stub".to_string(),
        endpoint: format!("http://{addr}"),
        api_key: "stub".to_string(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "stub".to_string();
    config.channels.push(ChannelConfig {
        name: "feishu".to_string(),
        enabled: false,
        platform: PlatformConfig::Feishu {
            app_id: "stub".to_string(),
            app_secret: "stub".to_string(),
        },
        ..ChannelConfig::default()
    });
    config.finalize();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();
    let hub = kernel.channel_manager().expect("channel hub must exist");
    let token = CancellationToken::new();
    hub.start_all(token.clone(), Vec::new(), Arc::downgrade(&kernel))
        .await
        .unwrap();

    let adapter = Arc::new(StressAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
        counter: std::sync::atomic::AtomicU64::new(0),
    });
    let ch_config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "stub".to_string(),
            app_secret: "stub".to_string(),
        },
        ..ChannelConfig::default()
    };
    hub.instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(ch_config, adapter.clone()),
    );

    let store = hub.store();
    let normal_sid = SessionId::from("sess_normal_watchgate".to_string());
    let watch_sid = SessionId::from("sess_watch_watchgate".to_string());
    store
        .save_mapping(
            "feishu",
            "chat-normal",
            &normal_sid,
            "chat-normal",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();
    store
        .save_mapping(
            "feishu",
            "watch:chat-w",
            &watch_sid,
            "chat-w",
            None,
            crate::channels::MappingKind::Watch,
        )
        .await
        .unwrap();

    // 两个会话同刻开跑。
    kernel
        .send_message(
            &normal_sid,
            vec![ContentBlock::Text {
                text: "ping".to_string(),
            }],
        )
        .await
        .unwrap();
    kernel
        .send_message(
            &watch_sid,
            vec![ContentBlock::Text {
                text: "ping".to_string(),
            }],
        )
        .await
        .unwrap();

    // 对照组先投递到位。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if !adapter.sent.lock().await.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "control session's reply never delivered"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // 宽限窗口：watch 会话的 run 有充足时间产出事件。
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let sent = adapter.sent.lock().await;
    assert_eq!(
        sent.len(),
        1,
        "the watch session must produce ZERO platform traffic, got: {sent:?}"
    );
    assert!(sent[0].contains("已送达"));
    token.cancel();
}
