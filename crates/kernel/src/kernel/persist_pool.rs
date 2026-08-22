//! 会话持久化 worker 池——conductor 落盘出口（2026-08-22 洪峰丢回复
//! 根治）：`MessageAdded`/`MessageReplaced` 的慢 IO 写（生产为
//! jsonl store）从 conductor 事件循环挪进 per-session worker，循环
//! 只余同步 dispatch（µs 级）；洪峰下 bus listener 队列不再被
//! inline append 堵爆丢件（与投递层当年事故同型同治）。
//!
//! 不变式（替代旧"单循环顺序消费 + inline append"时序保证）：
//! - 同 session 落盘顺序 = conductor 事件顺序（单循环顺序
//!   dispatch + 池的 per-key FIFO）；
//! - `Stopped` 臂先 `wait_idle` 再转运/读库——最终答案必落盘；
//! - 关停 `drain_on_cancel`：daemon 重启时已入队的写排空再走。

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::types::SessionId;
use crate::utils::keyed_pool::{Handler, KeyedPool};

/// 落盘任务（Append 单条 / Replace 全量，对应 store 的两个写口）。
pub(crate) enum PersistJob {
    Append(crate::types::Message),
    Replace(Vec<crate::types::Message>),
}

/// per-session FIFO 持久化池（无 worker 状态、无节拍钩）。
pub(crate) type PersistPool = KeyedPool<SessionId, PersistJob, ()>;

/// 每 session 队列深度（对齐旧 bus listener 容量量级；打满=深度
/// 异常，池记 ERROR 丢件）。
const QUEUE_DEPTH: usize = 256;
/// 空闲巡检节拍（TTL 过期由它驱动）。
const TICK_INTERVAL: Duration = Duration::from_secs(30);
/// 空闲 TTL：超期零到达的 worker 摘牌退出（dispatch 随用随建，
/// 换代不丢序——dispatch 顺序即落盘顺序）。
const IDLE_TTL: Duration = Duration::from_mins(5);

/// 排空屏障（含上界与告警降级）：`Stopped` 转运、subagent 兜底
/// 读、interruption marker 三个调用点共用（上界
/// `DRAIN_TIMEOUT`）——病态慢盘下不为排空无限拖延（积压最坏 =
/// 队列深 × 单写延迟，无界）；超时照走（退回旧码同型窗口），
/// `what` 说明调用点以便定位。（关停全池排空的 10s 上界在
/// `Kernel::stop`，两者互参。）
pub(crate) async fn wait_drained(pool: &PersistPool, sid: &SessionId, what: &'static str) {
    wait_drained_within(pool, sid, what, DRAIN_TIMEOUT).await;
}

/// 单 key 排空上界（30s：正常为 ms 级；仅 store 病态时触达——
/// 触达即 warn，宁慢不丢）。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

async fn wait_drained_within(
    pool: &PersistPool,
    sid: &SessionId,
    what: &'static str,
    timeout: Duration,
) {
    if tokio::time::timeout(timeout, pool.wait_idle(sid))
        .await
        .is_err()
    {
        tracing::warn!("persist drain timed out for session={} ({what})", sid.0);
    }
}

/// 以 `store` 为写口构建持久化池（`token` 取消时排空队列后退出）。
pub(crate) fn build(
    store: Arc<dyn crate::storage::MessageStore>,
    token: CancellationToken,
) -> PersistPool {
    let handler: Handler<SessionId, PersistJob, ()> = Arc::new(move |sid, job, _state| {
        let store = Arc::clone(&store);
        Box::pin(async move {
            match job {
                PersistJob::Append(message) => {
                    if let Err(e) = store.append(&sid.0, &[message]).await {
                        tracing::warn!("Failed to persist message for session={}: {e}", sid.0);
                    }
                }
                PersistJob::Replace(messages) => {
                    if let Err(e) = store.replace(&sid.0, &messages).await {
                        tracing::warn!("Failed to replace messages for session={}: {e}", sid.0);
                    }
                }
            }
        })
    });
    KeyedPool::new(
        QUEUE_DEPTH,
        TICK_INTERVAL,
        IDLE_TTL,
        true,
        token,
        handler,
        None,
    )
}

#[cfg(test)]
#[path = "persist_pool_test.rs"]
mod tests;
