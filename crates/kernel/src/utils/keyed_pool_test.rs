//! `keyed_pool` 单元测试。

use super::*;
use std::sync::Mutex as StdMutex;

fn spin_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    pred()
}

/// 记录型 handler：state 是自增计数（验证同 worker 状态延续 / 换代
/// 后重置），每次调用把 `(job, state 值)` 追加进共享日志。
type Log = Arc<StdMutex<Vec<(u32, usize)>>>;

fn recording_handler(log: Log) -> Handler<&'static str, u32, usize> {
    Arc::new(move |_key, job, mut state| {
        let log = log.clone();
        Box::pin(async move {
            state += 1;
            log.lock().expect("log").push((job, state));
            state
        })
    })
}

fn test_pool(
    log: Log,
    tick_interval: Duration,
    idle_ttl: Duration,
) -> KeyedPool<&'static str, u32, usize> {
    KeyedPool::new(
        64,
        tick_interval,
        idle_ttl,
        true,
        CancellationToken::new(),
        recording_handler(log),
        None,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_same_key_fifo_and_state_threaded() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let pool = test_pool(
        log.clone(),
        Duration::from_secs(60),
        Duration::from_secs(60),
    );
    for job in 0..20u32 {
        pool.dispatch(&"a", job);
    }
    assert!(spin_until(Duration::from_secs(5), || log
        .lock()
        .expect("log")
        .len()
        == 20));
    let entries = log.lock().expect("log").clone();
    let jobs: Vec<u32> = entries.iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, (0..20).collect::<Vec<_>>(), "同 key 必须 FIFO 保序");
    let states: Vec<usize> = entries.iter().map(|(_, s)| *s).collect();
    assert_eq!(states, (1..=20).collect::<Vec<_>>(), "state 随 worker 延续");
    assert_eq!(pool.worker_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ttl_expiry_removes_worker_and_next_dispatch_gets_fresh_state() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let pool = test_pool(
        log.clone(),
        Duration::from_millis(20),
        Duration::from_millis(60),
    );
    pool.dispatch(&"a", 1);
    assert!(spin_until(Duration::from_secs(2), || !log
        .lock()
        .expect("log")
        .is_empty()));
    assert!(
        spin_until(Duration::from_secs(2), || pool.worker_count() == 0),
        "静默超 TTL 应摘牌"
    );
    pool.dispatch(&"a", 2);
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 2));
    let states: Vec<usize> = log.lock().expect("log").iter().map(|(_, s)| *s).collect();
    assert_eq!(states, vec![1, 1], "换代后 state 必须重置");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_refreshes_ttl() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let pool = test_pool(
        log.clone(),
        Duration::from_millis(20),
        Duration::from_millis(150),
    );
    // 每 50ms 到达一次（< TTL 150ms），持续 400ms：worker 应始终存活。
    for job in 0..8u32 {
        pool.dispatch(&"a", job);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 8));
    assert_eq!(pool.worker_count(), 1, "持续到达必须续命");
    let states: Vec<usize> = log.lock().expect("log").iter().map(|(_, s)| *s).collect();
    assert_eq!(states, (1..=8).collect::<Vec<_>>(), "同一 worker 服务全程");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_panic_is_swallowed_and_worker_survives() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let panic_once = Arc::new(AtomicU32::new(0));
    let handler: Handler<&'static str, u32, usize> = {
        let log = log.clone();
        let panic_once = panic_once.clone();
        Arc::new(move |_key, job, mut state| {
            let log = log.clone();
            let panic_once = panic_once.clone();
            Box::pin(async move {
                if panic_once.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("simulated handler bug");
                }
                state += 1;
                log.lock().expect("log").push((job, state));
                state
            })
        })
    };
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        CancellationToken::new(),
        handler,
        None,
    );
    pool.dispatch(&"a", 1); // 这个 job 会 panic（应被降级吞掉）
    pool.dispatch(&"a", 2);
    pool.dispatch(&"a", 3);
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 2));
    let entries = log.lock().expect("log").clone();
    let jobs: Vec<u32> = entries.iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![2, 3], "panic 的 job 丢失，后续 job 必须继续");
    let states: Vec<usize> = entries.iter().map(|(_, s)| *s).collect();
    assert_eq!(
        states,
        vec![1, 2],
        "panic 时 state 损失重置（Default 重启）"
    );
    // 账必须照销：wait_idle 不挂（复审 must-fix 的回归门）。
    pool.wait_idle(&"a").await;
    // worker 存活（同一 worker 服务了 job 2/3）。
    assert!(!pool.workers.get("a").expect("worker").worker.is_finished());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_replacement_redelivers_after_abort() {
    // Closed 换代 Ok 臂（复审 should-fix）：abort 强杀 worker →
    // dispatch 走 Closed 原地换代并重投——新 worker 正常处理。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let pool = test_pool(
        log.clone(),
        Duration::from_secs(60),
        Duration::from_secs(60),
    );
    pool.dispatch(&"a", 1);
    assert!(spin_until(Duration::from_secs(2), || !log
        .lock()
        .expect("log")
        .is_empty()));
    pool.abort_worker(&"a");
    assert!(spin_until(Duration::from_secs(2), || {
        pool.workers
            .get("a")
            .is_some_and(|e| e.worker.is_finished())
    }));
    pool.dispatch(&"a", 2);
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 2));
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![1, 2], "换代后重投的 job 必须被新 worker 处理");
    // 账无泄漏：旧 entry 销账 + 新 entry 记账销账都走完。
    tokio::time::timeout(Duration::from_secs(2), pool.wait_idle(&"a"))
        .await
        .expect("账漏：wait_idle 挂死");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_after_cancel_is_inert() {
    // cancel 后 worker 退出（drain=false）、rx 随任务 drop：后续
    // dispatch 走 Closed 臂——pool 已 cancel **不换代**（must-fix
    // 守卫），job 丢件留痕；entry 保留、不 panic、不挂死。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let token = CancellationToken::new();
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        token.clone(),
        recording_handler(log.clone()),
        None,
    );
    pool.dispatch(&"a", 1);
    assert!(spin_until(Duration::from_secs(2), || !log
        .lock()
        .expect("log")
        .is_empty()));
    token.cancel();
    assert!(spin_until(Duration::from_secs(2), || {
        pool.workers
            .get("a")
            .is_some_and(|e| e.worker.is_finished())
    }));
    pool.dispatch(&"a", 2);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![1], "cancel 后 dispatch 必须惰性");
}

