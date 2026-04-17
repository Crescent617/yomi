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
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ArcSwap::new(Arc::new(CancellationToken::new()))),
        }
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.inner.load().cancel();
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
        }
    }

    /// 强制重置，无论是否已取消都创建新 token
    /// 注意：这会使得之前获取的 `cancelled()` future 永远等待旧 token
    pub fn force_reset(&self) {
        self.inner.store(Arc::new(CancellationToken::new()));
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

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
