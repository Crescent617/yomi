use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 协作式取消令牌
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<CancelTokenInner>,
}

#[derive(Debug)]
struct CancelTokenInner {
    cancelled: AtomicBool,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancelTokenInner {
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }

    /// 检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 如果已取消则返回错误
    pub fn check_cancelled(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("Operation cancelled")
        }
        Ok(())
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

    #[test]
    fn test_cancel_token() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(token.check_cancelled().is_err());
    }

    #[test]
    fn test_cancel_token_clone() {
        let token1 = CancelToken::new();
        let token2 = token1.clone();

        token1.cancel();
        assert!(token2.is_cancelled()); // Shared state
    }
}
