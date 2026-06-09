use crate::goal::{GoalStatus, GoalStore};
use crate::hooks::{HookContext, HookEvent, HookHandler, HookResult, PreStopDecision};
use crate::types::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Goal-aware `PreStop` hook handler.
///
/// When the agent is about to stop after a streaming turn, this hook checks
/// whether an active goal exists in the store. If so, it requests the agent
/// to continue streaming and injects the goal continuation prompt as steer blocks.
pub struct GoalPreStopHandler {
    store: Arc<dyn GoalStore>,
}

impl GoalPreStopHandler {
    pub fn new(store: Arc<dyn GoalStore>) -> Self {
        Self { store }
    }
}

impl std::fmt::Debug for GoalPreStopHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalPreStopHandler")
            .field("store", &"<dyn GoalStore>")
            .finish()
    }
}

#[async_trait]
impl HookHandler for GoalPreStopHandler {
    fn name(&self) -> &'static str {
        "goal-pre-stop"
    }

    fn events(&self) -> &[HookEvent] {
        &[HookEvent::PreStop]
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookResult> {
        tracing::info!(
            "Running GoalPreStopHandler for session {}",
            ctx.session_id
        );
        match self.store.load(&ctx.session_id).await {
            Ok(Some(goal)) => {
                if matches!(goal.status, GoalStatus::Active) {
                    return Ok(HookResult::PreStop(PreStopDecision {
                        continue_session: true,
                        steer_blocks: Some(vec![crate::types::ContentBlock::Text {
                            text: goal.build_continue_prompt(),
                        }]),
                    }));
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "GoalPreStopHandler failed to load goal for session {}: {}",
                    ctx.session_id,
                    e
                );
            }
        }
        Ok(HookResult::Passthrough)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{GoalState, JsonGoalStore};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_goal_active_returns_continue() {
        let tmp = TempDir::new().unwrap();
        let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
        let mut state = GoalState::new("do something");
        state.status = GoalStatus::Active;
        store.save("sess-1", &state).await.unwrap();

        let handler = GoalPreStopHandler::new(store);
        let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
        let result = handler.run(&ctx).await.unwrap();
        match result {
            HookResult::PreStop(d) => {
                assert!(d.continue_session);
                assert!(d.steer_blocks.is_some());
            }
            _ => panic!("expected PreStop"),
        }
    }

    #[tokio::test]
    async fn test_no_goal_returns_passthrough() {
        let tmp = TempDir::new().unwrap();
        let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
        let handler = GoalPreStopHandler::new(store);
        let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
        let result = handler.run(&ctx).await.unwrap();
        assert!(matches!(result, HookResult::Passthrough));
    }

    #[tokio::test]
    async fn test_completed_goal_returns_passthrough() {
        let tmp = TempDir::new().unwrap();
        let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
        let mut state = GoalState::new("do something");
        state.status = GoalStatus::Completed;
        store.save("sess-1", &state).await.unwrap();

        let handler = GoalPreStopHandler::new(store);
        let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
        let result = handler.run(&ctx).await.unwrap();
        assert!(matches!(result, HookResult::Passthrough));
    }
}
