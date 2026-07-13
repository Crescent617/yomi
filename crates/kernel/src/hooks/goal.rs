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
    background_tasks: Arc<crate::agent::BgTaskTracker>,
}

impl GoalPreStopHandler {
    pub fn new(
        store: Arc<dyn GoalStore>,
        background_tasks: Arc<crate::agent::BgTaskTracker>,
    ) -> Self {
        Self {
            store,
            background_tasks,
        }
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
        tracing::info!("running GoalPreStopHandler");
        let session_id = crate::types::SessionId::from(ctx.session_id.clone());
        if self.background_tasks.is_running(&session_id) {
            tracing::info!(
                session_id = %ctx.session_id,
                "skipping goal auto-continue while background tasks are running"
            );
            return Ok(HookResult::Passthrough);
        }
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
                tracing::warn!("GoalPreStopHandler failed to load goal: {}", e);
            }
        }
        Ok(HookResult::Passthrough)
    }
}

#[cfg(test)]
#[path = "goal_test.rs"]
mod tests;
