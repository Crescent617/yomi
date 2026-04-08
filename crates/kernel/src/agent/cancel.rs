use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::Notify;

/// 可重置的取消令牌 - 支持 select! 和 reset
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<CancelTokenInner>,
}

#[derive(Debug)]
struct CancelTokenInner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancelTokenInner {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    /// 检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 重置取消状态（用于新请求）
    pub fn reset(&self) {
        self.inner.cancelled.store(false, Ordering::SeqCst);
        // Note: Notify 不需要重置，它会在新 wait 时自动处理
    }

    /// 返回 Future 用于 select! - 取消时完成
    pub fn cancelled(&self) -> CancelledFuture {
        CancelledFuture {
            inner: self.inner.clone(),
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Future that resolves when token is cancelled
#[derive(Debug, Clone)]
pub struct CancelledFuture {
    inner: Arc<CancelTokenInner>,
}

impl Future for CancelledFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Fast path: already cancelled
        if self.inner.cancelled.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }

        // Register for notification
        // We need to get a notified future and poll it
        // Since Notify::notified() returns an owned future, we need to store it
        // For simplicity, we use a different approach: just check and return Pending
        // The actual notification will be checked on next poll

        // Create a waker that will be notified when cancel() is called
        let notify = &self.inner.notify;
        let notified = notify.notified();
        tokio::pin!(notified);

        match notified.poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => {
                // Double-check after registering
                if self.inner.cancelled.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }
        }
    }
}
