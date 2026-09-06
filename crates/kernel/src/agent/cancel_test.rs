use super::*;

use std::time::Duration;

#[tokio::test]
async fn test_cancel_token_basic() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());

    token.cancel();
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn test_cancel_token_reset() {
    let token = CancelToken::new();

    // 取消
    token.cancel();
    assert!(token.is_cancelled());

    // 重置
    token.reset_if_cancelled();
    assert!(!token.is_cancelled());

    // 可以再次取消
    token.cancel();
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn test_cancel_token_cancelled_future() {
    let token = CancelToken::new();

    // 未取消时，cancelled() 应该等待
    let cancelled_fut = token.cancelled();
    let token2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        token2.cancel();
    });

    tokio::time::timeout(Duration::from_millis(100), cancelled_fut)
        .await
        .expect("should complete");
}

#[tokio::test]
async fn test_cancel_token_reset_while_waiting() {
    let token = CancelToken::new();

    // 获取 cancelled future（基于当前 token）
    let cancelled_fut = token.cancelled();

    // force_reset 会创建新 token（无论是否已取消）
    token.force_reset();

    // 但 cancelled_fut 仍然监听旧的 token，不会被唤醒
    // 因为旧 token 没有被 cancel
    let result = tokio::time::timeout(Duration::from_millis(50), cancelled_fut).await;
    assert!(
        result.is_err(),
        "old token was never cancelled, so future should not complete"
    );
}

#[tokio::test]
async fn test_cancel_token_clone() {
    let token1 = CancelToken::new();
    let token2 = token1.clone();

    // 两者共享同一状态
    token1.cancel();
    assert!(token1.is_cancelled());
    assert!(token2.is_cancelled()); // token2 也应该看到取消状态

    // token2 重置也影响 token1（已取消状态）
    token2.reset_if_cancelled();
    assert!(!token1.is_cancelled());
    assert!(!token2.is_cancelled());
}

#[tokio::test]
async fn test_cancel_token_cancellation_after_reset() {
    let token = CancelToken::new();

    // 先获取一个 cancelled future
    let cancelled_fut = token.cancelled();

    // 强制重置 token（无论是否已取消）
    token.force_reset();

    // 原 future 应该无法完成（等待的是旧的已丢弃的 token）
    // 新 token 可以正常取消
    let cancelled_fut2 = token.cancelled();
    token.cancel();

    // 新 future 应该完成
    tokio::time::timeout(Duration::from_millis(10), cancelled_fut2)
        .await
        .expect("new future should complete");

    // 旧 future 应该超时（因为旧 token 不会被再取消）
    let result = tokio::time::timeout(Duration::from_millis(50), cancelled_fut).await;
    assert!(
        result.is_err(),
        "old future waits on old token which is never cancelled"
    );
}

// ── shutdown 归因 ────────────────────────────────────────────────────

#[tokio::test]
async fn test_shutdown_attribution_set_take_reset() {
    let token = CancelToken::new();

    // 普通取消不归因 shutdown。
    token.cancel();
    assert!(!token.take_for_shutdown());

    let token = CancelToken::new();
    token.cancel_for_shutdown();
    assert!(token.is_cancelled());
    // 读取即复位：一次取消只归因一次。
    assert!(token.take_for_shutdown());
    assert!(!token.take_for_shutdown());
}

#[tokio::test]
async fn test_reset_clears_shutdown_attribution() {
    let token = CancelToken::new();
    token.cancel_for_shutdown();
    token.reset_if_cancelled();
    assert!(!token.take_for_shutdown());

    let token = CancelToken::new();
    token.cancel_for_shutdown();
    token.force_reset();
    assert!(!token.take_for_shutdown());
}

#[tokio::test]
async fn test_child_token_has_independent_shutdown_attribution() {
    // subagent 的 token 经 child_token 派生：父子并发停 run 时各自
    // 读取各自的标记（共享 flag 会被 take-once 竞态吞掉一半）。
    let parent = CancelToken::new();
    let child = parent.child_token();

    parent.cancel_for_shutdown();
    // 取消经 tokio child 传播，但归因各自独立。
    assert!(child.is_cancelled());
    assert!(parent.take_for_shutdown());
    assert!(!child.take_for_shutdown());

    let parent = CancelToken::new();
    let child = parent.child_token();
    child.cancel_for_shutdown();
    assert!(!parent.take_for_shutdown());
    assert!(child.take_for_shutdown());
}