/// 双闸门 handler：job0 卡 gate0、job1 卡 gate1（entered 各自信
/// 号）——编排"cancel → 旧 worker drain 中 → dispatch 落
/// Closed"的精确时序。
fn dual_gated_handler(
    log: Log,
    entered: (Arc<Notify>, Arc<Notify>),
    gates: (Arc<Notify>, Arc<Notify>),
) -> Handler<&'static str, u32, usize> {
    Arc::new(move |_key, job, mut state| {
        let log = log.clone();
        let entered = (entered.0.clone(), entered.1.clone());
        let gates = (gates.0.clone(), gates.1.clone());
        Box::pin(async move {
            match job {
                0 => {
                    entered.0.notify_one();
                    gates.0.notified().await;
                }
                1 => {
                    entered.1.notify_one();
                    gates.1.notified().await;
                }
                _ => {}
            }
            state += 1;
            log.lock().expect("log").push((job, state));
            state
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_during_drain_does_not_respawn() {
    // fresh-eyes 终审 must-fix 回归：cancel → 旧 worker 进 drain
    // （`rx.close()` 已执行、正处理 job1）→ dispatch 落 Closed 臂
    // ——不得换代（否则新 worker 同 token 也进 drain，同 key 双
    // worker 并发写 + 旧 entry 账孤立于 `wait_all_idle`）。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let entered = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let gates = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let token = CancellationToken::new();
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        true,
        token.clone(),
        dual_gated_handler(log.clone(), entered.clone(), gates.clone()),
        None,
    );
    pool.dispatch(&"a", 0);
    pool.dispatch(&"a", 1);
    entered.0.notified().await; // job0 在 handler 卡住
    token.cancel();
    gates.0.notify_one(); // 放行 → select 走 cancel 臂 → rx.close() → drain
    entered.1.notified().await; // worker 正在 drain job1（rx 已 close）
    pool.dispatch(&"a", 2); // Closed 臂：必须丢件、不换代
    tokio::time::sleep(Duration::from_millis(100)).await;
    gates.1.notify_one(); // drain 收尾
    tokio::time::timeout(Duration::from_secs(2), pool.wait_all_idle())
        .await
        .expect("账漏：wait_all_idle 挂死");
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(
        jobs,
        vec![0, 1],
        "drain 只处理旧队列；job2 必须丢（不换代）"
    );
    assert!(
        pool.has_worker(&"a"),
        "entry 保留（旧账对 wait_all_idle 可见）"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_after_cancel_vacant_does_not_spawn() {
    // Vacant 臂同款守卫：cancel 后无 entry 的 key dispatch——不新
    // 建、不记账、丢件留痕。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let token = CancellationToken::new();
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        true,
        token.clone(),
        recording_handler(log.clone()),
        None,
    );
    token.cancel();
    pool.dispatch(&"a", 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!pool.has_worker(&"a"), "cancelled pool 不得新建 worker");
    assert!(log.lock().expect("log").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_queue_rolls_back_accounting() {
    // capacity=1 + gate 卡 worker：job1 占满队列，job2 触发 Full
    // → 账回滚（若回滚漏了，outstanding 永不归零，wait_idle 挂死）。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let pool = KeyedPool::new(
        1,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        CancellationToken::new(),
        gated_handler(log.clone(), entered.clone(), gate.clone()),
        None,
    );
    pool.dispatch(&"a", 0);
    entered.notified().await; // worker 已 recv job0 并卡住，队列空
    pool.dispatch(&"a", 1); // 占满 capacity=1
    pool.dispatch(&"a", 2); // Full → 丢件 + 账回滚
    gate.notify_one();
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 2));
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![0, 1], "Full 的 job2 必须被丢弃");
    tokio::time::timeout(Duration::from_secs(2), pool.wait_idle(&"a"))
        .await
        .expect("Full 账回滚漏记：wait_idle 挂死");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_all_idle_covers_every_key() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        CancellationToken::new(),
        gated_handler(log.clone(), entered.clone(), gate.clone()),
        None,
    );
    pool.dispatch(&"a", 0); // gate 卡住
    pool.dispatch(&"b", 1); // 快活
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 1));
    let blocked = tokio::time::timeout(Duration::from_millis(100), pool.wait_all_idle()).await;
    assert!(blocked.is_err(), "key a 卡住时 wait_all_idle 不得返回");
    gate.notify_one();
    tokio::time::timeout(Duration::from_secs(2), pool.wait_all_idle())
        .await
        .expect("wait_all_idle hung");
    assert_eq!(log.lock().expect("log").len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_idle_waits_for_inflight_handler() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let handler: Handler<&'static str, u32, usize> = {
        let log = log.clone();
        Arc::new(move |_key, job, mut state| {
            let log = log.clone();
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                state += 1;
                log.lock().expect("log").push((job, state));
                state
            })
        })
    };
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        CancellationToken::new(),
        handler,
        None,
    );
    pool.dispatch(&"a", 7);
    let start = Instant::now();
    pool.wait_idle(&"a").await;
    assert!(
        start.elapsed() >= Duration::from_millis(120),
        "wait_idle 必须等 handler 收尾"
    );
    assert_eq!(log.lock().expect("log").len(), 1);
    // 无 worker 的 key 立即返回。
    let start = Instant::now();
    pool.wait_idle(&"never-existed").await;
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_hook_fires_when_queue_empty() {
    let ticks = Arc::new(AtomicU32::new(0));
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let on_tick: TickHook<&'static str, usize> = {
        let ticks = ticks.clone();
        Arc::new(move |_key, state| {
            let ticks = ticks.clone();
            Box::pin(async move {
                ticks.fetch_add(1, Ordering::SeqCst);
                (state, false)
            })
        })
    };
    let pool = KeyedPool::new(
        64,
        Duration::from_millis(20),
        Duration::from_secs(60),
        false,
        CancellationToken::new(),
        recording_handler(log),
        Some(on_tick),
    );
    pool.dispatch(&"a", 1);
    assert!(spin_until(Duration::from_secs(2), || ticks
        .load(Ordering::SeqCst)
        >= 2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tick_hook_hold_defers_expiry() {
    // hold=true 时越过 TTL 也不摘牌（delivery 的 buffer 在飞防线）；
    // hold 解除后按 TTL 正常过期。
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let holding = Arc::new(AtomicU32::new(1));
    let on_tick: TickHook<&'static str, usize> = {
        let holding = holding.clone();
        Arc::new(move |_key, state| {
            let holding = holding.clone();
            Box::pin(async move { (state, holding.load(Ordering::SeqCst) == 1) })
        })
    };
    let pool = KeyedPool::new(
        64,
        Duration::from_millis(20),
        Duration::from_millis(60),
        false,
        CancellationToken::new(),
        recording_handler(log),
        Some(on_tick),
    );
    pool.dispatch(&"a", 1);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(pool.has_worker(&"a"), "hold 期间不得过期（已越 TTL 多拍）");
    holding.store(0, Ordering::SeqCst);
    assert!(
        spin_until(Duration::from_secs(2), || !pool.has_worker(&"a")),
        "hold 解除后必须按 TTL 过期"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_keys_get_independent_workers() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let pool = test_pool(
        log.clone(),
        Duration::from_secs(60),
        Duration::from_secs(60),
    );
    pool.dispatch(&"a", 1);
    pool.dispatch(&"b", 2);
    pool.dispatch(&"c", 3);
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 3));
    assert_eq!(pool.worker_count(), 3);
    assert!(pool.has_worker(&"a") && pool.has_worker(&"b") && pool.has_worker(&"c"));
}

