use super::{HookContext, HookEvent, HookHandler, HookResult, PreStopDecision};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of hook handlers.
///
/// Handlers are grouped by `HookEvent` in a `HashMap` for O(1) lookup.
/// All handlers registered for a given event are evaluated in registration order.
#[derive(Default, Clone)]
pub struct HookRegistry {
    handlers: HashMap<HookEvent, Vec<Arc<dyn HookHandler>>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.values().all(|v| v.is_empty())
    }

    /// Register a hook handler. It is inserted into every event bucket
    /// returned by `handler.events()`.
    #[allow(clippy::needless_pass_by_value)]
    pub fn register(&mut self, handler: Arc<dyn HookHandler>) {
        for event in handler.events() {
            self.handlers
                .entry(*event)
                .or_default()
                .push(Arc::clone(&handler));
        }
    }

    /// Get all handlers registered for the given event.
    pub fn matching(&self, event: HookEvent) -> Vec<Arc<dyn HookHandler>> {
        self.handlers.get(&event).cloned().unwrap_or_default()
    }

    /// Convenience: run all `PreStop` handlers and merge decisions.
    ///
    /// Returns `(final_decision, steer_blocks)`.
    /// If any hook sets `continue_session: false`, the overall result is `false`.
    /// All steer blocks from all hooks are concatenated.
    pub async fn run_pre_stop(&self, ctx: &HookContext) -> HookResult {
        let handlers = self.matching(HookEvent::PreStop);
        if handlers.is_empty() {
            return HookResult::Passthrough;
        }

        let mut continue_session = None;
        let mut steer_blocks = Vec::new();

        for h in handlers {
            tracing::debug!("Running PreStop hook: {}", h.name());
            match h.run(ctx).await {
                Ok(HookResult::PreStop(d)) => {
                    if let Some(cs) = continue_session {
                        continue_session = Some(cs && d.continue_session);
                    } else {
                        continue_session = Some(d.continue_session);
                    }
                    if let Some(blocks) = d.steer_blocks {
                        steer_blocks.extend(blocks);
                    }
                }
                Err(e) => {
                    tracing::warn!("Hook '{}' failed: {}", h.name(), e);
                }
                _ => {}
            }
        }
        tracing::info!(
            "PreStop hooks resulted in continue_session={}, steer_blocks={}",
            continue_session.unwrap_or(false),
            steer_blocks.len()
        );

        HookResult::PreStop(PreStopDecision {
            continue_session: continue_session.unwrap_or(false),
            steer_blocks: steer_blocks.into(),
        })
    }

    /// Convenience: run all `PreToolUse` handlers and merge decisions.
    /// Returns (`final_decision`, `context_messages`).
    ///
    /// `context_messages` are injected as independent user messages by the agent
    /// before the tool is executed (aligned with Claude Code `additionalContext`).
    pub async fn run_pre_tool(&self, ctx: &HookContext) -> (HookResult, Vec<String>) {
        let handlers = self.matching(HookEvent::PreToolUse);
        if handlers.is_empty() {
            return (HookResult::Passthrough, Vec::new());
        }

        let mut decision = super::PreToolDecision {
            action: super::PreToolAction::Allow,
            ..Default::default()
        };
        let mut contexts = Vec::new();

        for h in handlers {
            tracing::debug!("Running PreToolUse hook: {}", h.name());
            match h.run(ctx).await {
                Ok(HookResult::PreTool(d)) => {
                    if matches!(d.action, super::PreToolAction::Block) {
                        decision.action = super::PreToolAction::Block;
                        decision.reason = d.reason.or(decision.reason);
                        // Block clears any previously collected updated_input.
                        decision.updated_input = None;
                    } else if d.updated_input.is_some() {
                        // Allow (or passthrough) with updated_input — last writer wins
                        // unless already blocked.
                        decision.updated_input = d.updated_input;
                    }
                    if let Some(ctx) = d.context {
                        contexts.push(ctx);
                    }
                }
                Err(e) => {
                    tracing::warn!("Hook '{}' failed: {}", h.name(), e);
                }
                _ => {}
            }
        }

        (HookResult::PreTool(decision), contexts)
    }

    /// Convenience: run all `PostToolUse` handlers and merge decisions.
    /// Returns (`final_decision`, `context_messages`).
    ///
    /// `context_messages` are injected as independent user messages by the agent
    /// (aligned with Claude Code `additionalContext`).
    pub async fn run_post_tool(&self, ctx: &HookContext) -> (HookResult, Vec<String>) {
        let handlers = self.matching(HookEvent::PostToolUse);
        if handlers.is_empty() {
            return (HookResult::Passthrough, Vec::new());
        }

        let mut decision = super::PostToolDecision::default();
        let mut contexts = Vec::new();

        for h in handlers {
            tracing::debug!("Running PostToolUse hook: {}", h.name());
            match h.run(ctx).await {
                Ok(HookResult::PostTool(d)) => {
                    if !d.continue_session {
                        decision.continue_session = false;
                    }
                    if d.updated_output.is_some() {
                        decision.updated_output = d.updated_output;
                    }
                    if let Some(append) = d.append_output {
                        decision.append_output =
                            Some(decision.append_output.unwrap_or_default() + &append);
                    }
                    if let Some(ctx) = d.context {
                        contexts.push(ctx);
                    }
                }
                Err(e) => {
                    tracing::warn!("Hook '{}' failed: {}", h.name(), e);
                }
                _ => {}
            }
        }

        (HookResult::PostTool(decision), contexts)
    }
}
