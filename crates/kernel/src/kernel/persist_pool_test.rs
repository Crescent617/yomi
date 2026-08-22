//! `persist_pool` 单元测试：落盘任务经 per-session FIFO 写入 mock
//! store，`wait_idle` 作排空屏障。

use super::*;
use crate::storage::MessageStore as _;
use crate::types::{ContentBlock, Message, Result, Role};
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

/// 内存 mock：记录每次写调用（含次序），get/replace 语义与真store一致。
#[derive(Default)]
struct MemStore {
    data: StdMutex<HashMap<String, Vec<Message>>>,
    /// 写调用日志（"append"/"replace" + session + 条数）——钉顺序用。
    calls: StdMutex<Vec<(String, String, usize)>>,
}

#[async_trait::async_trait]
impl crate::storage::MessageStore for MemStore {
    async fn append(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        self.calls.lock().expect("calls").push((
            "append".to_string(),
            session_id.to_string(),
            messages.len(),
        ));
        self.data
            .lock()
            .expect("data")
            .entry(session_id.to_string())
            .or_default()
            .extend(messages.iter().cloned());
        Ok(())
    }

    async fn get(&self, session_id: &str) -> Result<Vec<Message>> {
        Ok(self
            .data
            .lock()
            .expect("data")
            .get(session_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_inlined(&self, session_id: &str) -> Result<Vec<Message>> {
        self.get(session_id).await
    }

    async fn replace(&self, session_id: &str, messages: &[Message]) -> Result<()> {
        self.calls.lock().expect("calls").push((
            "replace".to_string(),
            session_id.to_string(),
            messages.len(),
        ));
        self.data
            .lock()
            .expect("data")
            .insert(session_id.to_string(), messages.to_vec());
        Ok(())
    }
}

fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_and_replace_land_in_order_after_wait_idle() {
    let store = Arc::new(MemStore::default());
    let pool = build(store.clone(), CancellationToken::new());
    let sid = SessionId::from("s1");

    pool.dispatch(&sid, PersistJob::Append(user_msg("m1")));
    pool.dispatch(&sid, PersistJob::Append(user_msg("m2")));
    pool.dispatch(&sid, PersistJob::Replace(vec![user_msg("compacted")]));
    pool.dispatch(&sid, PersistJob::Append(user_msg("m3")));
    pool.wait_idle(&sid).await;

    let calls = store.calls.lock().expect("calls").clone();
    assert_eq!(
        calls,
        vec![
            ("append".to_string(), "s1".to_string(), 1),
            ("append".to_string(), "s1".to_string(), 1),
            ("replace".to_string(), "s1".to_string(), 1),
            ("append".to_string(), "s1".to_string(), 1),
        ],
        "同 session 落盘次序必须等于 dispatch 次序"
    );
    let msgs = store.get("s1").await.expect("get");
    let texts: Vec<&str> = msgs
        .iter()
        .filter_map(|m| m.content.first().and_then(|b| b.as_text()))
        .collect();
    assert_eq!(texts, vec!["compacted", "m3"], "replace 覆盖后再 append");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_sessions_persist_independently() {
    let store = Arc::new(MemStore::default());
    let pool = build(store.clone(), CancellationToken::new());
    let s1 = SessionId::from("s1");
    let s2 = SessionId::from("s2");

    pool.dispatch(&s1, PersistJob::Append(user_msg("a1")));
    pool.dispatch(&s2, PersistJob::Append(user_msg("b1")));
    pool.dispatch(&s1, PersistJob::Append(user_msg("a2")));
    pool.wait_idle(&s1).await;
    pool.wait_idle(&s2).await;

    let a = store.get("s1").await.expect("get s1");
    let b = store.get("s2").await.expect("get s2");
    assert_eq!(a.len(), 2);
    assert_eq!(b.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_drains_queued_writes() {
    let store = Arc::new(MemStore::default());
    let token = CancellationToken::new();
    let pool = build(store.clone(), token.clone());
    let sid = SessionId::from("s1");

    pool.dispatch(&sid, PersistJob::Append(user_msg("m1")));
    pool.dispatch(&sid, PersistJob::Append(user_msg("m2")));
    token.cancel();
    // drain 完成后 wait_idle 必返回；内容必须齐全（顺序已由池保证）。
    pool.wait_idle(&sid).await;
    let msgs = store.get("s1").await.expect("get");
    assert_eq!(msgs.len(), 2, "cancel 时已入队的写必须排空落盘");
}

/// 卡死 store：`append` 永不返回——钉 `wait_drained` 的上界降级
/// （超时照走、不挂死调用方）。
struct StuckStore;

#[async_trait::async_trait]
impl crate::storage::MessageStore for StuckStore {
    async fn append(&self, _session_id: &str, _messages: &[Message]) -> Result<()> {
        std::future::pending::<()>().await;
        #[allow(unreachable_code)]
        Ok(())
    }

    async fn get(&self, _session_id: &str) -> Result<Vec<Message>> {
        Ok(Vec::new())
    }

    async fn get_inlined(&self, _session_id: &str) -> Result<Vec<Message>> {
        Ok(Vec::new())
    }

    async fn replace(&self, _session_id: &str, _messages: &[Message]) -> Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_drained_times_out_and_returns() {
    let store = Arc::new(StuckStore);
    let pool = build(store, CancellationToken::new());
    let sid = SessionId::from("s1");
    pool.dispatch(&sid, PersistJob::Append(user_msg("m1")));
    let start = std::time::Instant::now();
    wait_drained_within(&pool, &sid, "test", Duration::from_millis(50)).await;
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "store 病态时 wait_drained 必须按上界降级返回"
    );
}
