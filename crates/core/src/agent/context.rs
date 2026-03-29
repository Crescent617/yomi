use crate::agent::AgentState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 在 Agent 任务和外部控制者之间共享的状态
#[derive(Debug, Clone)]
pub struct AgentExecutionContext {
    inner: Arc<AgentExecutionContextInner>,
}

#[derive(Debug)]
struct AgentExecutionContextInner {
    state_tx: tokio::sync::watch::Sender<AgentState>,
    iteration_count: AtomicUsize,
}

impl AgentExecutionContext {
    pub fn new(initial_state: AgentState) -> (Self, tokio::sync::watch::Receiver<AgentState>) {
        let (state_tx, state_rx) = tokio::sync::watch::channel(initial_state);
        let ctx = Self {
            inner: Arc::new(AgentExecutionContextInner {
                state_tx,
                iteration_count: AtomicUsize::new(0),
            }),
        };
        (ctx, state_rx)
    }

    pub fn transition_to(&self, new_state: AgentState) -> bool {
        let current = *self.inner.state_tx.borrow();
        if !current.can_transition_to(new_state) {
            tracing::warn!(
                "Invalid state transition: {:?} -> {:?}",
                current, new_state
            );
            return false;
        }
        self.inner.state_tx.send_replace(new_state);
        true
    }

    pub fn current_state(&self) -> AgentState {
        *self.inner.state_tx.borrow()
    }

    pub fn increment_iteration(&self) -> usize {
        self.inner.iteration_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn iteration_count(&self) -> usize {
        self.inner.iteration_count.load(Ordering::SeqCst)
    }
}
