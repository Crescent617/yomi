use arc_swap::ArcSwap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 可重置的取消令牌 - 使用 arc-swap + `CancellationToken` 实现安全重置
///
/// 设计：CancelToken 包含 Arc<`ArcSwap`<...>>，这样：
/// - Clone 时共享同一个 ArcSwap（共享状态）
/// - reset/cancel 操作通过 `ArcSwap` 原子性地替换 token
/// - `cancelled()` 获取当前 token 的快照，避免 reset 竞态
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<ArcSwap<CancellationToken>>,
    /// 最近一次 cancel 是否由 kernel shutdown 发起——停 run 时经
    /// `take_for_shutdown` 读取（读即复位），据此选择 interruption
    /// marker 与 `Stopped` reason（daemon 打断 vs 用户停止）。
    for_shutdown: Arc<std::sync::atomic::AtomicBool>,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(CancellationToken::new()))),
            for_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Create a `CancelToken` from an existing tokio `CancellationToken`.
    #[must_use]
    pub fn from_runtime_token(token: CancellationToken) -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(token))),
            for_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.inner.load().cancel();
    }

    /// 以 kernel shutdown 的名义取消——本次停 run 的 interruption
    /// marker 与 `Stopped` reason 携带 shutdown 标识。
    pub fn cancel_for_shutdown(&self) {
        self.for_shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        self.cancel();
    }

    /// 读取并复位 shutdown 标记（一次取消只归因一次）。
    pub fn take_for_shutdown(&self) -> bool {
        self.for_shutdown
            .swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.load().is_cancelled()
    }

    /// 如果已被取消，则重置取消状态（用于新请求）
    /// 原子性地替换为新的 `CancellationToken`
    /// 注意：如果未取消，此操作无效果
    pub fn reset_if_cancelled(&self) {
        if self.is_cancelled() {
            self.inner.store(Arc::new(CancellationToken::new()));
            self.for_shutdown
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    /// 创建子 token，当父 token 被取消时，子 token 也会被取消
    /// 子 token 的 reset 不会影响父 token
    #[must_use]
    pub fn child_token(&self) -> Self {
        let child = self.inner.load().child_token();
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(child))),
            // 独立归因（不共享）：subagent 的 token 经此派生——父子
            // agent 并发停 run 时各自读取各自的 shutdown 标记，
            // 共享一个 flag 会被 take-once 竞态吞掉一半。
            for_shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// 强制重置，无论是否已取消都创建新 token
    /// 注意：这会使得之前获取的 `cancelled()` future 永远等待旧 token
    pub fn force_reset(&self) {
        self.inner.store(Arc::new(CancellationToken::new()));
        self.for_shutdown
            .store(false, std::sync::atomic::Ordering::Release);
    }

    /// 返回 Future 用于 select! - 取消时完成
    ///
    /// 注意：调用时会通过 `load_full()` 获取当前 token 的所有权，
    /// 即使后续 reset 也会继续等待原 token，避免竞态
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> {
        // 克隆 Arc 以避免持有 arc-swap 的引用
        let token = self.inner.load_full();
        async move {
            token.cancelled().await;
        }
    }

    /// 获取当前的 tokio `CancellationToken` 用于运行时取消检查
    ///
    /// 注意：如果后续调用 reset()，此方法返回的 token 会被替换，
    /// 但已获取的 token 仍然有效（可以继续用于取消检查）
    pub fn runtime_token(&self) -> CancellationToken {
        (**self.inner.load()).clone()
    }
}

/// Check if an error is a cancellation error.
pub fn is_cancelled_error(err: &crate::types::KernelError) -> bool {
    err.is_cancelled()
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "cancel_test.rs"]
mod tests;