// ── 关停语义 ─────────────────────────────────────────────────────────

/// 带闸门 handler：job 0 进 handler 后卡在 gate 上（`entered` 信
/// 号通知测试），放行前 cancel——drain=true 时排队 job 也必须做
/// 完才退；drain=false 时排队 job 随 cancel 丢弃。
fn gated_handler(
    log: Log,
    entered: Arc<Notify>,
    gate: Arc<Notify>,
) -> Handler<&'static str, u32, usize> {
    Arc::new(move |_key, job, mut state| {
        let log = log.clone();
        let entered = entered.clone();
        let gate = gate.clone();
        Box::pin(async move {
            if job == 0 {
                entered.notify_one();
                gate.notified().await;
            }
            state += 1;
            log.lock().expect("log").push((job, state));
            state
        })
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_on_cancel_finishes_queued_jobs() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let token = CancellationToken::new();
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        true,
        token.clone(),
        gated_handler(log.clone(), entered.clone(), gate.clone()),
        None,
    );
    pool.dispatch(&"a", 0);
    pool.dispatch(&"a", 1);
    pool.dispatch(&"a", 2);
    entered.notified().await; // job 0 已在 handler 内，1/2 在队列
    token.cancel();
    gate.notify_one();
    assert!(spin_until(Duration::from_secs(2), || log
        .lock()
        .expect("log")
        .len()
        == 3));
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![0, 1, 2], "drain 必须按 FIFO 排空队列");
    assert!(spin_until(Duration::from_secs(2), || {
        pool.workers
            .get("a")
            .is_some_and(|e| e.worker.is_finished())
    }));
    assert!(pool.is_quiet(&"a"), "排空后未了账必须归零");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_drain_cancel_drops_queued_jobs() {
    let log: Log = Arc::new(StdMutex::new(Vec::new()));
    let entered = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let token = CancellationToken::new();
    let pool = KeyedPool::new(
        64,
        Duration::from_secs(60),
        Duration::from_secs(60),
        false,
        token.clone(),
        gated_handler(log.clone(), entered.clone(), gate.clone()),
        None,
    );
    pool.dispatch(&"a", 0);
    pool.dispatch(&"a", 1);
    pool.dispatch(&"a", 2);
    entered.notified().await;
    token.cancel();
    gate.notify_one();
    assert!(spin_until(Duration::from_secs(2), || {
        pool.workers
            .get("a")
            .is_some_and(|e| e.worker.is_finished())
    }));
    let jobs: Vec<u32> = log.lock().expect("log").iter().map(|(j, _)| *j).collect();
    assert_eq!(jobs, vec![0], "无 drain：在飞的做完，排队的一律丢弃");
}
